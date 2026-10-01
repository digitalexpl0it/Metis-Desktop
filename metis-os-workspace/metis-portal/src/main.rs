mod background;
mod capture;
mod compositor_events;
mod compositor_ipc;
mod compositor_remote_input;
mod consent;
mod mutter;
mod pipewire;
mod power_profile;
mod screencast;
mod screensaver;
mod screenshot;
mod settings;

use std::io::{self, Write};
use std::sync::Arc;

use background::MetisBackground;
use screencast::MetisScreencast;
use screenshot::MetisScreenshot;
use settings::MetisSettings;

const DBUS_NAME: &str = "org.freedesktop.impl.portal.desktop.metis";

/// Stderr is fully buffered when redirected to `portal.log` by the compositor;
/// flush after every write so `tail -f` shows lines immediately.
struct FlushStderr;

impl Write for FlushStderr {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = io::stderr().write(buf)?;
        io::stderr().flush()?;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        io::stderr().flush()
    }
}

fn init_portal_logging() {
    tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(|| FlushStderr)
        .init();
}

#[tokio::main]
async fn main() -> ashpd::Result<()> {
    init_portal_logging();

    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--help" | "-h") => {
            eprintln!(
                "Usage: metis-portal [--capture-test [PATH]]\n\
                 Metis xdg-desktop-portal backend (Settings, ScreenCast, Mutter RDP shim)."
            );
            return Ok(());
        }
        Some("--version" | "-V") => {
            eprintln!("metis-portal {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some("--capture-test") => {
            let path = args
                .next()
                .unwrap_or_else(|| "/tmp/metis-capture-test.png".into());
            tracing::info!(%path, "capture test");
            let captured = capture::capture_fullscreen_png()
                .await
                .map_err(ashpd::PortalError::Failed)?;
            std::fs::copy(&captured.path, &path).map_err(|err| {
                ashpd::PortalError::Failed(format!("copy screenshot to {path}: {err}"))
            })?;
            tracing::info!(%path, "capture test ok");
            return Ok(());
        }
        _ => return run_portal().await,
    }
}

async fn run_portal() -> ashpd::Result<()> {
    tracing::info!(
        "starting Metis portal backend (Settings, Screenshot, ScreenCast, Background, PowerProfile)"
    );

    // Push gsettings before any client queries Settings — Nautilus / libadwaita
    // fall back to color-scheme when the portal answer is late.
    metis_config::sync_session_appearance_from_config();

    let pipewire = Arc::new(pipewire::PipeWireHub::start()?);
    let capture = Arc::new(capture::CaptureHub::new(Arc::clone(&pipewire)));

    let connection = zbus::Connection::session()
        .await
        .map_err(|err| ashpd::PortalError::Failed(format!("session bus connection: {err}")))?;

    // ashpd::Builder::build_with_connection claims the portal name then awaits
    // `connection.closed()` forever. Register Mutter / PowerProfile / screensaver
    // *before* that so gnome-remote-desktop can see RemoteDesktop + ScreenCast.
    if let Err(err) = power_profile::serve(&connection).await {
        tracing::warn!(%err, "PowerProfileMonitor portal unavailable");
    }

    if let Err(err) = mutter::serve(&connection, Arc::clone(&pipewire), Arc::clone(&capture)).await
    {
        tracing::warn!(
            %err,
            "Mutter RemoteDesktop/ScreenCast shim unavailable — gnome-remote-desktop RDP will not bind"
        );
    }

    // Own the legacy idle-inhibit D-Bus names (games/media keep the screen awake
    // through these). Kept alive for the whole session; a startup failure is
    // non-fatal (Wayland `zwp_idle_inhibit` still works).
    let _screensaver = match screensaver::serve().await {
        Ok(conn) => Some(conn),
        Err(err) => {
            tracing::warn!(%err, "screensaver: idle-inhibit service unavailable");
            None
        }
    };

    tracing::info!("portal backends registered; entering ashpd serve loop");
    ashpd::backend::Builder::new(DBUS_NAME)?
        .settings(MetisSettings)
        .screenshot(MetisScreenshot::new(Arc::clone(&capture)))
        .screencast(MetisScreencast::new(
            Arc::clone(&capture),
            Arc::clone(&pipewire),
        ))
        .background(MetisBackground)
        .build_with_connection(connection)
        .await?;

    Ok(())
}
