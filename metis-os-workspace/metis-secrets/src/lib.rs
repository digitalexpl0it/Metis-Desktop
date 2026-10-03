//! Credential storage via the freedesktop Secret Service (gnome-keyring, KWallet,
//! KeePassXC, ...). DE-agnostic: this only speaks the standard D-Bus interface.
//!
//! Shared by `metis-shell` (calendar sync) and `metis-settings` (account editing)
//! so both read/write the same keyring items.

use std::collections::HashMap;

use oo7::{Keyring, Secret};

pub type SecretResult<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Attribute "kind" for a CalDAV account password.
pub const CALDAV_PASSWORD: &str = "caldav_password";
/// Attribute "kind" for a Microsoft 365 OAuth refresh token.
pub const MS_REFRESH_TOKEN: &str = "ms_refresh_token";
/// Attribute "kind" for Metis Viewer host passwords (RDP / Metis Remote).
/// Values live in the Secret Service only — never in `viewer.json`.
pub const VIEWER_PASSWORD: &str = "viewer_password";

fn attributes<'a>(account: &'a str, kind: &'a str) -> HashMap<&'a str, &'a str> {
    let mut map = HashMap::new();
    map.insert("app", "metis");
    map.insert("account", account);
    map.insert("kind", kind);
    map
}

pub async fn store(account: &str, kind: &str, value: &str) -> SecretResult<()> {
    let keyring = Keyring::new().await?;
    keyring
        .create_item(
            &format!("Metis {kind} ({account})"),
            &attributes(account, kind),
            Secret::text(value),
            true,
        )
        .await?;
    Ok(())
}

pub async fn get(account: &str, kind: &str) -> SecretResult<Option<String>> {
    let keyring = Keyring::new().await?;
    let items = keyring.search_items(&attributes(account, kind)).await?;
    match items.first() {
        Some(item) => {
            let secret = item.secret().await?;
            Ok(Some(
                String::from_utf8_lossy(secret.as_bytes()).into_owned(),
            ))
        }
        None => Ok(None),
    }
}

#[allow(dead_code)]
pub async fn delete(account: &str, kind: &str) -> SecretResult<()> {
    let keyring = Keyring::new().await?;
    keyring.delete(&attributes(account, kind)).await?;
    Ok(())
}
