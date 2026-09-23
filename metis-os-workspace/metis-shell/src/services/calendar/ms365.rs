use std::sync::Mutex;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use reqwest::Client;
use serde::Deserialize;

use crate::services::secrets;

use super::model::Event;
use super::provider::{EventProvider, ProviderError, ProviderResult};

const SCOPE: &str = "https://graph.microsoft.com/Calendars.ReadWrite offline_access openid profile";
const GRAPH: &str = "https://graph.microsoft.com/v1.0";

fn token_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/token")
}

#[allow(dead_code)] // MS365 device-code OAuth helper
fn devicecode_endpoint(tenant: &str) -> String {
    format!("https://login.microsoftonline.com/{tenant}/oauth2/v2.0/devicecode")
}

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(30))
        .user_agent("metis-shell/0.2")
        .build()
        .unwrap_or_default()
}

// ---- Device-code login (used by the Calendars settings page) ----

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub message: String,
    pub interval: u64,
    pub expires_in: u64,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    expires_in: Option<u64>,
    error: Option<String>,
}

#[allow(dead_code)] // MS365 device-code OAuth helper (re-exported for settings UI)
pub async fn start_device_login(tenant: &str, client_id: &str) -> ProviderResult<DeviceCode> {
    let resp = client()
        .post(devicecode_endpoint(tenant))
        .form(&[("client_id", client_id), ("scope", SCOPE)])
        .send()
        .await?
        .error_for_status()?
        .json::<DeviceCode>()
        .await?;
    Ok(resp)
}

/// Poll the token endpoint until the user authorizes, then persist the refresh
/// token in the Secret Service.
#[allow(dead_code)] // MS365 device-code OAuth helper (re-exported for settings UI)
pub async fn complete_device_login(
    account_id: &str,
    tenant: &str,
    client_id: &str,
    code: &DeviceCode,
) -> ProviderResult<()> {
    let deadline = Instant::now() + Duration::from_secs(code.expires_in);
    let interval = Duration::from_secs(code.interval.max(1));
    loop {
        if Instant::now() >= deadline {
            return Err("Device-code login timed out".into());
        }
        tokio::time::sleep(interval).await;
        let resp = client()
            .post(token_endpoint(tenant))
            .form(&[
                ("client_id", client_id),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", &code.device_code),
            ])
            .send()
            .await?
            .json::<TokenResponse>()
            .await?;

        if let Some(refresh) = resp.refresh_token {
            secrets::store(account_id, secrets::MS_REFRESH_TOKEN, &refresh).await?;
            return Ok(());
        }
        match resp.error.as_deref() {
            Some("authorization_pending") | Some("slow_down") => continue,
            Some(other) => return Err(format!("Device-code login failed: {other}").into()),
            None => continue,
        }
    }
}

// ---- Provider ----

pub struct Ms365Provider {
    account_id: String,
    tenant: String,
    client_id: String,
    color: Option<String>,
    deletable: bool,
    client: Client,
    token: Mutex<Option<(String, Instant)>>,
}

impl Ms365Provider {
    pub fn new(
        account_id: impl Into<String>,
        tenant: impl Into<String>,
        client_id: impl Into<String>,
        color: Option<String>,
        deletable: bool,
    ) -> Self {
        Self {
            account_id: account_id.into(),
            tenant: tenant.into(),
            client_id: client_id.into(),
            color,
            deletable,
            client: client(),
            token: Mutex::new(None),
        }
    }

    async fn access_token(&self) -> ProviderResult<String> {
        if let Some((token, expiry)) = self.token.lock().ok().and_then(|g| g.clone())
            && Instant::now() < expiry
        {
            return Ok(token);
        }
        let refresh = secrets::get(&self.account_id, secrets::MS_REFRESH_TOKEN)
            .await?
            .ok_or("Microsoft 365 account is not signed in")?;
        let resp = self
            .client
            .post(token_endpoint(&self.tenant))
            .form(&[
                ("client_id", self.client_id.as_str()),
                ("grant_type", "refresh_token"),
                ("refresh_token", refresh.as_str()),
                ("scope", SCOPE),
            ])
            .send()
            .await?
            .json::<TokenResponse>()
            .await?;

        if let Some(error) = resp.error {
            return Err(format!("Token refresh failed: {error}").into());
        }
        let access = resp.access_token.ok_or("No access token returned")?;
        if let Some(new_refresh) = resp.refresh_token {
            let _ = secrets::store(&self.account_id, secrets::MS_REFRESH_TOKEN, &new_refresh).await;
        }
        let ttl = resp.expires_in.unwrap_or(3600).saturating_sub(60);
        if let Ok(mut guard) = self.token.lock() {
            *guard = Some((access.clone(), Instant::now() + Duration::from_secs(ttl)));
        }
        Ok(access)
    }
}

#[derive(Deserialize)]
struct GraphPage {
    value: Vec<GraphEvent>,
    #[serde(rename = "@odata.nextLink")]
    next_link: Option<String>,
}

#[derive(Deserialize)]
struct GraphEvent {
    id: String,
    subject: Option<String>,
    #[serde(rename = "isAllDay")]
    is_all_day: Option<bool>,
    start: Option<GraphDateTime>,
    end: Option<GraphDateTime>,
    location: Option<GraphLocation>,
}

#[derive(Deserialize)]
struct GraphDateTime {
    #[serde(rename = "dateTime")]
    date_time: String,
}

#[derive(Deserialize)]
struct GraphLocation {
    #[serde(rename = "displayName")]
    display_name: Option<String>,
}

fn parse_graph_dt(value: &str) -> Option<DateTime<Local>> {
    // Graph returns e.g. "2024-06-17T13:00:00.0000000" in the requested (UTC) tz.
    let trimmed = value.split('.').next().unwrap_or(value);
    let naive = NaiveDateTime::parse_from_str(trimmed, "%Y-%m-%dT%H:%M:%S").ok()?;
    Some(Utc.from_utc_datetime(&naive).with_timezone(&Local))
}

#[async_trait]
impl EventProvider for Ms365Provider {
    fn account_id(&self) -> &str {
        &self.account_id
    }

    async fn fetch(
        &self,
        since: DateTime<Local>,
        until: DateTime<Local>,
    ) -> ProviderResult<Vec<Event>> {
        let token = self.access_token().await?;
        let start = since
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        let end = until
            .with_timezone(&Utc)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();

        let mut url = format!(
            "{GRAPH}/me/calendarView?startDateTime={start}&endDateTime={end}&$top=100&$select=id,subject,isAllDay,start,end,location"
        );
        let mut out = Vec::new();
        for _ in 0..10 {
            let page = self
                .client
                .get(&url)
                .bearer_auth(&token)
                .header("Prefer", "outlook.timezone=\"UTC\"")
                .send()
                .await?
                .error_for_status()?
                .json::<GraphPage>()
                .await?;

            for ev in page.value {
                let Some(start_dt) = ev.start.as_ref().and_then(|d| parse_graph_dt(&d.date_time))
                else {
                    continue;
                };
                let end_dt = ev
                    .end
                    .as_ref()
                    .and_then(|d| parse_graph_dt(&d.date_time))
                    .unwrap_or(start_dt + chrono::Duration::hours(1));
                out.push(Event {
                    id: format!("{}:{}", self.account_id, ev.id),
                    account_id: self.account_id.clone(),
                    calendar_id: "ms365".into(),
                    summary: ev.subject.unwrap_or_else(|| "(no title)".into()),
                    start: start_dt,
                    end: end_dt,
                    all_day: ev.is_all_day.unwrap_or(false),
                    location: ev.location.and_then(|l| l.display_name),
                    color: self.color.clone(),
                    source_ref: Some(ev.id),
                    etag: None,
                    can_delete: self.deletable,
                });
            }

            match page.next_link {
                Some(next) => url = next,
                None => break,
            }
        }
        Ok(out)
    }

    async fn delete(&self, event: &Event) -> ProviderResult<()> {
        if !self.deletable {
            return Err("This calendar is read-only".into());
        }
        let Some(id) = &event.source_ref else {
            return Err("Event has no Graph id".into());
        };
        let token = self.access_token().await?;
        self.client
            .delete(format!("{GRAPH}/me/events/{id}"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| Box::new(e) as ProviderError)?
            .error_for_status()
            .map_err(|e| Box::new(e) as ProviderError)?;
        Ok(())
    }
}
