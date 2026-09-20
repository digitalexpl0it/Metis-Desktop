//! D-Bus types and Authority proxy for org.freedesktop.PolicyKit1.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use zbus::zvariant::{OwnedValue, Type};

#[derive(Debug, Clone, Deserialize, Serialize, Type)]
pub struct Identity {
    pub kind: String,
    pub details: HashMap<String, OwnedValue>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Type)]
pub struct Subject {
    pub kind: String,
    pub details: HashMap<String, OwnedValue>,
}

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
pub trait Authority {
    fn register_authentication_agent(
        &self,
        subject: &Subject,
        locale: &str,
        object_path: &str,
    ) -> zbus::Result<()>;

    fn unregister_authentication_agent(
        &self,
        subject: &Subject,
        object_path: &str,
    ) -> zbus::Result<()>;
}

/// Polkit D-Bus error domain used when BeginAuthentication fails/cancels.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.PolicyKit1.Error")]
pub enum PolkitError {
    #[zbus(error)]
    ZBus(zbus::Error),
    Failed(String),
    Cancelled(String),
    NotSupported(String),
    NotAuthorized(String),
    CancellationIdNotUnique(String),
}
