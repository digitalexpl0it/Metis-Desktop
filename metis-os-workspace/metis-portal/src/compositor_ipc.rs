use metis_protocol::{CompositorCommand, CompositorEvent, OutputInfo};

pub fn portal_app_id(app_id: Option<ashpd::MaybeAppID>) -> Option<String> {
    app_id.map(|id| id.to_string())
}

/// Connected outputs from the compositor (primary first). Empty on IPC failure.
pub fn list_outputs() -> Vec<OutputInfo> {
    match metis_protocol::send_compositor_command(&CompositorCommand::ListOutputs) {
        Ok(CompositorEvent::OutputList { outputs }) => outputs,
        Ok(other) => {
            tracing::debug!(?other, "ListOutputs unexpected reply");
            Vec::new()
        }
        Err(err) => {
            tracing::debug!(%err, "ListOutputs IPC failed");
            Vec::new()
        }
    }
}

/// Ask the compositor to elevate capture UI. Returns `Err` when the compositor
/// explicitly rejects (e.g. session locked); transport failures are soft-logged
/// and treated as `Ok` so nested/dev sessions without a compositor still work.
pub fn begin_capture_overlay(app_id: Option<String>) -> Result<(), String> {
    let cmd = CompositorCommand::BeginCaptureOverlay { app_id };
    match metis_protocol::send_compositor_command(&cmd) {
        Ok(CompositorEvent::Pong) => Ok(()),
        Ok(CompositorEvent::Error { message }) => Err(message),
        Ok(_) => Ok(()),
        Err(err) => {
            tracing::debug!(%err, "BeginCaptureOverlay IPC failed");
            Ok(())
        }
    }
}

pub fn end_capture_overlay(app_id: Option<String>) {
    let cmd = CompositorCommand::EndCaptureOverlay { app_id };
    match metis_protocol::send_compositor_command(&cmd) {
        Ok(CompositorEvent::Pong) => {}
        Ok(CompositorEvent::Error { message }) => {
            tracing::warn!(%message, "EndCaptureOverlay rejected");
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(%err, "EndCaptureOverlay IPC failed"),
    }
}

/// Tell the compositor to hold an idle inhibitor for `cookie` (blocking IPC —
/// call from `spawn_blocking`). Failures are logged, not fatal: the worst case
/// is the screen still blanks under a running media app.
pub fn inhibit_idle(cookie: u32, app_name: Option<String>, reason: Option<String>) {
    let cmd = CompositorCommand::InhibitIdle {
        cookie,
        app_name,
        reason,
    };
    match metis_protocol::send_compositor_command(&cmd) {
        Ok(CompositorEvent::Pong) => {}
        Ok(CompositorEvent::Error { message }) => {
            tracing::warn!(%message, "InhibitIdle rejected");
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(%err, "InhibitIdle IPC failed"),
    }
}

/// Release the idle inhibitor previously taken for `cookie`.
pub fn uninhibit_idle(cookie: u32) {
    let cmd = CompositorCommand::UninhibitIdle { cookie };
    match metis_protocol::send_compositor_command(&cmd) {
        Ok(CompositorEvent::Pong) => {}
        Ok(CompositorEvent::Error { message }) => {
            tracing::warn!(%message, "UninhibitIdle rejected");
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(%err, "UninhibitIdle IPC failed"),
    }
}

pub fn set_clipboard(mime: &str, text: Option<&str>, image_path: Option<&str>) {
    let cmd = CompositorCommand::SetClipboard {
        mime: mime.to_string(),
        text: text.map(str::to_string),
        image_path: image_path.map(str::to_string),
    };
    match metis_protocol::send_compositor_command(&cmd) {
        Ok(CompositorEvent::Pong) => {}
        Ok(CompositorEvent::Error { message }) => {
            tracing::warn!(%message, "SetClipboard rejected");
        }
        Ok(_) => {}
        Err(err) => tracing::debug!(%err, "SetClipboard IPC failed"),
    }
}
