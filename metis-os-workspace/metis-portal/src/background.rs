//! Stub `org.freedesktop.impl.portal.Background` backend.
//!
//! Flatpak apps (media players, launchers) may request background execution or
//! autostart. Metis does not manage autostart today; we allow background runs
//! so sandboxed clients get a successful response instead of a missing backend.

use std::collections::HashMap;
use std::sync::Arc;

use ashpd::{
    MaybeAppID, PortalError,
    backend::{
        background::{
            Activity, AppState, AutoStartFlags, Background, BackgroundImpl, BackgroundSignalEmitter,
        },
        request::RequestImpl,
    },
    desktop::HandleToken,
};
use async_trait::async_trait;
use enumflags2::BitFlags;

pub struct MetisBackground;

#[async_trait]
impl RequestImpl for MetisBackground {
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl BackgroundImpl for MetisBackground {
    async fn get_app_state(&self) -> Result<HashMap<MaybeAppID, AppState>, PortalError> {
        Ok(HashMap::new())
    }

    async fn notify_background(
        &self,
        _token: HandleToken,
        _app_id: MaybeAppID,
        _name: &str,
    ) -> Result<Background, PortalError> {
        Ok(Background::new(Activity::Allow))
    }

    async fn enable_autostart(
        &self,
        _app_id: MaybeAppID,
        _enable: bool,
        _commandline: Vec<String>,
        _flags: BitFlags<AutoStartFlags>,
    ) -> Result<bool, PortalError> {
        Ok(false)
    }

    fn set_signal_emitter(&mut self, _signal_emitter: Arc<dyn BackgroundSignalEmitter>) {}
}
