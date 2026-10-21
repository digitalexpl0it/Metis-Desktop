//! Credential storage via the freedesktop Secret Service.
//!
//! DE-agnostic client (`oo7`): talks `org.freedesktop.secrets` on the session bus.
//! Metis’s default provider is `metis-secretsd`; third-party providers work if they
//! already own the bus.
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
/// Per-app key for `org.freedesktop.impl.portal.Secret` (hex-encoded 32 bytes).
pub const PORTAL_SECRET: &str = "portal_secret";
/// OpenSSH private key PEM (Phase 22 §C). `account` is a user-facing label.
pub const SSH_PRIVATE_KEY: &str = "ssh_private_key";
/// Extra attribute so unlock-time loaders can find SSH items without kind filter.
pub const SSH_KEY_ATTR: &str = "org.metis.ssh-key";

fn attributes<'a>(account: &'a str, kind: &'a str) -> HashMap<&'a str, &'a str> {
    let mut map = HashMap::new();
    map.insert("app", "metis");
    map.insert("account", account);
    map.insert("kind", kind);
    if kind == SSH_PRIVATE_KEY {
        map.insert(SSH_KEY_ATTR, "1");
    }
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

pub async fn delete(account: &str, kind: &str) -> SecretResult<()> {
    let keyring = Keyring::new().await?;
    keyring.delete(&attributes(account, kind)).await?;
    Ok(())
}

/// Labels of stored SSH private keys (`kind = ssh_private_key`).
pub async fn list_ssh_key_labels() -> SecretResult<Vec<String>> {
    let keyring = Keyring::new().await?;
    let mut attrs = HashMap::new();
    attrs.insert("app", "metis");
    attrs.insert("kind", SSH_PRIVATE_KEY);
    let items = keyring.search_items(&attrs).await?;
    let mut labels = Vec::new();
    for item in items {
        let attrs = item.attributes().await?;
        if let Some(account) = attrs.get("account") {
            labels.push(account.clone());
        }
    }
    labels.sort();
    labels.dedup();
    Ok(labels)
}

/// All SSH private key PEMs for `ssh-add` after vault unlock.
pub async fn list_ssh_key_secrets() -> SecretResult<Vec<(String, String)>> {
    let keyring = Keyring::new().await?;
    let mut attrs = HashMap::new();
    attrs.insert("app", "metis");
    attrs.insert("kind", SSH_PRIVATE_KEY);
    let items = keyring.search_items(&attrs).await?;
    let mut out = Vec::new();
    for item in items {
        let attrs = item.attributes().await?;
        let label = attrs
            .get("account")
            .cloned()
            .unwrap_or_else(|| "ssh-key".into());
        let secret = item.secret().await?;
        out.push((
            label,
            String::from_utf8_lossy(secret.as_bytes()).into_owned(),
        ));
    }
    Ok(out)
}
