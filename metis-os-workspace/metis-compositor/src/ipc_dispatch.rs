//! Pure IPC capability / lock-denylist decisions (Phase 15 §D / Phase 16 tests).

use metis_protocol::CompositorCommand;

/// Privilege scope for a compositor IPC request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcCaps {
    /// Bar / Settings / full session control plane.
    Full,
    /// Desktop-widgets helper: Launch + read-only / light reload only.
    Widgets,
}

/// Resolve capability from an optional widgets token.
///
/// - No token → [`IpcCaps::Full`] (trusted same-UID clients: bar, settings).
/// - Matching widgets token → [`IpcCaps::Widgets`].
/// - Present but wrong token → error.
pub fn resolve_ipc_caps(
    widgets_token: Option<&str>,
    presented: Option<&str>,
) -> Result<IpcCaps, String> {
    match presented {
        None => Ok(IpcCaps::Full),
        Some(t) => {
            if widgets_token.is_some_and(|expected| expected == t) {
                Ok(IpcCaps::Widgets)
            } else {
                Err("invalid or expired IPC capability token".into())
            }
        }
    }
}

pub fn widgets_command_allowed(cmd: &CompositorCommand) -> bool {
    matches!(
        cmd,
        CompositorCommand::Ping
            | CompositorCommand::GetMonitor
            | CompositorCommand::ListOutputs
            | CompositorCommand::ListWindows
            | CompositorCommand::GetLayout
            | CompositorCommand::Launch { .. }
            | CompositorCommand::ApplyBackground
            | CompositorCommand::ReloadLocale
    )
}

/// Commands refused while the session is locked (must not focus/reveal clients,
/// launch programs, touch the clipboard, or start capture).
pub fn command_denied_while_locked(cmd: &CompositorCommand) -> bool {
    use CompositorCommand as C;
    matches!(
        cmd,
        C::FocusWindow { .. }
            | C::ActivateWindow { .. }
            | C::SetFullscreen { .. }
            | C::SetMinimized { .. }
            | C::ToggleScratchpad
            | C::SetScratchpad { .. }
            | C::MoveWindow { .. }
            | C::MoveWindowToWorkspace { .. }
            | C::MoveWindowToOutput { .. }
            | C::MoveWorkspaceToOutput { .. }
            | C::SwitchWorkspace { .. }
            | C::SetWorkspaceLayout { .. }
            | C::SetDefaultLayout { .. }
            | C::ApplyLayout { .. }
            | C::SetTileMode { .. }
            | C::CloseWindow { .. }
            | C::Launch { .. }
            | C::EndSession
            | C::SetClipboard { .. }
            | C::BeginCaptureOverlay { .. }
            | C::BeginScreenshotOverlay
            | C::CaptureWindowThumbs { .. }
            | C::CaptureWorkspaceThumbs { .. }
            | C::InjectRemotePointerAbsolute { .. }
            | C::InjectRemotePointerRelative { .. }
            | C::InjectRemotePointerButton { .. }
            | C::InjectRemotePointerScroll { .. }
            | C::InjectRemoteKey { .. }
    )
}

/// Early-gate outcome before the full IPC match arm runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IpcGate {
    Allow,
    Reject(&'static str),
}

pub fn gate_ipc_command(cmd: &CompositorCommand, caps: IpcCaps, session_locked: bool) -> IpcGate {
    if caps == IpcCaps::Widgets && !widgets_command_allowed(cmd) {
        return IpcGate::Reject("IPC command not allowed for desktop-widgets capability");
    }
    if session_locked && command_denied_while_locked(cmd) {
        return IpcGate::Reject("session is locked");
    }
    IpcGate::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use metis_grid::{GridLayout, LayoutKind, PixelRect};
    use metis_protocol::TileMode;

    fn launch() -> CompositorCommand {
        CompositorCommand::Launch {
            argv: vec!["kitty".into()],
            program: String::new(),
        }
    }

    #[test]
    fn resolve_caps_none_full_match_widgets_wrong_errors() {
        assert_eq!(resolve_ipc_caps(Some("tok"), None).unwrap(), IpcCaps::Full);
        assert_eq!(
            resolve_ipc_caps(Some("tok"), Some("tok")).unwrap(),
            IpcCaps::Widgets
        );
        assert!(resolve_ipc_caps(Some("tok"), Some("nope")).is_err());
        assert!(resolve_ipc_caps(None, Some("tok")).is_err());
    }

    #[test]
    fn widgets_allowlist_matrix() {
        assert!(widgets_command_allowed(&CompositorCommand::Ping));
        assert!(widgets_command_allowed(&launch()));
        assert!(widgets_command_allowed(&CompositorCommand::ApplyBackground));
        assert!(!widgets_command_allowed(&CompositorCommand::EndSession));
        assert!(!widgets_command_allowed(&CompositorCommand::LockSession));
        assert!(!widgets_command_allowed(&CompositorCommand::CloseWindow {
            id: 1
        }));
        assert!(!widgets_command_allowed(
            &CompositorCommand::BeginScreenshotOverlay
        ));
    }

    #[test]
    fn lock_denylist_blocks_mutators_allows_reads() {
        assert!(command_denied_while_locked(&launch()));
        assert!(command_denied_while_locked(
            &CompositorCommand::FocusWindow { id: 1 }
        ));
        assert!(command_denied_while_locked(
            &CompositorCommand::SetClipboard {
                mime: "text/plain".into(),
                text: Some("x".into()),
                image_path: None,
            }
        ));
        assert!(!command_denied_while_locked(&CompositorCommand::Ping));
        assert!(!command_denied_while_locked(
            &CompositorCommand::ListOutputs
        ));
        assert!(!command_denied_while_locked(
            &CompositorCommand::LockSession
        ));
        assert!(!command_denied_while_locked(
            &CompositorCommand::ReloadLocale
        ));
    }

    #[test]
    fn gate_combines_caps_and_lock() {
        assert_eq!(
            gate_ipc_command(&CompositorCommand::Ping, IpcCaps::Widgets, true),
            IpcGate::Allow
        );
        assert!(matches!(
            gate_ipc_command(&CompositorCommand::EndSession, IpcCaps::Widgets, false),
            IpcGate::Reject(_)
        ));
        assert!(matches!(
            gate_ipc_command(&launch(), IpcCaps::Full, true),
            IpcGate::Reject("session is locked")
        ));
        assert_eq!(
            gate_ipc_command(
                &CompositorCommand::ApplyLayout {
                    layout: GridLayout::default(),
                    gutter_px: 0,
                },
                IpcCaps::Full,
                false
            ),
            IpcGate::Allow
        );
        assert!(matches!(
            gate_ipc_command(
                &CompositorCommand::MoveWindow {
                    id: 1,
                    rect: PixelRect {
                        x: 0,
                        y: 0,
                        width: 100,
                        height: 100,
                    },
                },
                IpcCaps::Full,
                true
            ),
            IpcGate::Reject("session is locked")
        ));
        let _ = (LayoutKind::Grid, TileMode::Grid);
    }
}
