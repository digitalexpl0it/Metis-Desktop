use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::time::Duration;

use metis_protocol::{
    CompositorCommand, CompositorEvent, GridLayout, GridMetrics, MonitorRect, TileMode, WindowInfo,
};

#[allow(dead_code)] // compositor IPC helper
pub fn primary_monitor_rect() -> MonitorRect {
    match send_command(CompositorCommand::GetMonitor) {
        Ok(CompositorEvent::Monitor { rect }) => rect,
        _ => MonitorRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        },
    }
}

#[allow(dead_code)] // compositor IPC helper
pub fn get_layout() -> std::io::Result<(GridLayout, u32, GridMetrics)> {
    match send_command(CompositorCommand::GetLayout)? {
        CompositorEvent::LayoutChanged {
            layout,
            gutter_px,
            metrics,
        } => Ok((layout, gutter_px, metrics)),
        CompositorEvent::Error { message } => Err(std::io::Error::other(message)),
        _ => Err(std::io::Error::other(
            "unexpected compositor response for GetLayout",
        )),
    }
}

pub fn close_window(id: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::CloseWindow { id })?;
    Ok(())
}

#[allow(dead_code)] // compositor IPC helper
pub fn set_window_fullscreen(id: u32, enabled: bool) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::SetFullscreen { id, enabled })?;
    Ok(())
}

#[allow(dead_code)] // compositor IPC helper
pub fn focus_window(id: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::FocusWindow { id })?;
    Ok(())
}

/// Minimize or restore a window by id.
pub fn set_minimized(id: u32, minimized: bool) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::SetMinimized { id, minimized })?;
    Ok(())
}

/// Bring a window to the foreground (restore + raise + focus).
pub fn activate_window(id: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::ActivateWindow { id })?;
    Ok(())
}

/// Move a window to another virtual workspace (1-based).
pub fn move_window_to_workspace(window_id: u32, workspace: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::MoveWindowToWorkspace {
        window_id,
        workspace,
    })?;
    Ok(())
}

/// Switch the active virtual workspace (1-based) on `output` (output name, or
/// `None` to target the output under the pointer).
pub fn switch_workspace(output: Option<String>, id: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::SwitchWorkspace { output, id })?;
    Ok(())
}

#[allow(dead_code)] // compositor IPC helper
pub fn set_tile_mode(tile_id: &str, mode: TileMode) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::SetTileMode {
        tile_id: tile_id.to_string(),
        mode,
    })?;
    Ok(())
}

pub fn list_windows() -> std::io::Result<Vec<WindowInfo>> {
    match send_command(CompositorCommand::ListWindows)? {
        CompositorEvent::WindowList { windows } => Ok(windows),
        CompositorEvent::Error { message } => Err(std::io::Error::other(message)),
        _ => Err(std::io::Error::other("unexpected compositor response")),
    }
}

#[allow(dead_code)] // compositor IPC helper
pub fn apply_grid_layout(layout: GridLayout, gutter_px: u32) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::ApplyLayout { layout, gutter_px })?;
    Ok(())
}

pub fn launch_program(program: &str) -> std::io::Result<()> {
    launch_argv(metis_protocol::split_command_line(program))
}

/// Spawn via compositor IPC using an argv vector (no shell).
pub fn launch_argv(argv: impl IntoIterator<Item = impl Into<String>>) -> std::io::Result<()> {
    let argv: Vec<String> = argv.into_iter().map(Into::into).collect();
    if argv.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "empty launch argv",
        ));
    }
    let _ = send_command(CompositorCommand::Launch {
        argv,
        program: String::new(),
    })?;
    Ok(())
}

/// Ask the compositor to lock the session (enter the compositor-rendered lock
/// screen). Best-effort.
pub fn lock_session() -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::LockSession)?;
    Ok(())
}

/// Ask the compositor to re-read `wallpaper.json` and apply the background live.
pub fn apply_background() -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::ApplyBackground)?;
    Ok(())
}

pub fn reload_gaming_config() -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::ReloadGaming)?;
    Ok(())
}

/// Ask the compositor to end the Metis session (stops its event loop).
pub fn end_session() -> std::io::Result<()> {
    match send_command(CompositorCommand::EndSession) {
        Ok(_) => Ok(()),
        // The compositor may exit before the reply is flushed back to us.
        Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Ok(()),
        Err(e) => Err(e),
    }
}

pub fn set_clipboard(
    mime: String,
    text: Option<String>,
    image_path: Option<String>,
) -> std::io::Result<()> {
    let _ = send_command(CompositorCommand::SetClipboard {
        mime,
        text,
        image_path,
    })?;
    Ok(())
}

fn send_command(cmd: CompositorCommand) -> std::io::Result<CompositorEvent> {
    let path = metis_protocol::ipc_socket_path();
    let mut stream = UnixStream::connect(&path).map_err(|e| {
        std::io::Error::new(
            e.kind(),
            format!("Metis compositor not running at {}: {e}", path.display()),
        )
    })?;
    stream.set_read_timeout(Some(Duration::from_millis(400)))?;
    let mut payload = serde_json::to_value(&cmd).map_err(std::io::Error::other)?;
    if let Ok(token) = std::env::var("METIS_IPC_TOKEN")
        && !token.is_empty()
        && let Some(obj) = payload.as_object_mut()
    {
        obj.insert("token".into(), serde_json::Value::String(token));
    }
    let payload = serde_json::to_string(&payload).map_err(std::io::Error::other)?;
    writeln!(stream, "{payload}")?;
    stream.flush()?;
    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response)?;
    let line = response.trim();
    if line.is_empty() {
        return Err(std::io::Error::other("empty compositor response"));
    }
    serde_json::from_str(line).map_err(|e| std::io::Error::other(e.to_string()))
}

pub fn spawn_listener(handles: crate::state::StateHandles) {
    std::thread::Builder::new()
        .name("metis-compositor-events".into())
        .spawn(move || listen(handles))
        .expect("failed to spawn compositor event thread");
}

fn listen(handles: crate::state::StateHandles) {
    let events = handles.events.clone();

    if send_command(CompositorCommand::Ping).is_ok() {
        events.publish(crate::state::SystemEvent::CompositorConnected);
    }

    loop {
        match connect_events_socket() {
            Ok(stream) => {
                tracing::info!("subscribed to compositor event stream");
                read_events(stream, &events);
            }
            Err(err) => {
                tracing::debug!(%err, "event socket connect failed, retrying");
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

fn connect_events_socket() -> std::io::Result<UnixStream> {
    let path = metis_protocol::events_socket_path();
    let stream = UnixStream::connect(&path)?;
    stream.set_read_timeout(None)?;
    Ok(stream)
}

fn read_events(stream: UnixStream, events: &crate::state::EventPublisher) {
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let Ok(line) = line else {
            tracing::warn!("compositor event stream disconnected");
            break;
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<CompositorEvent>(&line) {
            Ok(evt) => {
                events.publish(crate::state::SystemEvent::Compositor(evt));
            }
            Err(err) => {
                tracing::warn!(%err, line = line.as_str(), "failed to parse compositor event");
            }
        }
    }
}
