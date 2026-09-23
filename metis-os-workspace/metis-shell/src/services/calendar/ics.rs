use chrono::{DateTime, Duration, Local, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;

/// A parsed VEVENT before recurrence expansion.
#[derive(Clone, Debug)]
pub struct RawEvent {
    pub uid: String,
    pub summary: String,
    pub start: DateTime<Local>,
    pub end: DateTime<Local>,
    pub all_day: bool,
    pub location: Option<String>,
    /// Raw "FREQ=...;..." payload (without the "RRULE:" prefix), if recurring.
    pub rrule: Option<String>,
}

type Params = Option<Vec<(String, Vec<String>)>>;

/// Parse an iCalendar document into raw (un-expanded) events.
pub fn parse_ics(text: &str) -> Vec<RawEvent> {
    let mut out = Vec::new();
    let parser = ical::IcalParser::new(text.as_bytes());
    for calendar in parser.flatten() {
        for event in calendar.events {
            if let Some(raw) = raw_from_event(&event) {
                out.push(raw);
            }
        }
    }
    out
}

fn raw_from_event(event: &ical::parser::ical::component::IcalEvent) -> Option<RawEvent> {
    let mut uid = String::new();
    let mut summary = String::new();
    let mut location = None;
    let mut rrule = None;
    let mut start: Option<(DateTime<Local>, bool)> = None;
    let mut end: Option<DateTime<Local>> = None;
    let mut duration: Option<Duration> = None;

    for prop in &event.properties {
        let value = prop.value.clone().unwrap_or_default();
        match prop.name.as_str() {
            "UID" => uid = value,
            "SUMMARY" => summary = value,
            "LOCATION" => location = Some(value),
            "RRULE" => rrule = Some(value),
            "DTSTART" => start = parse_dt(&value, &prop.params),
            "DTEND" => end = parse_dt(&value, &prop.params).map(|(d, _)| d),
            "DURATION" => duration = parse_duration(&value),
            _ => {}
        }
    }

    let (start_dt, all_day) = start?;
    let end_dt = end.unwrap_or_else(|| {
        if let Some(d) = duration {
            start_dt + d
        } else if all_day {
            start_dt + Duration::days(1)
        } else {
            start_dt + Duration::hours(1)
        }
    });

    if uid.is_empty() {
        uid = format!("{}-{}", summary, start_dt.timestamp());
    }
    if summary.is_empty() {
        summary = "(no title)".into();
    }

    Some(RawEvent {
        uid,
        summary,
        start: start_dt,
        end: end_dt,
        all_day,
        location,
        rrule,
    })
}

/// Parse a DTSTART/DTEND value with optional TZID/VALUE params into local time.
fn parse_dt(value: &str, params: &Params) -> Option<(DateTime<Local>, bool)> {
    let tzid = param(params, "TZID");
    let is_date = value.len() == 8
        || param(params, "VALUE")
            .map(|v| v.eq_ignore_ascii_case("DATE"))
            .unwrap_or(false);

    if is_date {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d").ok()?;
        let naive = date.and_hms_opt(0, 0, 0)?;
        let local = Local.from_local_datetime(&naive).single()?;
        return Some((local, true));
    }

    if let Some(stripped) = value.strip_suffix('Z') {
        let naive = NaiveDateTime::parse_from_str(stripped, "%Y%m%dT%H%M%S").ok()?;
        let utc = Utc.from_utc_datetime(&naive);
        return Some((utc.with_timezone(&Local), false));
    }

    let naive = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S").ok()?;
    if let Some(tzid) = tzid
        && let Ok(tz) = tzid.parse::<Tz>()
    {
        let dt = tz.from_local_datetime(&naive).single()?;
        return Some((dt.with_timezone(&Local), false));
    }
    let local = Local.from_local_datetime(&naive).single()?;
    Some((local, false))
}

fn param(params: &Params, key: &str) -> Option<String> {
    params.as_ref()?.iter().find_map(|(name, values)| {
        if name.eq_ignore_ascii_case(key) {
            values.first().cloned()
        } else {
            None
        }
    })
}

/// Parse an iCalendar DURATION like `PT1H30M` or `P1D`.
fn parse_duration(value: &str) -> Option<Duration> {
    let s = value.trim();
    let (sign, s) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1, s.strip_prefix('+').unwrap_or(s)),
    };
    let s = s.strip_prefix('P')?;
    let mut total = 0i64;
    let mut num = String::new();
    let mut in_time = false;
    for ch in s.chars() {
        match ch {
            'T' => in_time = true,
            '0'..='9' => num.push(ch),
            unit => {
                let n: i64 = num.parse().ok()?;
                num.clear();
                total += match (unit, in_time) {
                    ('W', _) => n * 7 * 86400,
                    ('D', _) => n * 86400,
                    ('H', true) => n * 3600,
                    ('M', true) => n * 60,
                    ('S', true) => n,
                    _ => 0,
                };
            }
        }
    }
    Some(Duration::seconds(sign * total))
}
