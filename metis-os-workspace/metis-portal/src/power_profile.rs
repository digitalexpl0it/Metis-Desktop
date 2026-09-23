//! `org.freedesktop.impl.portal.PowerProfileMonitor` backend.
//!
//! Sandbox-friendly apps read `power-saver-enabled` via GIO's PowerProfileMonitor.
//! Metis mirrors the active profile from `powerprofilesctl`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use zbus::{Connection, interface};

const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

struct PowerProfileIface {
    power_saver: Arc<AtomicBool>,
}

#[interface(name = "org.freedesktop.impl.portal.PowerProfileMonitor")]
impl PowerProfileIface {
    #[zbus(property(emits_changed_signal = "false"))]
    fn version(&self) -> u32 {
        1
    }

    #[zbus(property)]
    fn power_saver_enabled(&self) -> bool {
        self.power_saver.load(Ordering::Relaxed)
    }
}

fn read_power_saver_enabled() -> bool {
    let output = std::process::Command::new("powerprofilesctl")
        .arg("get")
        .output();
    match output {
        Ok(out) if out.status.success() => {
            String::from_utf8_lossy(&out.stdout).trim() == "power-saver"
        }
        _ => false,
    }
}

/// Register PowerProfileMonitor on the portal impl connection and start polling.
pub async fn serve(connection: &Connection) -> zbus::Result<()> {
    let power_saver = Arc::new(AtomicBool::new(read_power_saver_enabled()));
    connection
        .object_server()
        .at(
            DESKTOP_PATH,
            PowerProfileIface {
                power_saver: Arc::clone(&power_saver),
            },
        )
        .await?;

    spawn_poll(connection.clone(), power_saver);
    tracing::info!("PowerProfileMonitor portal registered");
    Ok(())
}

fn spawn_poll(connection: Connection, power_saver: Arc<AtomicBool>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;
            let enabled = read_power_saver_enabled();
            let prev = power_saver.swap(enabled, Ordering::Relaxed);
            if prev == enabled {
                continue;
            }
            let object_server = connection.object_server();
            let Ok(iface_ref) = object_server
                .interface::<_, PowerProfileIface>(DESKTOP_PATH)
                .await
            else {
                continue;
            };
            let inner = iface_ref.get().await;
            if let Err(err) =
                PowerProfileIface::power_saver_enabled_changed(&inner, iface_ref.signal_emitter())
                    .await
            {
                tracing::debug!(%err, "PowerProfileMonitor: property notify failed");
            }
        }
    });
}
