//! Metis gaming daemon — event-driven power/GameMode hooks (no polling).

use std::sync::mpsc;
use std::time::Duration;

use metis_gaming::session::{GamingDaemon, spawn_event_listener};
use metis_protocol::CompositorEvent;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_gaming=info,warn".into()),
        )
        .init();

    let _ = metis_config::save_default_gaming_config();

    let (tx, rx) = mpsc::channel::<CompositorEvent>();
    spawn_event_listener(tx);

    tracing::info!("metis-gamingd started");

    let mut daemon = GamingDaemon::new();
    loop {
        match rx.recv_timeout(Duration::from_secs(30)) {
            Ok(evt) => daemon.handle_event(evt),
            Err(mpsc::RecvTimeoutError::Timeout) => check_runtime_command(),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
}

fn check_runtime_command() {
    let path = metis_protocol::runtime_command_path();
    let Ok(cmd) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(parsed) =
        metis_protocol::parse_runtime_command(cmd.trim(), metis_protocol::BAR_RUNTIME_VERBS)
    else {
        return;
    };
    match parsed.verb {
        "reload-gaming" => tracing::info!("gamingd: reload-gaming"),
        "optimize-gaming" => {
            if parsed.arg == "yes" || std::env::var_os("METIS_GAMING_OPTIMIZE_YES").is_some() {
                match metis_gaming::optimize_flatpak_gaming() {
                    Ok(r) => tracing::info!(?r, "gamingd: flatpak optimize done"),
                    Err(err) => tracing::warn!(%err, "gamingd: flatpak optimize failed"),
                }
            } else {
                tracing::warn!(
                    "gamingd: optimize-gaming ignored without confirmation (need `yes`)"
                );
            }
        }
        _ => {}
    }
    let _ = std::fs::remove_file(&path);
}
