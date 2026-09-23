use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use ashpd::{
    MaybeAppID, PortalError, WindowIdentifierType,
    backend::{request::RequestImpl, screenshot::ScreenshotImpl},
    desktop::{
        Color, HandleToken,
        screenshot::{AvailableTargets, ColorOptions, Screenshot, ScreenshotOptions},
    },
};
use async_trait::async_trait;
use enumflags2::BitFlags;

use crate::capture::CaptureHub;
use crate::compositor_ipc;

static SCREENSHOT_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

struct ScreenshotGuard;

impl Drop for ScreenshotGuard {
    fn drop(&mut self) {
        SCREENSHOT_IN_FLIGHT.store(false, Ordering::Release);
    }
}

pub struct MetisScreenshot {
    capture: Arc<CaptureHub>,
}

impl MetisScreenshot {
    pub fn new(capture: Arc<CaptureHub>) -> Self {
        Self { capture }
    }
}

#[async_trait]
impl RequestImpl for MetisScreenshot {
    async fn close(&self, _token: HandleToken) {}
}

#[async_trait]
impl ScreenshotImpl for MetisScreenshot {
    /// `screenshot_png` always captures the full screen; advertise only that.
    fn available_targets(&self) -> BitFlags<AvailableTargets> {
        BitFlags::from(AvailableTargets::Screen)
    }

    async fn screenshot(
        &self,
        _token: HandleToken,
        app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        _options: ScreenshotOptions,
    ) -> ashpd::backend::Result<Screenshot> {
        if SCREENSHOT_IN_FLIGHT
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            tracing::warn!(
                ?app_id,
                "portal screenshot rejected — capture already in progress"
            );
            return Err(PortalError::Failed(
                "Another screenshot capture is already in progress".into(),
            ));
        }
        let _guard = ScreenshotGuard;

        tracing::info!(?app_id, "portal screenshot request");
        let portal_app = compositor_ipc::portal_app_id(app_id);
        if let Err(message) = compositor_ipc::begin_capture_overlay(portal_app.clone()) {
            tracing::warn!(%message, "BeginCaptureOverlay rejected");
            if message.contains("session is locked") {
                return Err(PortalError::Failed(message));
            }
        }
        let result = self.capture.screenshot_png().await;
        if result.is_err() {
            compositor_ipc::end_capture_overlay(portal_app);
        }
        // On success the compositor clears the elevate session once the picker maps,
        // or after a short timeout if capture UI never appears.
        let path = result?;
        let uri = format!("file://{}", path.display())
            .parse()
            .map_err(|err| PortalError::Failed(format!("invalid screenshot uri: {err}")))?;
        Ok(Screenshot::new(uri))
    }

    async fn pick_color(
        &self,
        _token: HandleToken,
        _app_id: Option<MaybeAppID>,
        _window_identifier: Option<WindowIdentifierType>,
        _options: ColorOptions,
    ) -> ashpd::backend::Result<Color> {
        Err(PortalError::Failed(
            "PickColor is not implemented in the Metis portal yet".into(),
        ))
    }
}
