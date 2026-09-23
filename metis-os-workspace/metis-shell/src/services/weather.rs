//! Bar weather service.
//!
//! Fetches current conditions + a short hourly forecast for one or more
//! locations from Open-Meteo (keyless), on a dedicated thread with its own
//! Tokio runtime. Results are delivered to the GTK main thread over a channel.
//!
//! Location resolution order:
//!   1. `weather.json` pinned `locations` (first is the bar's primary reading)
//!   2. otherwise, auto-detect a single city from the system timezone
//!
//! Temperature unit follows `weather.json` (`auto` resolves US-style regions to
//! Fahrenheit, everything else to Celsius).

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use crate::config::{TempUnit, load_weather_config};

const REFRESH_SECS: u64 = 15 * 60;
/// Retry sooner than the normal refresh when a fetch fails (e.g. offline at
/// startup) so a transient hiccup doesn't strand the widget for 15 minutes.
const RETRY_SECS: u64 = 30;
const HOURLY_POINTS: usize = 5;

/// Auto-detected location, cached for the process lifetime so a later geocoding
/// failure can't wipe out a location we already resolved.
static CACHED_GEO: OnceLock<Mutex<Option<Geo>>> = OnceLock::new();

/// Latest snapshot shared across the weather worker and the GTK main thread.
/// Must not be `thread_local` — the worker writes it; the bar / desktop widgets
/// read it on the UI thread.
static LAST_WEATHER: Mutex<Option<WeatherSnapshot>> = Mutex::new(None);

#[derive(Debug, thiserror::Error)]
enum WeatherError {
    #[error("http request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("invalid response")]
    Invalid,
}

/// A single point in the short hourly forecast strip.
#[derive(Debug, Clone, PartialEq)]
pub struct HourlyPoint {
    /// Local hour-of-day (0–23). Formatted for display via [`hour_label`].
    pub hour24: u32,
    pub temp: f64,
    pub code: i64,
    pub is_day: bool,
}

/// Current conditions + forecast for one location.
#[derive(Debug, Clone, PartialEq)]
pub struct LocationWeather {
    pub name: String,
    pub temp: f64,
    pub code: i64,
    pub is_day: bool,
    pub label: String,
    pub high: f64,
    pub low: f64,
    pub hourly: Vec<HourlyPoint>,
}

/// Latest weather reading delivered to the bar. `locations[0]` drives the icon
/// and temperature shown in the bar itself.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct WeatherSnapshot {
    pub fahrenheit: bool,
    pub locations: Vec<LocationWeather>,
    pub error: Option<String>,
}

enum WeatherCommand {
    Refresh,
}

static WEATHER_CMD_TX: OnceLock<Sender<WeatherCommand>> = OnceLock::new();

/// Last weather snapshot — used to re-hydrate bar / desktop widgets after a
/// rebuild (the async service only pushes every ~15 minutes).
pub fn last_weather_snapshot() -> Option<WeatherSnapshot> {
    LAST_WEATHER.lock().ok().and_then(|guard| guard.clone())
}

/// Remember a snapshot on the GTK thread (also written by the worker).
pub fn remember_snapshot(snapshot: &WeatherSnapshot) {
    store_last_weather(snapshot);
}

fn store_last_weather(snapshot: &WeatherSnapshot) {
    if let Ok(mut guard) = LAST_WEATHER.lock() {
        *guard = Some(snapshot.clone());
    }
}

/// Spawn the weather background thread and return the snapshot receiver.
pub fn spawn_weather_service() -> Receiver<WeatherSnapshot> {
    let (tx, rx) = mpsc::channel();
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let _ = WEATHER_CMD_TX.set(cmd_tx);
    tracing::debug!("weather: spawning service thread");
    thread::Builder::new()
        .name("metis-weather".into())
        .spawn(move || {
            tracing::debug!("weather: thread started, building runtime");
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!(error = %err, "weather: failed to build runtime");
                    return;
                }
            };
            runtime.block_on(weather_loop(tx, cmd_rx));
            tracing::warn!("weather: loop exited");
        })
        .expect("weather thread");
    rx
}

/// Request an immediate refresh (e.g. when the popover opens).
pub fn weather_refresh() {
    if let Some(tx) = WEATHER_CMD_TX.get() {
        let _ = tx.send(WeatherCommand::Refresh);
    }
}

async fn weather_loop(tx: Sender<WeatherSnapshot>, cmd_rx: Receiver<WeatherCommand>) {
    loop {
        let snapshot = build_snapshot().await;
        tracing::debug!(
            locations = snapshot.locations.len(),
            error = ?snapshot.error,
            "weather: snapshot ready"
        );
        let failed = snapshot.error.is_some();
        store_last_weather(&snapshot);
        if tx.send(snapshot).is_err() {
            return;
        }
        // Retry quickly after a failure; otherwise wait the full refresh window.
        // Wake early on a manual refresh request. (std mpsc, polled so we stay on
        // the async thread.)
        let wait_secs = if failed { RETRY_SECS } else { REFRESH_SECS };
        let mut waited = 0u64;
        loop {
            tokio::time::sleep(Duration::from_millis(500)).await;
            waited += 500;
            if cmd_rx.try_recv().is_ok() {
                // Drain any extra queued refreshes.
                while cmd_rx.try_recv().is_ok() {}
                break;
            }
            if waited >= wait_secs * 1000 {
                break;
            }
        }
    }
}

#[derive(Debug, Clone)]
struct Geo {
    name: String,
    lat: f64,
    lon: f64,
    country_code: Option<String>,
}

async fn build_snapshot() -> WeatherSnapshot {
    let cfg = load_weather_config();

    let mut targets: Vec<Geo> = Vec::new();
    if !cfg.locations.is_empty() {
        for loc in &cfg.locations {
            targets.push(Geo {
                name: loc.name.clone(),
                lat: loc.latitude,
                lon: loc.longitude,
                country_code: None,
            });
        }
    } else if cfg.auto_detect {
        match detect_location(cfg.ip_geolocation).await {
            Some(geo) => targets.push(geo),
            None => {
                // Auto-detect is on but we couldn't resolve a location — almost
                // always a transient network/geocoding failure. Report it as
                // unavailable (it retries soon) rather than asking the user to
                // configure something.
                return WeatherSnapshot {
                    error: Some("Weather unavailable".into()),
                    ..Default::default()
                };
            }
        }
    }

    if targets.is_empty() {
        // Auto-detect disabled and no pinned locations.
        return WeatherSnapshot {
            error: Some("Set a location in Settings".into()),
            ..Default::default()
        };
    }

    let fahrenheit = match cfg.unit {
        TempUnit::Fahrenheit => true,
        TempUnit::Celsius => false,
        TempUnit::Auto => infer_fahrenheit(&targets),
    };

    let mut locations = Vec::new();
    let mut last_err = None;
    for target in &targets {
        match fetch_location(target, fahrenheit).await {
            Ok(weather) => locations.push(weather),
            Err(err) => {
                tracing::warn!(location = %target.name, error = %err, "weather: forecast fetch failed");
                last_err = Some(format!("{err}"));
            }
        }
    }

    if locations.is_empty() {
        return WeatherSnapshot {
            fahrenheit,
            error: Some(last_err.unwrap_or_else(|| "Weather unavailable".into())),
            ..Default::default()
        };
    }

    WeatherSnapshot {
        fahrenheit,
        locations,
        error: None,
    }
}

/// Resolve (and cache) the auto-detect location.
///
/// Accuracy order: IP geolocation (city-level, needs network) → system
/// `zoneinfo` tables (offline, coarse zone anchor city) → network geocode of the
/// timezone city. The first success is cached for the process lifetime.
async fn detect_location(ip_enabled: bool) -> Option<Geo> {
    if let Some(cached) = cached_geo() {
        return Some(cached);
    }

    if ip_enabled {
        if let Some(geo) = ip_geolocate().await {
            tracing::info!(city = %geo.name, lat = geo.lat, lon = geo.lon, "weather: location from IP geolocation");
            set_cached_geo(geo.clone());
            return Some(geo);
        }
        tracing::warn!("weather: IP geolocation unavailable — falling back to timezone");
    }

    let Some(tz) = system_timezone() else {
        tracing::warn!("weather: could not read the system timezone");
        return None;
    };

    if let Some(geo) = tz_geo(&tz) {
        tracing::info!(tz = %tz, city = %geo.name, lat = geo.lat, lon = geo.lon, "weather: location from zoneinfo");
        set_cached_geo(geo.clone());
        return Some(geo);
    }

    let Some(city) = tz_city(&tz) else {
        tracing::warn!(%tz, "weather: could not derive a city from the timezone");
        return None;
    };
    match geocode(&city).await {
        Some(geo) => {
            tracing::info!(city = %geo.name, lat = geo.lat, lon = geo.lon, "weather: location from geocoder");
            set_cached_geo(geo.clone());
            Some(geo)
        }
        None => {
            tracing::warn!(%city, "weather: geocoding failed for detected timezone city");
            None
        }
    }
}

/// City-level location from the public IP, via the keyless ipwho.is service.
async fn ip_geolocate() -> Option<Geo> {
    let resp = match http_client().get("https://ipwho.is/").send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(error = %err, "weather: IP geolocation request failed");
            return None;
        }
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!(error = %err, "weather: IP geolocation parse failed");
            return None;
        }
    };
    if !json
        .get("success")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let lat = json.get("latitude")?.as_f64()?;
    let lon = json.get("longitude")?.as_f64()?;
    let city = json
        .get("city")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    let region = json.get("region").and_then(|v| v.as_str());
    let name = city.unwrap_or_else(|| region.unwrap_or("Current location").to_string());
    Some(Geo {
        name,
        lat,
        lon,
        country_code: json
            .get("country_code")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// Look up coordinates for an IANA timezone from the system `zoneinfo` tables
/// (`zone1970.tab` / `zone.tab`). These ship with tzdata, so no network needed.
fn tz_geo(tz: &str) -> Option<Geo> {
    const TAB_PATHS: [&str; 2] = [
        "/usr/share/zoneinfo/zone1970.tab",
        "/usr/share/zoneinfo/zone.tab",
    ];
    for path in TAB_PATHS {
        let Ok(content) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in content.lines() {
            if line.starts_with('#') {
                continue;
            }
            let mut cols = line.split('\t');
            let (Some(codes), Some(coords), Some(name)) = (cols.next(), cols.next(), cols.next())
            else {
                continue;
            };
            if name != tz {
                continue;
            }
            let Some((lat, lon)) = parse_iso6709(coords) else {
                continue;
            };
            return Some(Geo {
                name: tz_city(tz).unwrap_or_else(|| tz.to_string()),
                lat,
                lon,
                country_code: codes.split(',').next().map(|c| c.to_string()),
            });
        }
    }
    None
}

/// Parse ISO 6709 coordinates as used by the tzdata tables, e.g.
/// `+340308-1181434` -> (34.0522, -118.2428).
fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let bytes = s.as_bytes();
    if bytes.len() < 2 {
        return None;
    }
    // The longitude begins at the next sign after the leading latitude sign.
    let split = (1..bytes.len()).find(|&i| bytes[i] == b'+' || bytes[i] == b'-')?;
    let lat = parse_angle(&s[..split], 2)?;
    let lon = parse_angle(&s[split..], 3)?;
    Some((lat, lon))
}

/// Parse a signed ±DDMM[SS] / ±DDDMM[SS] angle into decimal degrees.
fn parse_angle(s: &str, deg_len: usize) -> Option<f64> {
    let sign = match s.as_bytes().first()? {
        b'+' => 1.0,
        b'-' => -1.0,
        _ => return None,
    };
    let digits = &s[1..];
    let deg: f64 = digits.get(..deg_len)?.parse().ok()?;
    let min: f64 = digits.get(deg_len..deg_len + 2)?.parse().ok()?;
    let sec: f64 = match digits.get(deg_len + 2..deg_len + 4) {
        Some(ss) => ss.parse().ok()?,
        None => 0.0,
    };
    Some(sign * (deg + min / 60.0 + sec / 3600.0))
}

/// Identifying User-Agent. MET Norway's API *requires* a unique UA (it rejects
/// requests without one), and it's polite for the other keyless services too.
const USER_AGENT: &str = concat!(
    "MetisDesktop/",
    env!("CARGO_PKG_VERSION"),
    " (https://github.com/metis-os)"
);

/// Shared HTTP client with a hard timeout so a stalled host can never hang the
/// weather thread (it just fails fast, logs, and retries on the next cycle).
fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .user_agent(USER_AGENT)
        .build()
        .unwrap_or_default()
}

fn cached_geo() -> Option<Geo> {
    CACHED_GEO
        .get_or_init(|| Mutex::new(None))
        .lock()
        .ok()
        .and_then(|g| g.clone())
}

fn set_cached_geo(geo: Geo) {
    if let Ok(mut slot) = CACHED_GEO.get_or_init(|| Mutex::new(None)).lock() {
        *slot = Some(geo);
    }
}

async fn geocode(query: &str) -> Option<Geo> {
    let url = format!(
        "https://geocoding-api.open-meteo.com/v1/search?name={}&count=1&language=en&format=json",
        encode(query)
    );
    let resp = match http_client().get(&url).send().await {
        Ok(resp) => resp,
        Err(err) => {
            tracing::warn!(error = %err, "weather: geocode request failed");
            return None;
        }
    };
    let json: serde_json::Value = match resp.json().await {
        Ok(json) => json,
        Err(err) => {
            tracing::warn!(error = %err, "weather: geocode response parse failed");
            return None;
        }
    };
    let first = json.get("results")?.as_array()?.first()?;
    Some(Geo {
        name: first.get("name")?.as_str()?.to_string(),
        lat: first.get("latitude")?.as_f64()?,
        lon: first.get("longitude")?.as_f64()?,
        country_code: first
            .get("country_code")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string()),
    })
}

/// Fetch a location's weather, trying providers in order so a single upstream
/// host being unreachable (e.g. `api.open-meteo.com` blackholed on some
/// networks) doesn't strand the widget. Open-Meteo is primary; MET Norway
/// (`api.met.no`, a different host/operator) is the automatic fallback.
async fn fetch_location(geo: &Geo, fahrenheit: bool) -> Result<LocationWeather, WeatherError> {
    match fetch_openmeteo(geo, fahrenheit).await {
        Ok(weather) => Ok(weather),
        Err(primary) => {
            tracing::warn!(
                location = %geo.name,
                error = %primary,
                "weather: Open-Meteo failed — trying MET Norway fallback"
            );
            match fetch_metno(geo, fahrenheit).await {
                Ok(weather) => {
                    tracing::info!(location = %geo.name, "weather: served by MET Norway fallback");
                    Ok(weather)
                }
                Err(fallback) => {
                    tracing::warn!(
                        location = %geo.name,
                        error = %fallback,
                        "weather: MET Norway fallback also failed"
                    );
                    // Surface the primary provider's error to the user.
                    Err(primary)
                }
            }
        }
    }
}

async fn fetch_openmeteo(geo: &Geo, fahrenheit: bool) -> Result<LocationWeather, WeatherError> {
    let unit = if fahrenheit { "fahrenheit" } else { "celsius" };
    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}\
         &current=temperature_2m,weather_code,is_day\
         &hourly=temperature_2m,weather_code,is_day\
         &daily=temperature_2m_max,temperature_2m_min\
         &temperature_unit={unit}&timezone=auto&forecast_days=2",
        lat = geo.lat,
        lon = geo.lon,
    );
    let resp: serde_json::Value = http_client().get(&url).send().await?.json().await?;

    let current = resp.get("current").ok_or(WeatherError::Invalid)?;
    let temp = current
        .get("temperature_2m")
        .and_then(|v| v.as_f64())
        .ok_or(WeatherError::Invalid)?;
    let code = current
        .get("weather_code")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let is_day = current.get("is_day").and_then(|v| v.as_i64()).unwrap_or(1) != 0;
    let current_time = current.get("time").and_then(|v| v.as_str()).unwrap_or("");

    let daily = resp.get("daily");
    let high = daily
        .and_then(|d| d.get("temperature_2m_max"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(temp);
    let low = daily
        .and_then(|d| d.get("temperature_2m_min"))
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .and_then(|v| v.as_f64())
        .unwrap_or(temp);

    let hourly = parse_hourly(resp.get("hourly"), current_time);

    Ok(LocationWeather {
        name: geo.name.clone(),
        temp,
        code,
        is_day,
        label: weather_label(code),
        high,
        low,
        hourly,
    })
}

/// Fallback provider: MET Norway Locationforecast 2.0 (`api.met.no`).
///
/// Reports in Celsius on an hourly timeseries; we convert to the requested unit
/// and translate MET's `symbol_code` strings into the same WMO codes the rest
/// of the UI already understands. Coordinates are truncated to 4 decimals per
/// MET's terms of service (improves their cache hit rate).
async fn fetch_metno(geo: &Geo, fahrenheit: bool) -> Result<LocationWeather, WeatherError> {
    let url = format!(
        "https://api.met.no/weatherapi/locationforecast/2.0/compact?lat={lat:.4}&lon={lon:.4}",
        lat = geo.lat,
        lon = geo.lon,
    );
    let resp: serde_json::Value = http_client().get(&url).send().await?.json().await?;

    let series = resp
        .get("properties")
        .and_then(|p| p.get("timeseries"))
        .and_then(|t| t.as_array())
        .filter(|a| !a.is_empty())
        .ok_or(WeatherError::Invalid)?;

    let first = &series[0];
    let temp_c = metno_instant_temp(first).ok_or(WeatherError::Invalid)?;
    let temp = to_unit(temp_c, fahrenheit);

    let symbol = metno_symbol(first).unwrap_or("");
    let (code, sym_is_day) = metno_symbol_to_code(symbol);
    let is_day = sym_is_day.unwrap_or_else(|| metno_is_daytime(first));

    // High/low across the next ~24 hourly points (a reasonable "today" window;
    // MET doesn't provide a daily min/max like Open-Meteo does).
    let mut high = temp;
    let mut low = temp;
    for entry in series.iter().take(24) {
        if let Some(t) = metno_instant_temp(entry).map(|c| to_unit(c, fahrenheit)) {
            high = high.max(t);
            low = low.min(t);
        }
    }

    let hourly = metno_hourly(series, fahrenheit);

    Ok(LocationWeather {
        name: geo.name.clone(),
        temp,
        code,
        is_day,
        label: weather_label(code),
        high,
        low,
        hourly,
    })
}

/// Instant air temperature (°C) from a MET Norway timeseries entry.
fn metno_instant_temp(entry: &serde_json::Value) -> Option<f64> {
    entry
        .get("data")?
        .get("instant")?
        .get("details")?
        .get("air_temperature")?
        .as_f64()
}

/// The most specific `symbol_code` available for a MET Norway entry, preferring
/// the shortest forecast window.
fn metno_symbol(entry: &serde_json::Value) -> Option<&str> {
    let data = entry.get("data")?;
    for key in ["next_1_hours", "next_6_hours", "next_12_hours"] {
        if let Some(sym) = data
            .get(key)
            .and_then(|h| h.get("summary"))
            .and_then(|s| s.get("symbol_code"))
            .and_then(|v| v.as_str())
        {
            return Some(sym);
        }
    }
    None
}

/// Rough daytime heuristic for entries whose symbol has no day/night suffix
/// (e.g. `cloudy`, `rain`): treat 06:00–19:59 local as day.
fn metno_is_daytime(entry: &serde_json::Value) -> bool {
    entry
        .get("time")
        .and_then(|v| v.as_str())
        .and_then(local_hour)
        .map(|h| (6..20).contains(&h))
        .unwrap_or(true)
}

/// Build the short hourly strip from a MET Norway timeseries.
fn metno_hourly(series: &[serde_json::Value], fahrenheit: bool) -> Vec<HourlyPoint> {
    let mut points = Vec::new();
    for entry in series.iter().take(HOURLY_POINTS) {
        let Some(time) = entry.get("time").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(hour) = local_hour(time) else {
            continue;
        };
        let temp = to_unit(metno_instant_temp(entry).unwrap_or(0.0), fahrenheit);
        let symbol = metno_symbol(entry).unwrap_or("");
        let (code, sym_is_day) = metno_symbol_to_code(symbol);
        let is_day = sym_is_day.unwrap_or_else(|| (6..20).contains(&hour));
        points.push(HourlyPoint {
            hour24: hour,
            temp,
            code,
            is_day,
        });
    }
    points
}

/// Convert an RFC 3339 UTC timestamp to the local hour-of-day (0–23).
fn local_hour(rfc3339: &str) -> Option<u32> {
    let dt = chrono::DateTime::parse_from_rfc3339(rfc3339).ok()?;
    let local = dt.with_timezone(&chrono::Local);
    Some(chrono::Timelike::hour(&local))
}

/// Celsius → the requested display unit.
fn to_unit(celsius: f64, fahrenheit: bool) -> f64 {
    if fahrenheit {
        celsius * 9.0 / 5.0 + 32.0
    } else {
        celsius
    }
}

/// Map a MET Norway `symbol_code` (e.g. `partlycloudy_day`, `heavyrain`,
/// `lightsnowshowersandthunder_night`) to a WMO code the UI icon table
/// understands, plus an explicit day/night flag when the symbol carries one.
fn metno_symbol_to_code(symbol: &str) -> (i64, Option<bool>) {
    let (base, is_day) = if let Some(b) = symbol.strip_suffix("_day") {
        (b, Some(true))
    } else if let Some(b) = symbol.strip_suffix("_night") {
        (b, Some(false))
    } else if let Some(b) = symbol.strip_suffix("_polartwilight") {
        (b, Some(true))
    } else {
        (symbol, None)
    };
    let heavy = base.starts_with("heavy");
    let light = base.starts_with("light");
    // Order matters — check compound conditions before their substrings.
    let code = if base.contains("thunder") {
        95
    } else if base.contains("snowshowers") {
        if heavy { 86 } else { 85 }
    } else if base.contains("snow") {
        if heavy {
            75
        } else if light {
            71
        } else {
            73
        }
    } else if base.contains("sleet") {
        // Freezing/mixed precip — closest WMO bucket is freezing rain.
        if heavy { 67 } else { 66 }
    } else if base.contains("rainshowers") {
        if heavy {
            82
        } else if light {
            80
        } else {
            81
        }
    } else if base.contains("rain") {
        if heavy {
            65
        } else if light {
            61
        } else {
            63
        }
    } else if base.contains("drizzle") {
        51
    } else if base == "fog" {
        45
    } else if base == "cloudy" {
        3
    } else if base == "partlycloudy" {
        2
    } else if base == "fair" {
        1
    } else if base == "clearsky" {
        0
    } else {
        // Unknown/empty — overcast is the safest neutral glyph.
        3
    };
    (code, is_day)
}

fn parse_hourly(hourly: Option<&serde_json::Value>, current_time: &str) -> Vec<HourlyPoint> {
    let Some(hourly) = hourly else {
        return Vec::new();
    };
    let times = hourly.get("time").and_then(|v| v.as_array());
    let temps = hourly.get("temperature_2m").and_then(|v| v.as_array());
    let codes = hourly.get("weather_code").and_then(|v| v.as_array());
    let days = hourly.get("is_day").and_then(|v| v.as_array());
    let (Some(times), Some(temps)) = (times, temps) else {
        return Vec::new();
    };

    // Compare on the "YYYY-MM-DDTHH" prefix so a minute-level current time still
    // aligns to the hourly buckets.
    let cur_hour = current_time.get(..13).unwrap_or(current_time);
    let start = times
        .iter()
        .position(|t| {
            t.as_str()
                .map(|s| s.get(..13).unwrap_or(s) >= cur_hour)
                .unwrap_or(false)
        })
        .unwrap_or(0);

    let mut points = Vec::new();
    for (i, time) in times.iter().enumerate().skip(start).take(HOURLY_POINTS) {
        let hour24 = time
            .as_str()
            .and_then(|s| s.get(11..13))
            .and_then(|h| h.parse::<u32>().ok())
            .unwrap_or(0);
        let temp = temps.get(i).and_then(|v| v.as_f64()).unwrap_or(0.0);
        let code = codes
            .and_then(|c| c.get(i))
            .and_then(|v| v.as_i64())
            .unwrap_or(0);
        let is_day = days
            .and_then(|d| d.get(i))
            .and_then(|v| v.as_i64())
            .unwrap_or(1)
            != 0;
        points.push(HourlyPoint {
            hour24,
            temp,
            code,
            is_day,
        });
    }
    points
}

/// Localized WMO condition name for UI chrome (bar overlay, desktop widget).
pub fn condition_label(code: i64) -> String {
    metis_i18n::tr(weather_msgid(code))
}

/// Localized hour chip for the hourly strip (`3PM`, `12AM`, …).
pub fn hour_label(hour24: u32) -> String {
    let (h12, suffix_key) = match hour24 % 24 {
        0 => (12, "AM"),
        h @ 1..=11 => (h, "AM"),
        12 => (12, "PM"),
        h => (h - 12, "PM"),
    };
    format!("{h12}{}", metis_i18n::tr(suffix_key))
}

fn weather_msgid(code: i64) -> &'static str {
    match code {
        0 => "Clear",
        1 => "Mostly clear",
        2 => "Partly cloudy",
        3 => "Cloudy",
        45 | 48 => "Fog",
        51..=57 => "Drizzle",
        61..=67 => "Rain",
        71..=77 => "Snow",
        80..=82 => "Showers",
        85 | 86 => "Snow showers",
        95..=99 => "Thunderstorm",
        _ => "Weather",
    }
}

fn weather_label(code: i64) -> String {
    condition_label(code)
}

/// US-style regions report in Fahrenheit; otherwise fall back to the locale.
fn infer_fahrenheit(targets: &[Geo]) -> bool {
    const F_REGIONS: [&str; 8] = ["US", "PR", "GU", "VI", "AS", "MP", "LR", "MM"];
    if targets.iter().any(|g| {
        g.country_code
            .as_deref()
            .map(|c| F_REGIONS.contains(&c))
            .unwrap_or(false)
    }) {
        return true;
    }
    std::env::var("LC_MEASUREMENT")
        .or_else(|_| std::env::var("LANG"))
        .map(|l| l.contains("US"))
        .unwrap_or(false)
}

/// Resolve the IANA timezone name (e.g. `America/New_York`).
fn system_timezone() -> Option<String> {
    if let Ok(tz) = std::env::var("TZ") {
        let tz = tz.trim_start_matches(':').trim().to_string();
        if !tz.is_empty() {
            return Some(tz);
        }
    }
    if let Ok(contents) = std::fs::read_to_string("/etc/timezone") {
        let tz = contents.trim().to_string();
        if !tz.is_empty() {
            return Some(tz);
        }
    }
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        let path = target.to_string_lossy();
        if let Some(idx) = path.find("zoneinfo/") {
            let tz = path[idx + "zoneinfo/".len()..].to_string();
            if !tz.is_empty() {
                return Some(tz);
            }
        }
    }
    None
}

/// Extract a geocodable city from an IANA timezone (`America/New_York` -> `New York`).
fn tz_city(tz: &str) -> Option<String> {
    let city = tz.rsplit('/').next()?.replace('_', " ");
    if city.is_empty() || city.eq_ignore_ascii_case("UTC") || city.eq_ignore_ascii_case("GMT") {
        return None;
    }
    Some(city)
}

fn encode(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_iso6709_la() {
        let (lat, lon) = parse_iso6709("+340308-1181434").expect("parse");
        assert!((lat - 34.0522).abs() < 0.01, "lat={lat}");
        assert!((lon + 118.2428).abs() < 0.01, "lon={lon}");
    }

    #[test]
    fn tz_city_strips_zone() {
        assert_eq!(tz_city("America/New_York").as_deref(), Some("New York"));
        assert_eq!(tz_city("Etc/UTC"), None);
    }

    #[test]
    fn maps_metno_symbols_to_wmo_codes() {
        assert_eq!(metno_symbol_to_code("clearsky_day"), (0, Some(true)));
        assert_eq!(metno_symbol_to_code("clearsky_night"), (0, Some(false)));
        assert_eq!(metno_symbol_to_code("fair_day"), (1, Some(true)));
        assert_eq!(metno_symbol_to_code("partlycloudy_night"), (2, Some(false)));
        assert_eq!(metno_symbol_to_code("cloudy"), (3, None));
        assert_eq!(metno_symbol_to_code("fog"), (45, None));
        assert_eq!(metno_symbol_to_code("lightrain"), (61, None));
        assert_eq!(metno_symbol_to_code("heavyrain"), (65, None));
        assert_eq!(metno_symbol_to_code("rainshowers_day"), (81, Some(true)));
        assert_eq!(metno_symbol_to_code("heavysnow"), (75, None));
        assert_eq!(metno_symbol_to_code("snowshowers_day"), (85, Some(true)));
        // Thunder wins over the precip type it's paired with.
        assert_eq!(
            metno_symbol_to_code("rainshowersandthunder_day"),
            (95, Some(true))
        );
        // Unknown symbol falls back to overcast with no day/night hint.
        assert_eq!(metno_symbol_to_code(""), (3, None));
    }

    #[test]
    fn celsius_converts_to_fahrenheit() {
        assert!((to_unit(0.0, true) - 32.0).abs() < 0.001);
        assert!((to_unit(100.0, true) - 212.0).abs() < 0.001);
        assert!((to_unit(21.0, false) - 21.0).abs() < 0.001);
    }

    #[test]
    fn detects_system_location_offline() {
        // This machine has tzdata; ensure offline detection yields coordinates.
        if let Some(tz) = system_timezone()
            && let Some(geo) = tz_geo(&tz)
        {
            assert!(geo.lat.abs() <= 90.0);
            assert!(geo.lon.abs() <= 180.0);
            eprintln!("detected: {} ({}, {})", geo.name, geo.lat, geo.lon);
        }
    }
}
