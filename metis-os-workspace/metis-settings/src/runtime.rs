//! Send a one-shot runtime command to the running Metis shell so it re-applies
//! config we just wrote. Mirrors `scripts/metis-cmd.sh` — the shell polls the
//! command file every 100ms and removes it after handling.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use metis_protocol::{CompositorCommand, CompositorEvent};

/// Query DRM (or current) video modes for one output.
pub fn list_output_modes(
    output: &str,
) -> (
    Vec<metis_protocol::OutputModeInfo>,
    Option<metis_protocol::OutputModeInfo>,
) {
    match send_command(CompositorCommand::ListOutputModes {
        output: output.to_string(),
    }) {
        Ok(CompositorEvent::OutputModes { modes, current }) => (modes, current),
        Ok(_) => (Vec::new(), None),
        Err(err) => {
            tracing::warn!(%err, output, "failed to list output modes via compositor IPC");
            (Vec::new(), None)
        }
    }
}

pub fn send(cmd: &str) {
    if let Err(err) = metis_protocol::write_runtime_command(cmd) {
        tracing::warn!(%err, cmd, "failed to write runtime command");
    }
}

/// Send a one-shot command to the desktop-widgets process (not the edge bar).
pub fn send_widgets(cmd: &str) {
    if let Err(err) = metis_protocol::write_runtime_command_widgets(cmd) {
        tracing::warn!(%err, cmd, "failed to write widgets runtime command");
    }
}

/// Ask the compositor to re-read `wallpaper.json` and apply the background live
/// (picture, solid colour, or gradient). Best-effort.
pub fn apply_background() {
    if let Err(err) = send_command(CompositorCommand::ApplyBackground) {
        tracing::warn!(%err, "failed to apply background via compositor IPC");
    }
}

/// Ask the compositor to re-read `input.json` and apply pointer/keyboard settings
/// immediately. Best-effort.
pub fn reload_input() {
    if let Err(err) = send_command(CompositorCommand::ReloadInput) {
        tracing::warn!(%err, "failed to reload input via compositor IPC");
    }
}

/// Re-read `keybinds.json` and apply desktop shortcuts live.
pub fn reload_keybinds() {
    if let Err(err) = send_command(CompositorCommand::ReloadKeybinds) {
        tracing::warn!(%err, "failed to reload keybinds via compositor IPC");
    }
}

pub fn reload_keybinds_async() {
    std::thread::spawn(reload_keybinds);
}

/// Re-read `decorations.json` and re-apply per-app titlebar overrides live.
pub fn reload_decorations_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::ReloadDecorations) {
            tracing::warn!(%err, "failed to reload decorations via compositor IPC");
        }
    });
}

/// Suppress / resume global shortcut dispatch while Settings captures a chord.
pub fn set_keybind_capture(active: bool) {
    if let Err(err) = send_command(CompositorCommand::SetKeybindCapture { active }) {
        tracing::debug!(%err, active, "failed to set keybind capture via compositor IPC");
    }
}

pub fn set_keybind_capture_async(active: bool) {
    std::thread::spawn(move || set_keybind_capture(active));
}

/// Re-read `outputs.json` and apply per-output scale immediately. Best-effort.
pub fn reload_outputs() {
    if let Err(err) = send_command(CompositorCommand::ReloadOutputs) {
        tracing::warn!(%err, "failed to reload outputs via compositor IPC");
    }
}

/// Like [`reload_outputs`], but never blocks the GTK main thread (used for live
/// toggles such as night light where `bluetoothctl`-class latency is unacceptable).
pub fn reload_outputs_async() {
    std::thread::spawn(reload_outputs);
}

/// Re-read `power.json` and apply idle preferences (screen blank timeout) live.
/// Best-effort; runs off the GTK main thread so a slow/absent compositor never
/// stalls the settings UI.
pub fn reload_power_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::ReloadPower) {
            tracing::debug!(%err, "failed to reload power via compositor IPC");
        }
    });
}

/// Re-read `lock.json` and re-decode the lock-screen background live. Best-effort;
/// runs off the GTK main thread so a slow/absent compositor never stalls the UI.
pub fn reload_lock_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::ReloadLock) {
            tracing::debug!(%err, "failed to reload lock config via compositor IPC");
        }
    });
}

/// Re-read `locale.json` and ask the compositor to refresh Fluent lock/SSD strings.
pub fn reload_locale_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::ReloadLocale) {
            tracing::debug!(%err, "failed to reload locale via compositor IPC");
        }
    });
}

/// Re-read `gaming.json` and apply graphics/offload preferences live.
pub fn reload_gaming_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::ReloadGaming) {
            tracing::debug!(%err, "failed to reload gaming config via compositor IPC");
        }
        send("reload-gaming");
    });
}

/// Lock the session now (used by the Settings "Lock now" affordance and shell
/// menu). Best-effort; never blocks the GTK main thread.
pub fn lock_session_async() {
    std::thread::spawn(|| {
        if let Err(err) = send_command(CompositorCommand::LockSession) {
            tracing::debug!(%err, "failed to lock session via compositor IPC");
        }
    });
}

/// Apply a layout mode (grid vs. scrolling) to every workspace on every output
/// immediately, so changing the "New workspace layout" default acts as a live
/// global on/off rather than only affecting future workspaces. Best-effort.
pub fn apply_default_layout(kind: metis_protocol::LayoutKind) {
    if let Err(err) = send_command(CompositorCommand::SetDefaultLayout { kind }) {
        tracing::warn!(%err, "failed to apply default layout via compositor IPC");
    }
}

/// Query the compositor for the connected outputs (name + geometry), primary
/// first. Returns an empty list if the compositor is unreachable, so callers can
/// degrade to a single global background.
pub fn list_outputs() -> Vec<metis_protocol::OutputInfo> {
    match send_command(CompositorCommand::ListOutputs) {
        Ok(CompositorEvent::OutputList { outputs }) => outputs,
        Ok(_) => Vec::new(),
        Err(err) => {
            tracing::warn!(%err, "failed to list outputs via compositor IPC");
            Vec::new()
        }
    }
}

/// Raise / unminimize the running Settings window via compositor ActivateWindow.
/// Wayland clients cannot unminimize themselves; GTK `present()` alone is not enough.
pub fn activate_settings_window() {
    std::thread::spawn(|| {
        let windows = match send_command(CompositorCommand::ListWindows) {
            Ok(CompositorEvent::WindowList { windows }) => windows,
            Ok(_) => return,
            Err(err) => {
                tracing::debug!(%err, "failed to list windows for settings activate");
                return;
            }
        };
        let Some(id) = windows.into_iter().find_map(|w| {
            let app_id = w.app_id.as_deref()?.trim().to_ascii_lowercase();
            if app_id == "com.metis.settings" || app_id == "metis-settings" {
                Some(w.id)
            } else {
                None
            }
        }) else {
            return;
        };
        if let Err(err) = send_command(CompositorCommand::ActivateWindow { id }) {
            tracing::debug!(%err, id, "failed to activate settings window");
        }
    });
}

fn send_command(cmd: CompositorCommand) -> std::io::Result<CompositorEvent> {
    let path = metis_protocol::ipc_socket_path();
    // Connect on a helper thread so a wedged compositor cannot block the GTK
    // main loop indefinitely (UnixStream::connect has no timeout).
    let path_for_connect = path.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(UnixStream::connect(&path_for_connect));
    });
    let mut stream = match rx.recv_timeout(Duration::from_millis(600)) {
        Ok(Ok(stream)) => stream,
        Ok(Err(err)) => return Err(err),
        Err(_) => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "compositor IPC connect timed out",
            ));
        }
    };
    stream.set_read_timeout(Some(Duration::from_millis(600)))?;
    stream.set_write_timeout(Some(Duration::from_millis(600)))?;
    let payload = serde_json::to_string(&cmd).map_err(std::io::Error::other)?;
    writeln!(stream, "{payload}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    serde_json::from_str(response.trim()).map_err(|e| std::io::Error::other(e.to_string()))
}
