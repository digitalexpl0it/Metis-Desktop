//! Legacy idle-inhibit D-Bus services.
//!
//! Most desktop apps that want to keep the screen awake — Chromium/Electron,
//! Firefox, SDL games, VLC/mpv — do **not** speak the Wayland
//! `zwp_idle_inhibit` protocol; they call the well-known
//! `org.freedesktop.ScreenSaver` (and, less commonly,
//! `org.freedesktop.PowerManagement.Inhibit`) D-Bus interfaces. Nothing in a
//! bare Metis session owns those names, so those requests are silently dropped
//! and the screen blanks mid-video.
//!
//! This module owns both names and forwards every `Inhibit`/`UnInhibit` to the
//! compositor over IPC, where they join the Wayland inhibitors in a single count
//! that gates the idle blanker. Cookies are allocated here and tracked per
//! D-Bus peer so a crashing client cannot leave the screen awake forever — when
//! a peer drops off the bus we release everything it held.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use zbus::{self, Connection, interface, message::Header};

use crate::compositor_ipc;

/// Shared inhibitor bookkeeping across both D-Bus interfaces. Cookies are unique
/// across the whole process (the compositor keeps a single cookie space).
#[derive(Clone)]
pub struct InhibitService {
    next_cookie: Arc<AtomicU32>,
    /// cookie → owning peer unique name, for dead-peer cleanup.
    owners: Arc<Mutex<HashMap<u32, String>>>,
}

impl InhibitService {
    fn new() -> Self {
        Self {
            next_cookie: Arc::new(AtomicU32::new(1)),
            owners: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Allocate a fresh non-zero cookie and forward the inhibit to the
    /// compositor off-thread (the IPC call is blocking).
    fn engage(&self, owner: Option<String>, app_name: String, reason: String) -> u32 {
        let mut cookie = self.next_cookie.fetch_add(1, Ordering::Relaxed);
        if cookie == 0 {
            cookie = self.next_cookie.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(owner) = owner {
            self.owners.lock().unwrap().insert(cookie, owner);
        }
        let app = (!app_name.is_empty()).then_some(app_name);
        let why = (!reason.is_empty()).then_some(reason);
        tokio::task::spawn_blocking(move || compositor_ipc::inhibit_idle(cookie, app, why));
        cookie
    }

    fn release(&self, cookie: u32) {
        self.owners.lock().unwrap().remove(&cookie);
        tokio::task::spawn_blocking(move || compositor_ipc::uninhibit_idle(cookie));
    }

    fn has_any(&self) -> bool {
        !self.owners.lock().unwrap().is_empty()
    }

    /// Release every cookie held by a peer that just dropped off the bus.
    fn release_peer(&self, owner: &str) {
        let cookies = {
            let mut guard = self.owners.lock().unwrap();
            take_peer_cookies(&mut guard, owner)
        };
        if cookies.is_empty() {
            return;
        }
        tracing::info!(peer = %owner, count = cookies.len(), "screensaver: peer gone, releasing inhibitors");
        for cookie in cookies {
            tokio::task::spawn_blocking(move || compositor_ipc::uninhibit_idle(cookie));
        }
    }
}

/// Remove and return every cookie owned by `owner` (pure map surgery for tests).
fn take_peer_cookies(owners: &mut HashMap<u32, String>, owner: &str) -> Vec<u32> {
    let cookies: Vec<u32> = owners
        .iter()
        .filter(|(_, o)| o.as_str() == owner)
        .map(|(c, _)| *c)
        .collect();
    for cookie in &cookies {
        owners.remove(cookie);
    }
    cookies
}

/// `NameOwnerChanged` reclaim predicate: unique bus names with an empty
/// `new_owner` mean the peer disconnected without `UnInhibit`.
fn should_reclaim_peer(name: &str, new_owner_absent: bool) -> bool {
    name.starts_with(':') && new_owner_absent
}

fn sender_of(header: &Header<'_>) -> Option<String> {
    header.sender().map(|s| s.to_string())
}

/// `org.freedesktop.ScreenSaver` — the interface browsers, Electron, SDL, and
/// media players use.
struct ScreenSaverIface {
    svc: InhibitService,
}

#[interface(name = "org.freedesktop.ScreenSaver")]
impl ScreenSaverIface {
    async fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        application_name: String,
        reason_for_inhibit: String,
    ) -> u32 {
        self.svc
            .engage(sender_of(&header), application_name, reason_for_inhibit)
    }

    async fn un_inhibit(&self, cookie: u32) {
        self.svc.release(cookie);
    }

    async fn get_active(&self) -> bool {
        false
    }

    async fn set_active(&self, _active: bool) -> bool {
        false
    }

    async fn get_active_time(&self) -> u32 {
        0
    }

    async fn simulate_user_activity(&self) {}

    async fn lock(&self) {}
}

/// `org.freedesktop.PowerManagement.Inhibit` — the older GNOME power interface a
/// few apps still use to block blank/suspend.
struct PowerInhibitIface {
    svc: InhibitService,
}

#[interface(name = "org.freedesktop.PowerManagement.Inhibit")]
impl PowerInhibitIface {
    async fn inhibit(
        &self,
        #[zbus(header)] header: Header<'_>,
        application: String,
        reason: String,
    ) -> u32 {
        self.svc.engage(sender_of(&header), application, reason)
    }

    async fn un_inhibit(&self, cookie: u32) {
        self.svc.release(cookie);
    }

    async fn has_inhibit(&self) -> bool {
        self.svc.has_any()
    }
}

/// Bring the idle-inhibit D-Bus services up on the session bus. Returns the
/// connection, which the caller must keep alive for the session's lifetime.
/// Name-ownership failures are logged, not fatal — a co-installed screensaver
/// service simply keeps the name and Metis falls back to Wayland inhibitors.
pub async fn serve() -> zbus::Result<Connection> {
    let svc = InhibitService::new();

    let conn = zbus::connection::Builder::session()?
        .serve_at(
            "/org/freedesktop/ScreenSaver",
            ScreenSaverIface { svc: svc.clone() },
        )?
        .serve_at("/ScreenSaver", ScreenSaverIface { svc: svc.clone() })?
        .serve_at(
            "/org/freedesktop/PowerManagement/Inhibit",
            PowerInhibitIface { svc: svc.clone() },
        )?
        .build()
        .await?;

    for name in [
        "org.freedesktop.ScreenSaver",
        "org.freedesktop.PowerManagement.Inhibit",
    ] {
        match conn.request_name(name).await {
            Ok(()) => tracing::info!(%name, "screensaver: owning D-Bus name"),
            Err(err) => {
                tracing::warn!(%name, %err, "screensaver: could not own D-Bus name (already taken?)")
            }
        }
    }

    spawn_peer_watch(conn.clone(), svc);
    Ok(conn)
}

/// Watch `NameOwnerChanged` so a peer that disconnects without calling
/// `UnInhibit` (e.g. a crashed browser) has its inhibitors reclaimed.
fn spawn_peer_watch(conn: Connection, svc: InhibitService) {
    tokio::spawn(async move {
        let proxy = match zbus::fdo::DBusProxy::new(&conn).await {
            Ok(proxy) => proxy,
            Err(err) => {
                tracing::warn!(%err, "screensaver: NameOwnerChanged watch unavailable");
                return;
            }
        };
        let mut stream = match proxy.receive_name_owner_changed().await {
            Ok(stream) => stream,
            Err(err) => {
                tracing::warn!(%err, "screensaver: NameOwnerChanged subscribe failed");
                return;
            }
        };
        while let Some(signal) = stream.next().await {
            let Ok(args) = signal.args() else {
                continue;
            };
            let name = args.name().to_string();
            // Only unique names (":1.42") represent a single peer; a lost owner
            // has an empty `new_owner`.
            if should_reclaim_peer(&name, args.new_owner().is_none()) {
                svc.release_peer(&name);
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_peer_cookies_reclaims_only_that_peer() {
        let mut owners = HashMap::from([
            (1, ":1.10".into()),
            (2, ":1.20".into()),
            (3, ":1.10".into()),
            (4, ":1.30".into()),
        ]);
        let mut gone = take_peer_cookies(&mut owners, ":1.10");
        gone.sort_unstable();
        assert_eq!(gone, vec![1, 3]);
        assert_eq!(owners.len(), 2);
        assert_eq!(owners.get(&2).map(String::as_str), Some(":1.20"));
        assert_eq!(owners.get(&4).map(String::as_str), Some(":1.30"));
        assert!(take_peer_cookies(&mut owners, ":1.10").is_empty());
    }

    #[test]
    fn should_reclaim_only_unique_names_with_empty_new_owner() {
        assert!(should_reclaim_peer(":1.42", true));
        assert!(!should_reclaim_peer(":1.42", false));
        assert!(!should_reclaim_peer("org.freedesktop.ScreenSaver", true));
        assert!(!should_reclaim_peer("", true));
    }
}
