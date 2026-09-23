use std::sync::Mutex;

use async_trait::async_trait;
use chrono::{DateTime, Local, Utc};
use quick_xml::events::Event as XmlEvent;
use quick_xml::name::QName;
use quick_xml::reader::Reader;
use reqwest::{Client, Method};
use url::Url;

use crate::services::secrets;

use super::ics::parse_ics;
use super::model::Event;
use super::provider::{EventProvider, ProviderError, ProviderResult};
use super::recurrence::expand;

pub struct CalDavProvider {
    account_id: String,
    base_url: String,
    username: String,
    color: Option<String>,
    deletable: bool,
    client: Client,
    password: Mutex<Option<String>>,
}

impl CalDavProvider {
    pub fn new(
        account_id: impl Into<String>,
        base_url: impl Into<String>,
        username: impl Into<String>,
        color: Option<String>,
        deletable: bool,
    ) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .user_agent("metis-shell/0.2")
            .build()
            .unwrap_or_default();
        Self {
            account_id: account_id.into(),
            base_url: base_url.into(),
            username: username.into(),
            color,
            deletable,
            client,
            password: Mutex::new(None),
        }
    }

    async fn password(&self) -> Option<String> {
        if let Some(cached) = self.password.lock().ok().and_then(|g| g.clone()) {
            return Some(cached);
        }
        let pw = secrets::get(&self.account_id, secrets::CALDAV_PASSWORD)
            .await
            .ok()
            .flatten();
        if let Some(pw) = &pw
            && let Ok(mut guard) = self.password.lock()
        {
            *guard = Some(pw.clone());
        }
        pw
    }

    async fn dav_request(
        &self,
        method: &str,
        url: &str,
        depth: Option<&str>,
        body: Option<String>,
        if_match: Option<&str>,
    ) -> ProviderResult<String> {
        let password = self.password().await.unwrap_or_default();
        let method =
            Method::from_bytes(method.as_bytes()).map_err(|e| Box::new(e) as ProviderError)?;
        let mut req = self
            .client
            .request(method, url)
            .basic_auth(&self.username, Some(password))
            .header("Content-Type", "application/xml; charset=utf-8");
        if let Some(depth) = depth {
            req = req.header("Depth", depth);
        }
        if let Some(etag) = if_match {
            req = req.header("If-Match", etag);
        }
        if let Some(body) = body {
            req = req.body(body);
        }
        let resp = req.send().await.map_err(|e| Box::new(e) as ProviderError)?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() && status.as_u16() != 207 {
            return Err(format!("CalDAV {url} returned {status}").into());
        }
        Ok(text)
    }

    /// Resolve the configured base URL into one or more calendar collection URLs.
    async fn discover_calendars(&self) -> Vec<DavCalendar> {
        // First try a Depth:1 PROPFIND — works when base is a calendar or a home set.
        if let Ok(xml) = self
            .dav_request(
                "PROPFIND",
                &self.base_url,
                Some("1"),
                Some(PROPFIND_CALS.into()),
                None,
            )
            .await
        {
            let cals = self.calendars_from_multistatus(&xml);
            if !cals.is_empty() {
                return cals;
            }
        }

        // Otherwise walk principal -> calendar-home-set -> calendars.
        if let Ok(xml) = self
            .dav_request(
                "PROPFIND",
                &self.base_url,
                Some("0"),
                Some(PROPFIND_PRINCIPAL.into()),
                None,
            )
            .await
            && let Some(principal) = extract_nested_href(&xml, "current-user-principal")
            && let Some(principal_url) = self.resolve(&principal)
            && let Ok(home_xml) = self
                .dav_request(
                    "PROPFIND",
                    &principal_url,
                    Some("0"),
                    Some(PROPFIND_HOME.into()),
                    None,
                )
                .await
            && let Some(home) = extract_nested_href(&home_xml, "calendar-home-set")
            && let Some(home_url) = self.resolve(&home)
            && let Ok(list_xml) = self
                .dav_request(
                    "PROPFIND",
                    &home_url,
                    Some("1"),
                    Some(PROPFIND_CALS.into()),
                    None,
                )
                .await
        {
            return self.calendars_from_multistatus(&list_xml);
        }

        // Last resort: treat the base URL itself as a single calendar.
        vec![DavCalendar {
            url: self.base_url.clone(),
            color: self.color.clone(),
        }]
    }

    fn calendars_from_multistatus(&self, xml: &str) -> Vec<DavCalendar> {
        parse_multistatus(xml)
            .into_iter()
            .filter(|r| r.is_calendar)
            .filter_map(|r| {
                self.resolve(&r.href).map(|url| DavCalendar {
                    url,
                    color: r.color.or_else(|| self.color.clone()),
                })
            })
            .collect()
    }

    fn resolve(&self, href: &str) -> Option<String> {
        let base = Url::parse(&self.base_url).ok()?;
        base.join(href).ok().map(|u| u.to_string())
    }
}

struct DavCalendar {
    url: String,
    color: Option<String>,
}

#[async_trait]
impl EventProvider for CalDavProvider {
    fn account_id(&self) -> &str {
        &self.account_id
    }

    async fn fetch(
        &self,
        since: DateTime<Local>,
        until: DateTime<Local>,
    ) -> ProviderResult<Vec<Event>> {
        let start = since
            .with_timezone(&Utc)
            .format("%Y%m%dT%H%M%SZ")
            .to_string();
        let end = until
            .with_timezone(&Utc)
            .format("%Y%m%dT%H%M%SZ")
            .to_string();
        let body = report_body(&start, &end);

        let mut out = Vec::new();
        for calendar in self.discover_calendars().await {
            let Ok(xml) = self
                .dav_request("REPORT", &calendar.url, Some("1"), Some(body.clone()), None)
                .await
            else {
                continue;
            };
            for response in parse_multistatus(&xml) {
                if response.calendar_data.trim().is_empty() {
                    continue;
                }
                let href = self
                    .resolve(&response.href)
                    .unwrap_or(response.href.clone());
                let etag = if response.etag.is_empty() {
                    None
                } else {
                    Some(response.etag.clone())
                };
                for raw in parse_ics(&response.calendar_data) {
                    for (s, e) in expand(&raw, since, until) {
                        out.push(Event {
                            id: format!("{}:{}:{}", self.account_id, href, s.timestamp()),
                            account_id: self.account_id.clone(),
                            calendar_id: calendar.url.clone(),
                            summary: raw.summary.clone(),
                            start: s,
                            end: e,
                            all_day: raw.all_day,
                            location: raw.location.clone(),
                            color: calendar.color.clone(),
                            source_ref: Some(href.clone()),
                            etag: etag.clone(),
                            can_delete: self.deletable,
                        });
                    }
                }
            }
        }
        Ok(out)
    }

    async fn delete(&self, event: &Event) -> ProviderResult<()> {
        if !self.deletable {
            return Err("This calendar is read-only".into());
        }
        let Some(href) = &event.source_ref else {
            return Err("Event has no CalDAV href".into());
        };
        self.dav_request("DELETE", href, None, None, event.etag.as_deref())
            .await?;
        Ok(())
    }
}

const PROPFIND_CALS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav" xmlns:A="http://apple.com/ns/ical/">
  <D:prop><D:resourcetype/><D:displayname/><A:calendar-color/></D:prop>
</D:propfind>"#;

const PROPFIND_PRINCIPAL: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:"><D:prop><D:current-user-principal/></D:prop></D:propfind>"#;

const PROPFIND_HOME: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><C:calendar-home-set/></D:prop>
</D:propfind>"#;

fn report_body(start: &str, end: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<C:calendar-query xmlns:D="DAV:" xmlns:C="urn:ietf:params:xml:ns:caldav">
  <D:prop><D:getetag/><C:calendar-data/></D:prop>
  <C:filter><C:comp-filter name="VCALENDAR"><C:comp-filter name="VEVENT">
    <C:time-range start="{start}" end="{end}"/>
  </C:comp-filter></C:comp-filter></C:filter>
</C:calendar-query>"#
    )
}

#[derive(Default)]
struct DavResponse {
    href: String,
    etag: String,
    calendar_data: String,
    color: Option<String>,
    is_calendar: bool,
}

fn local_name(name: QName) -> String {
    name.local_name().as_ref().to_string()
}

/// Text events never contain entities — the reader emits those as
/// [`XmlEvent::GeneralRef`], resolved by [`decode_ref`].
fn decode_text(t: &quick_xml::events::BytesText) -> String {
    t.xml10_content().into_owned()
}

/// `&amp;` / `&#13;` / `&#x26;`: dropping these corrupts titles, hrefs and
/// the embedded iCalendar payload.
fn decode_ref(r: &quick_xml::events::BytesRef) -> Option<String> {
    match r.resolve_char_ref() {
        Ok(Some(ch)) => Some(ch.to_string()),
        Ok(None) => quick_xml::escape::resolve_xml_entity(r.as_ref()).map(str::to_string),
        Err(_) => None,
    }
}

fn parse_multistatus(xml: &str) -> Vec<DavResponse> {
    let mut reader = Reader::from_str(xml);
    let mut out = Vec::new();
    let mut cur = DavResponse::default();
    let mut in_response = false;
    let mut field: Option<&'static str> = None;
    let mut color_buf = String::new();

    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(e)) | Ok(XmlEvent::Empty(e)) => {
                let name = local_name(e.name());
                match name.as_str() {
                    "response" => {
                        in_response = true;
                        cur = DavResponse::default();
                    }
                    "href" if in_response => field = Some("href"),
                    "getetag" if in_response => field = Some("etag"),
                    "calendar-data" if in_response => field = Some("data"),
                    "calendar-color" if in_response => {
                        field = Some("color");
                        color_buf.clear();
                    }
                    "calendar" if in_response => cur.is_calendar = true,
                    _ => {}
                }
            }
            Ok(XmlEvent::Text(t)) => {
                if let Some(which) = field {
                    let text = decode_text(&t);
                    append_field(&mut cur, &mut color_buf, which, &text);
                }
            }
            Ok(XmlEvent::GeneralRef(r)) => {
                if let (Some(which), Some(text)) = (field, decode_ref(&r)) {
                    append_field(&mut cur, &mut color_buf, which, &text);
                }
            }
            Ok(XmlEvent::CData(t)) => {
                if let Some(which) = field {
                    append_field(&mut cur, &mut color_buf, which, &t.xml10_content());
                }
            }
            Ok(XmlEvent::End(e)) => {
                let name = local_name(e.name());
                match name.as_str() {
                    "href" | "getetag" | "calendar-data" => field = None,
                    "calendar-color" => {
                        let c = color_buf.trim();
                        if !c.is_empty() {
                            cur.color = Some(normalize_color(c));
                        }
                        field = None;
                    }
                    "response" => {
                        in_response = false;
                        out.push(std::mem::take(&mut cur));
                    }
                    _ => {}
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    out
}

fn append_field(cur: &mut DavResponse, color_buf: &mut String, which: &str, text: &str) {
    match which {
        "href" => cur.href.push_str(text.trim()),
        "etag" => cur.etag.push_str(text.trim()),
        "data" => cur.calendar_data.push_str(text),
        "color" => color_buf.push_str(text),
        _ => {}
    }
}

/// Apple stores colors as `#RRGGBBAA`; trim the alpha for CSS.
fn normalize_color(c: &str) -> String {
    if c.starts_with('#') && c.len() == 9 {
        c[..7].to_string()
    } else {
        c.to_string()
    }
}

/// Find the first `<href>` nested inside the named element.
fn extract_nested_href(xml: &str, element: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    let mut depth_in = 0i32;
    let mut capture = false;
    let mut value = String::new();
    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(e)) => {
                let name = local_name(e.name());
                if name == element {
                    depth_in += 1;
                } else if depth_in > 0 && name == "href" {
                    capture = true;
                    value.clear();
                }
            }
            Ok(XmlEvent::Text(t)) if capture => {
                value.push_str(&decode_text(&t));
            }
            Ok(XmlEvent::GeneralRef(r)) if capture => {
                if let Some(text) = decode_ref(&r) {
                    value.push_str(&text);
                }
            }
            Ok(XmlEvent::End(e)) => {
                let name = local_name(e.name());
                if name == "href" && capture {
                    return Some(value.trim().to_string());
                }
                if name == element && depth_in > 0 {
                    depth_in -= 1;
                }
            }
            Ok(XmlEvent::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multistatus_decodes_entity_and_char_refs() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
  <d:response>
    <d:href>/cal/a&amp;b.ics</d:href>
    <d:propstat><d:prop>
      <d:getetag>"e1"</d:getetag>
      <c:calendar-data>SUMMARY:Tom &amp; Jerry &lt;3&#13;&#x0A;END</c:calendar-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let out = parse_multistatus(xml);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].href, "/cal/a&b.ics");
        assert_eq!(out[0].etag, "\"e1\"");
        assert_eq!(out[0].calendar_data, "SUMMARY:Tom & Jerry <3\r\nEND");
    }

    #[test]
    fn nested_href_decodes_refs() {
        let xml = r#"<d:multistatus xmlns:d="DAV:"><d:response><d:propstat><d:prop>
<d:current-user-principal><d:href>/p/a&amp;b/</d:href></d:current-user-principal>
</d:prop></d:propstat></d:response></d:multistatus>"#;
        assert_eq!(
            extract_nested_href(xml, "current-user-principal").as_deref(),
            Some("/p/a&b/")
        );
    }
}
