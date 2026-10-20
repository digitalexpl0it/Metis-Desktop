//! `org.freedesktop.impl.portal.Secret` — per-app encrypt key for sandboxed apps.
//!
//! Stores a stable 32-byte key (hex-encoded) in the session Secret Service
//! under kind `portal_secret`. Fail closed when the vault is locked.

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::AsRawFd;

use ashpd::{
    MaybeAppID, PortalError,
    backend::{request::RequestImpl, secret::SecretImpl},
    desktop::HandleToken,
};
use async_trait::async_trait;
use rand::RngCore;
use zbus::zvariant::OwnedValue;

use metis_secrets::PORTAL_SECRET as KIND;

pub struct MetisSecret;

#[async_trait]
impl RequestImpl for MetisSecret {
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl SecretImpl for MetisSecret {
    async fn retrieve(
        &self,
        _token: HandleToken,
        app_id: MaybeAppID,
        fd: std::os::fd::OwnedFd,
    ) -> Result<HashMap<String, OwnedValue>, PortalError> {
        let account = {
            let s = app_id.to_string();
            if s.is_empty() { "unknown".into() } else { s }
        };

        let key_bytes = match metis_secrets::get(&account, KIND).await {
            Ok(Some(existing)) => decode_hex_key(&existing)
                .ok_or_else(|| PortalError::Failed("stored portal secret is corrupt".into()))?,
            Ok(None) => {
                let mut bytes = [0u8; 32];
                rand::thread_rng().fill_bytes(&mut bytes);
                let encoded = to_hex(&bytes);
                metis_secrets::store(&account, KIND, &encoded)
                    .await
                    .map_err(|err| {
                        tracing::warn!(%err, %account, "portal Secret: store failed");
                        PortalError::Failed(format!("secret store unavailable: {err}"))
                    })?;
                bytes.to_vec()
            }
            Err(err) => {
                tracing::warn!(%err, %account, "portal Secret: get failed");
                return Err(PortalError::Failed(format!(
                    "secret service unavailable: {err}"
                )));
            }
        };

        let mut file = std::fs::File::from(fd);
        file.write_all(&key_bytes).map_err(|err| {
            PortalError::Failed(format!("write secret fd ({}): {err}", file.as_raw_fd()))
        })?;
        Ok(HashMap::new())
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn decode_hex_key(s: &str) -> Option<Vec<u8>> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    (0..32)
        .map(|i| u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok())
        .collect()
}
