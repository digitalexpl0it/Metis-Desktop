mod caldav;
mod ics;
mod local;
mod model;
mod ms365;
mod provider;
mod recurrence;
mod thunderbird;

// Device-code login lives here for the (now external) settings Calendars page;
// the shell itself no longer calls it directly.
#[allow(unused_imports)]
pub use ms365::{complete_device_login, start_device_login};

pub use local::LocalEvent;
pub use model::Event;
pub use provider::EventProvider;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use chrono::{DateTime, Datelike, Days, Local, TimeZone};

use crate::config::{AccountKind, load_calendars_config};

use caldav::CalDavProvider;
use local::LocalProvider;
use ms365::Ms365Provider;
use thunderbird::ThunderbirdProvider;

/// Commands sent from the UI thread to the calendar service.
pub enum CalCommand {
    SetRange {
        since: DateTime<Local>,
        until: DateTime<Local>,
    },
    Refresh,
    /// Rebuild providers from calendars.json (after the user edits accounts).
    Reload,
    Dismiss(String),
    Delete(String),
    AddLocal(LocalEvent),
}

const REFRESH_SECS: u64 = 300;
const WATCH_SECS: u64 = 4;

/// Global handle to the calendar service so the runtime command poller can ask
/// it to rebuild providers after the settings app edits calendars.json.
static CAL_CMD_TX: OnceLock<Sender<CalCommand>> = OnceLock::new();

/// Trigger a provider rebuild from calendars.json (e.g. `reload-calendars`).
pub fn reload_calendars() {
    if let Some(tx) = CAL_CMD_TX.get() {
        let _ = tx.send(CalCommand::Reload);
    }
}

pub fn spawn_calendar_service() -> (Sender<CalCommand>, Receiver<Vec<Event>>) {
    let (cmd_tx, cmd_rx) = mpsc::channel::<CalCommand>();
    let (upd_tx, upd_rx) = mpsc::channel::<Vec<Event>>();
    let _ = CAL_CMD_TX.set(cmd_tx.clone());
    let _ = thread::Builder::new()
        .name("metis-calendar".into())
        .spawn(move || {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => service_loop(&rt, cmd_rx, upd_tx),
                Err(e) => tracing::error!(error = %e, "calendar runtime failed to start"),
            }
        });
    (cmd_tx, upd_rx)
}

fn service_loop(
    rt: &tokio::runtime::Runtime,
    cmd_rx: Receiver<CalCommand>,
    upd_tx: Sender<Vec<Event>>,
) {
    let mut providers = build_providers();
    let mut dismissed = load_dismissed();
    let (mut since, mut until) = default_range();
    let mut cache: Vec<Event> = load_cache();

    if !cache.is_empty() {
        let _ = upd_tx.send(visible(&cache, &dismissed));
    }

    cache = do_refresh(rt, &providers, since, until);
    if upd_tx.send(visible(&cache, &dismissed)).is_err() {
        return;
    }

    let mut last_full = Instant::now();
    let mut last_sig = watch_signature();

    loop {
        let mut emit = true;
        match cmd_rx.recv_timeout(Duration::from_secs(WATCH_SECS)) {
            Ok(CalCommand::SetRange { since: s, until: u }) => {
                since = s;
                until = u;
                cache = do_refresh(rt, &providers, since, until);
                last_full = Instant::now();
            }
            Ok(CalCommand::Refresh) => {
                cache = do_refresh(rt, &providers, since, until);
                last_full = Instant::now();
            }
            Ok(CalCommand::Reload) => {
                providers = build_providers();
                cache = do_refresh(rt, &providers, since, until);
                last_full = Instant::now();
            }
            Ok(CalCommand::Dismiss(id)) => {
                dismissed.insert(id);
                save_dismissed(&dismissed);
            }
            Ok(CalCommand::Delete(id)) => {
                if let Some(event) = cache.iter().find(|e| e.id == id).cloned()
                    && let Err(e) = rt.block_on(delete_event(&providers, &event))
                {
                    tracing::warn!(error = %e, "calendar delete failed");
                }
                cache = do_refresh(rt, &providers, since, until);
                last_full = Instant::now();
            }
            Ok(CalCommand::AddLocal(event)) => {
                let dir = local_dir();
                if let Err(e) = local::add_local_event(&dir, event) {
                    tracing::warn!(error = %e, "add local event failed");
                }
                cache = do_refresh(rt, &providers, since, until);
                last_full = Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {
                let sig = watch_signature();
                let changed = sig != last_sig;
                last_sig = sig;
                if changed || last_full.elapsed() >= Duration::from_secs(REFRESH_SECS) {
                    cache = do_refresh(rt, &providers, since, until);
                    last_full = Instant::now();
                } else {
                    emit = false;
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
        if emit {
            last_sig = watch_signature();
            if upd_tx.send(visible(&cache, &dismissed)).is_err() {
                break;
            }
        }
    }
}

fn do_refresh(
    rt: &tokio::runtime::Runtime,
    providers: &[Box<dyn EventProvider>],
    since: DateTime<Local>,
    until: DateTime<Local>,
) -> Vec<Event> {
    let events = rt.block_on(fetch_all(providers, since, until));
    save_cache(&events);
    events
}

/// A cheap fingerprint of the watched local files; changes trigger a refresh.
fn watch_signature() -> u64 {
    let mut sig = 0u64;
    if let Ok(entries) = std::fs::read_dir(local_dir()) {
        for entry in entries.flatten() {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified())
                && let Ok(d) = modified.duration_since(std::time::UNIX_EPOCH)
            {
                sig = sig
                    .wrapping_mul(31)
                    .wrapping_add(d.as_secs())
                    .wrapping_add(u64::from(d.subsec_nanos()));
            }
        }
    }
    sig
}

async fn fetch_all(
    providers: &[Box<dyn EventProvider>],
    since: DateTime<Local>,
    until: DateTime<Local>,
) -> Vec<Event> {
    let mut all = Vec::new();
    for provider in providers {
        match provider.fetch(since, until).await {
            Ok(events) => all.extend(events),
            Err(e) => tracing::warn!(account = provider.account_id(), error = %e, "fetch failed"),
        }
    }
    dedupe(all)
}

async fn delete_event(
    providers: &[Box<dyn EventProvider>],
    event: &Event,
) -> provider::ProviderResult<()> {
    for provider in providers {
        if provider.account_id() == event.account_id {
            return provider.delete(event).await;
        }
    }
    Err("No provider for this event".into())
}

fn dedupe(mut events: Vec<Event>) -> Vec<Event> {
    let mut seen = HashSet::new();
    events.retain(|e| seen.insert(e.id.clone()));
    events
}

fn visible(events: &[Event], dismissed: &HashSet<String>) -> Vec<Event> {
    let mut out: Vec<Event> = events
        .iter()
        .filter(|e| !dismissed.contains(&e.id))
        .cloned()
        .collect();
    out.sort_by(|a, b| a.start.cmp(&b.start).then(a.summary.cmp(&b.summary)));
    out
}

fn build_providers() -> Vec<Box<dyn EventProvider>> {
    let cfg = load_calendars_config();
    let dir = local_dir();
    let _ = std::fs::create_dir_all(&dir);

    let mut providers: Vec<Box<dyn EventProvider>> = Vec::new();
    for account in cfg.accounts.iter().filter(|a| a.enabled) {
        match account.kind {
            AccountKind::Local => {
                providers.push(Box::new(LocalProvider::new(
                    account.id.clone(),
                    dir.clone(),
                    account.color.clone(),
                )));
            }
            AccountKind::Caldav => {
                if let (Some(url), Some(username)) = (&account.url, &account.username) {
                    providers.push(Box::new(CalDavProvider::new(
                        account.id.clone(),
                        url.clone(),
                        username.clone(),
                        account.color.clone(),
                        !account.read_only,
                    )));
                } else {
                    tracing::warn!(account = %account.id, "CalDAV account missing url/username");
                }
            }
            AccountKind::Thunderbird => {
                providers.push(Box::new(ThunderbirdProvider::new(
                    account.id.clone(),
                    account.color.clone(),
                )));
                // Network calendars registered in Thunderbird show up as read-only
                // CalDAV sources (credentials, if any, come from the Secret Service).
                for net in thunderbird::network_calendars() {
                    if net.kind == "caldav" {
                        providers.push(Box::new(CalDavProvider::new(
                            format!("{}:{}", account.id, net.id),
                            net.uri,
                            net.username.unwrap_or_default(),
                            account.color.clone(),
                            false,
                        )));
                    }
                }
            }
            AccountKind::Ms365 => {
                if let (Some(tenant), Some(client_id)) = (&account.tenant, &account.client_id) {
                    providers.push(Box::new(Ms365Provider::new(
                        account.id.clone(),
                        tenant.clone(),
                        client_id.clone(),
                        account.color.clone(),
                        !account.read_only,
                    )));
                } else {
                    tracing::warn!(account = %account.id, "MS365 account missing tenant/client_id");
                }
            }
        }
    }
    providers
}

fn local_dir() -> PathBuf {
    load_calendars_config()
        .local_dir
        .map(PathBuf::from)
        .unwrap_or_else(crate::config::default_local_dir)
}

fn default_range() -> (DateTime<Local>, DateTime<Local>) {
    let now = Local::now();
    let first = now
        .date_naive()
        .with_day(1)
        .unwrap_or_else(|| now.date_naive());
    let since = first
        .checked_sub_days(Days::new(7))
        .unwrap_or(first)
        .and_hms_opt(0, 0, 0)
        .unwrap_or_default();
    let until = first
        .checked_add_days(Days::new(45))
        .unwrap_or(first)
        .and_hms_opt(23, 59, 59)
        .unwrap_or_default();
    (
        Local.from_local_datetime(&since).single().unwrap_or(now),
        Local.from_local_datetime(&until).single().unwrap_or(now),
    )
}

// ---- dismissed + cache persistence ----

fn dismissed_path() -> PathBuf {
    crate::config::config_dir().join("dismissed.json")
}

fn load_dismissed() -> HashSet<String> {
    std::fs::read_to_string(dismissed_path())
        .ok()
        .and_then(|t| serde_json::from_str::<Vec<String>>(&t).ok())
        .map(|v| v.into_iter().collect())
        .unwrap_or_default()
}

fn save_dismissed(set: &HashSet<String>) {
    let list: Vec<&String> = set.iter().collect();
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::create_dir_all(crate::config::config_dir());
        let _ = std::fs::write(dismissed_path(), json);
    }
}

fn cache_path() -> PathBuf {
    let dir = directories::ProjectDirs::from("com", "metis", "metis")
        .map(|d| d.cache_dir().join("calendars"))
        .unwrap_or_else(|| PathBuf::from(".cache/metis/calendars"));
    let _ = std::fs::create_dir_all(&dir);
    dir.join("events-cache.json")
}

fn load_cache() -> Vec<Event> {
    std::fs::read_to_string(cache_path())
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default()
}

fn save_cache(events: &[Event]) {
    if let Ok(json) = serde_json::to_string(events) {
        let _ = std::fs::write(cache_path(), json);
    }
}
