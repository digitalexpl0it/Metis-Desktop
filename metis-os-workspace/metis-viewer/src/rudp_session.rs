//! Metis Remote (RUDP) session windows — decode + present + input.
//!
//! One Quinn session can carry several monitor streams (protocol v3). Each
//! stream gets its own GTK window ([`StreamWindow`]); [`SessionUi`] owns the
//! shared bits (layout, subscription, clipboard, audio, pointer lock).

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::net::SocketAddr;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};

use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use metis_config::ViewerPlacement;
use metis_decode::{RudpCodec, open_decoder};
use metis_i18n::tr;
use metis_protocol::{
    RUDP_MAX_SUBSCRIBED_OUTPUTS, RudpControlMsg, RudpMonitorInfo, truncate_clipboard_text,
};
use metis_rudp_client::{
    ClientError, RudpClientConfig, RudpSession, SessionEvent, TofuMode, clear_host_pin, connect,
    pin_host, resolve_host_port,
};
use zeroize::Zeroize;

/// Which host monitors a Viewer session streams (one window per stream).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum MonitorChoice {
    /// Primary monitor only — the host default until `SubscribeOutputs`.
    #[default]
    Primary,
    /// These outputs (`--monitor NAME`, repeatable), capped at
    /// [`RUDP_MAX_SUBSCRIBED_OUTPUTS`]. Unknown names are skipped; none left ⇒
    /// primary.
    Named(Vec<String>),
    /// Every host monitor, up to [`RUDP_MAX_SUBSCRIBED_OUTPUTS`]
    /// (`--all-monitors` / saved `rudp_separate_monitor_windows`).
    All,
}

impl MonitorChoice {
    /// CLI override (`--monitor` / `--all-monitors`); `None` when neither given.
    pub fn from_cli(monitors: &[String], all: bool) -> Option<Self> {
        if all {
            Some(Self::All)
        } else if monitors.is_empty() {
            None
        } else {
            Some(Self::Named(monitors.to_vec()))
        }
    }

    /// Saved-host preference (`viewer.json` `rudp_separate_monitor_windows`).
    pub fn from_preference(separate_windows: bool) -> Self {
        if separate_windows {
            Self::All
        } else {
            Self::Primary
        }
    }

    /// Output ids to request for `layout`; `None` ⇒ keep the host default
    /// (primary only), i.e. send no `SubscribeOutputs` at all.
    fn wanted(&self, layout: &[RudpMonitorInfo]) -> Option<Vec<String>> {
        let mut ids: Vec<String> = match self {
            Self::Primary => return None,
            Self::Named(names) => names
                .iter()
                .filter(|n| layout.iter().any(|m| &m.id == *n))
                .cloned()
                .collect(),
            Self::All => {
                // Primary first so the first window is the main screen.
                let mut ordered: Vec<&RudpMonitorInfo> = layout.iter().collect();
                ordered.sort_by_key(|m| !m.primary);
                ordered.into_iter().map(|m| m.id.clone()).collect()
            }
        };
        let mut seen = HashSet::new();
        ids.retain(|id| seen.insert(id.clone()));
        ids.truncate(RUDP_MAX_SUBSCRIBED_OUTPUTS);
        if ids.is_empty() { None } else { Some(ids) }
    }
}

enum ConnectAttempt {
    Ok(RudpSession),
    Cancelled,
    Err(ClientError),
}

async fn connect_until_cancelled(cfg: RudpClientConfig, cancel: &AtomicBool) -> ConnectAttempt {
    tokio::select! {
        biased;
        result = connect(cfg) => match result {
            Ok(s) => ConnectAttempt::Ok(s),
            Err(e) => ConnectAttempt::Err(e),
        },
        _ = async {
            while !cancel.load(Ordering::Relaxed) {
                tokio::time::sleep(std::time::Duration::from_millis(40)).await;
            }
        } => ConnectAttempt::Cancelled,
    }
}

/// Abort an in-flight [`open_rudp_session`] handshake (including TOFU wait).
pub struct RudpConnectCancel {
    cancel: Arc<AtomicBool>,
    worker_tx: Sender<ToWorker>,
}

impl RudpConnectCancel {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        metis_config::clear_viewer_pending_placement();
        let _ = self.worker_tx.send(ToWorker::Shutdown);
    }
}

/// True when a failure string is a user-initiated cancel (no critical notify).
pub fn is_cancelled_message(msg: &str) -> bool {
    msg == tr("Connection cancelled.")
}

enum ToGtk {
    Connected,
    Event(SessionEvent),
    Failed(String),
    /// Worker is blocked waiting for [`ToWorker::TofuAccept`] / Decline.
    TofuUnknown {
        fingerprint: String,
        host_key: String,
    },
    TofuMismatch {
        got: String,
        pinned: String,
        host_key: String,
    },
}

enum ToWorker {
    Input(RudpControlMsg),
    Shutdown,
    TofuAccept,
    TofuDecline,
}

/// Open an RUDP session: TOFU (interactive), decode, present, send input.
///
/// `on_outcome` is invoked on the GTK thread: `Ok(())` when the session window
/// opens, `Err(message)` when connect fails. Failures are reported only via
/// this callback (no separate AlertDialog — those stack under the Viewer on
/// Wayland and trap focus).
///
/// Returns a cancel handle while the handshake is in flight (or `None` when
/// the attempt failed synchronously before a worker started).
#[allow(clippy::too_many_arguments)]
pub fn open_rudp_session(
    parent: &impl IsA<gtk::Window>,
    host: String,
    port: u16,
    username: String,
    mut password: String,
    monitors: MonitorChoice,
    placement: ViewerPlacement,
    on_outcome: impl Fn(Result<(), String>) + 'static,
) -> Option<RudpConnectCancel> {
    let host_key = format!("{host}:{port}");
    let on_outcome = Rc::new(on_outcome);
    let addr: SocketAddr = match resolve_host_port(&host, port) {
        Ok(a) => a,
        Err(e) => {
            metis_config::clear_viewer_pending_placement();
            on_outcome(Err(format!("{}: {e}", tr("Invalid host address"))));
            password.zeroize();
            return None;
        }
    };

    let (gtk_tx, gtk_rx) = mpsc::channel::<ToGtk>();
    let (worker_tx, worker_rx) = mpsc::channel::<ToWorker>();
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let pass = password.clone();
    password.zeroize();
    let host_key_worker = host_key.clone();

    if let Err(err) = std::thread::Builder::new()
        .name("metis-viewer-rudp".into())
        .spawn(move || {
            worker_main(
                addr,
                host_key_worker,
                username,
                pass,
                gtk_tx,
                worker_rx,
                cancel_worker,
            );
        })
    {
        metis_config::clear_viewer_pending_placement();
        on_outcome(Err(format!("{}: {err}", tr("Could not start connection"))));
        return None;
    }

    let handle = RudpConnectCancel {
        cancel,
        worker_tx: worker_tx.clone(),
    };

    let parent = parent.as_ref().clone();
    let worker_tx = Rc::new(worker_tx);
    let gtk_rx = Rc::new(Mutex::new(gtk_rx));
    let session_built = Rc::new(Cell::new(false));

    glib::timeout_add_local(std::time::Duration::from_millis(16), move || {
        let Ok(rx) = gtk_rx.lock() else {
            return glib::ControlFlow::Continue;
        };
        match rx.try_recv() {
            Ok(ToGtk::Connected) => {
                drop(rx);
                if !session_built.get() {
                    session_built.set(true);
                    SessionUi::start(
                        &parent,
                        host_key.clone(),
                        worker_tx.clone(),
                        gtk_rx.clone(),
                        monitors.clone(),
                        placement,
                    );
                    on_outcome(Ok(()));
                }
                // Session window owns further event polling.
                glib::ControlFlow::Break
            }
            Ok(ToGtk::Failed(msg)) => {
                drop(rx);
                on_outcome(Err(msg));
                glib::ControlFlow::Break
            }
            Ok(ToGtk::TofuUnknown {
                fingerprint,
                host_key,
            }) => {
                drop(rx);
                let parent = parent.clone();
                let worker_tx = worker_tx.clone();
                glib::spawn_future_local(async move {
                    if trust_dialog(&parent, &fingerprint).await
                        && pin_host(&host_key, &fingerprint).is_ok()
                    {
                        let _ = worker_tx.send(ToWorker::TofuAccept);
                    } else {
                        let _ = worker_tx.send(ToWorker::TofuDecline);
                    }
                });
                glib::ControlFlow::Continue
            }
            Ok(ToGtk::TofuMismatch {
                got,
                pinned,
                host_key,
            }) => {
                drop(rx);
                let parent = parent.clone();
                let worker_tx = worker_tx.clone();
                glib::spawn_future_local(async move {
                    if clear_pin_dialog(&parent, &got, &pinned).await {
                        let _ = clear_host_pin(&host_key);
                        let _ = worker_tx.send(ToWorker::TofuAccept);
                    } else {
                        let _ = worker_tx.send(ToWorker::TofuDecline);
                    }
                });
                glib::ControlFlow::Continue
            }
            Ok(ToGtk::Event(_)) => glib::ControlFlow::Continue,
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => {
                if !session_built.get() {
                    on_outcome(Err(tr("Connection failed.")));
                }
                glib::ControlFlow::Break
            }
        }
    });

    Some(handle)
}

fn worker_main(
    addr: SocketAddr,
    host_key: String,
    username: String,
    mut password: String,
    gtk_tx: Sender<ToGtk>,
    worker_rx: Receiver<ToWorker>,
    cancel: Arc<AtomicBool>,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = gtk_tx.send(ToGtk::Failed(e.to_string()));
            password.zeroize();
            return;
        }
    };

    let mut session = loop {
        if cancel.load(Ordering::SeqCst) {
            let _ = gtk_tx.send(ToGtk::Failed(tr("Connection cancelled.")));
            password.zeroize();
            return;
        }
        let cfg = RudpClientConfig {
            addr,
            host_key: host_key.clone(),
            username: username.clone(),
            password: password.clone(),
            tofu: TofuMode::Interactive,
        };

        match rt.block_on(connect_until_cancelled(cfg, &cancel)) {
            ConnectAttempt::Ok(s) => break s,
            ConnectAttempt::Cancelled => {
                let _ = gtk_tx.send(ToGtk::Failed(tr("Connection cancelled.")));
                password.zeroize();
                return;
            }
            ConnectAttempt::Err(ClientError::TofuUnknown { fingerprint }) => {
                if gtk_tx
                    .send(ToGtk::TofuUnknown {
                        fingerprint,
                        host_key: host_key.clone(),
                    })
                    .is_err()
                {
                    password.zeroize();
                    return;
                }
                match wait_tofu_decision(&worker_rx) {
                    TofuWait::Accept => continue,
                    TofuWait::Decline | TofuWait::Shutdown => {
                        let _ = gtk_tx.send(ToGtk::Failed(tr("Connection cancelled.")));
                        password.zeroize();
                        return;
                    }
                }
            }
            ConnectAttempt::Err(ClientError::TofuMismatch { got, pinned }) => {
                if gtk_tx
                    .send(ToGtk::TofuMismatch {
                        got,
                        pinned,
                        host_key: host_key.clone(),
                    })
                    .is_err()
                {
                    password.zeroize();
                    return;
                }
                match wait_tofu_decision(&worker_rx) {
                    TofuWait::Accept => continue,
                    TofuWait::Decline | TofuWait::Shutdown => {
                        let _ = gtk_tx.send(ToGtk::Failed(tr("Connection cancelled.")));
                        password.zeroize();
                        return;
                    }
                }
            }
            ConnectAttempt::Err(e) => {
                let _ = gtk_tx.send(ToGtk::Failed(friendly_connect_error(&e)));
                password.zeroize();
                return;
            }
        }
    };
    password.zeroize();

    if cancel.load(Ordering::SeqCst) {
        let _ = gtk_tx.send(ToGtk::Failed(tr("Connection cancelled.")));
        return;
    }

    let _ = gtk_tx.send(ToGtk::Connected);
    tracing::info!(id = %session.session_id(), "metis-viewer: RUDP SessionOk");

    loop {
        // Drain input without blocking the event pump too long.
        while let Ok(msg) = worker_rx.try_recv() {
            match msg {
                ToWorker::Input(m) => {
                    let _ = rt.block_on(session.send_input(m));
                }
                ToWorker::Shutdown | ToWorker::TofuAccept | ToWorker::TofuDecline => return,
            }
        }
        if let Some(ev) = rt.block_on(async {
            tokio::select! {
                biased;
                ev = session.recv_event() => ev,
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => None,
            }
        }) {
            let done = matches!(ev, SessionEvent::Disconnected);
            if gtk_tx.send(ToGtk::Event(ev)).is_err() {
                return;
            }
            if done {
                return;
            }
        }
        // Also check for disconnect via try after timeout path.
        while let Some(ev) = session.try_recv_event() {
            let done = matches!(ev, SessionEvent::Disconnected);
            if gtk_tx.send(ToGtk::Event(ev)).is_err() {
                return;
            }
            if done {
                return;
            }
        }
    }
}

enum TofuWait {
    Accept,
    Decline,
    Shutdown,
}

fn wait_tofu_decision(worker_rx: &Receiver<ToWorker>) -> TofuWait {
    loop {
        match worker_rx.recv() {
            Ok(ToWorker::TofuAccept) => return TofuWait::Accept,
            Ok(ToWorker::TofuDecline) => return TofuWait::Decline,
            Ok(ToWorker::Shutdown) => return TofuWait::Shutdown,
            Ok(ToWorker::Input(_)) => {}
            Err(_) => return TofuWait::Shutdown,
        }
    }
}

fn friendly_connect_error(err: &ClientError) -> String {
    match err {
        ClientError::Rejected { reason } if reason.starts_with("auth_failed") => {
            tr("Authentication failed. Check username and password.")
        }
        ClientError::Rejected { reason } if reason.starts_with("not_allowed") => {
            tr("This user is not allowed for Metis Remote on the host.")
        }
        ClientError::Rejected { reason } => {
            format!("{}: {reason}", tr("Connection rejected"))
        }
        ClientError::Message(m)
            if m.contains("timed out")
                || m.contains("ConnectionRefused")
                || m.contains("connection refused")
                || m.to_ascii_lowercase().contains("unreachable") =>
        {
            tr("Could not reach Metis Remote. Enable it on the host \
                 (Settings → Metis Remote), confirm the LAN firewall applied, \
                 and check the address/port (try 127.0.0.1 if using localhost).")
        }
        ClientError::Message(m)
            if m.contains("connection lost")
                || m.contains("ConnectionLost")
                || m.contains("connection closed")
                || m.contains("ConnectionClosed") =>
        {
            tr("Sign-in failed or the host closed the connection. \
                 Check username/password and that Metis Remote is enabled.")
        }
        other => other.to_string(),
    }
}

/// Rectangle `(x, y, w, h)` in widget pixels that a `vw`×`vh` picture occupies
/// inside an `aw`×`ah` widget under [`gtk::ContentFit::Contain`].
fn contain_rect(aw: f64, ah: f64, vw: f64, vh: f64) -> (f64, f64, f64, f64) {
    if aw <= 0.0 || ah <= 0.0 || vw <= 0.0 || vh <= 0.0 {
        return (0.0, 0.0, aw.max(1.0), ah.max(1.0));
    }
    let scale = (aw / vw).min(ah / vh);
    let (w, h) = (vw * scale, vh * scale);
    ((aw - w) / 2.0, (ah - h) / 2.0, w, h)
}

/// Content rect for pointer mapping — full widget when `fill` (ContentFit::Fill).
fn content_rect(aw: f64, ah: f64, vw: f64, vh: f64, fill: bool) -> (f64, f64, f64, f64) {
    if fill {
        (0.0, 0.0, aw.max(1.0), ah.max(1.0))
    } else {
        contain_rect(aw, ah, vw, vh)
    }
}

/// Logical `(width, height)` of the monitor behind a stream: the layout's
/// logical size when known, else the decoded video size.
fn logical_size(video: (u32, u32), monitor: Option<&RudpMonitorInfo>) -> (f64, f64) {
    match monitor {
        Some(m) if m.width > 0 && m.height > 0 => (f64::from(m.width), f64::from(m.height)),
        _ => (f64::from(video.0), f64::from(video.1)),
    }
}

/// Window-local pixels → compositor logical desktop coordinates, adding the
/// monitor's layout origin so a second monitor lands at the right place.
fn map_absolute(
    x: f64,
    y: f64,
    (aw, ah): (f64, f64),
    video: (u32, u32),
    monitor: Option<&RudpMonitorInfo>,
    fill: bool,
) -> (f64, f64) {
    let (rx, ry, rw, rh) = content_rect(aw, ah, f64::from(video.0), f64::from(video.1), fill);
    let u = ((x - rx) / rw).clamp(0.0, 1.0);
    let v = ((y - ry) / rh).clamp(0.0, 1.0);
    let (lw, lh) = logical_size(video, monitor);
    let (ox, oy) = monitor.map_or((0.0, 0.0), |m| (f64::from(m.x), f64::from(m.y)));
    // `lw - 1`: the last pixel column/row, so the far edge stays on this monitor.
    (ox + u * (lw - 1.0).max(0.0), oy + v * (lh - 1.0).max(0.0))
}

/// Relative pointer delta in window pixels → logical pixels of the monitor.
fn map_relative(
    dx: f64,
    dy: f64,
    (aw, ah): (f64, f64),
    video: (u32, u32),
    monitor: Option<&RudpMonitorInfo>,
    fill: bool,
) -> (f64, f64) {
    let (_, _, rw, rh) = content_rect(aw, ah, f64::from(video.0), f64::from(video.1), fill);
    let (lw, lh) = logical_size(video, monitor);
    (dx * lw / rw.max(1.0), dy * lh / rh.max(1.0))
}

fn decoder_codec(name: &str) -> RudpCodec {
    match name {
        "h264" => RudpCodec::H264,
        "av1" => RudpCodec::Av1,
        _ => RudpCodec::Hevc,
    }
}

/// Peek strip height used for CSS slide + top-edge hit test.
const CONN_BAR_PEEK_PX: f64 = 8.0;
const CONN_BAR_HIDE_MS: u64 = 1500;

/// One host-monitor stream: its window, decoder and pointer state.
struct StreamWindow {
    /// `None` until the first `VideoReady` binds this (placeholder) window.
    stream_id: Cell<Option<u8>>,
    output_id: RefCell<String>,
    win: gtk::Window,
    picture: gtk::Picture,
    /// Corner label until first video; then hidden (status lives in the bar).
    waiting: gtk::Label,
    bar: gtk::Box,
    bar_status: gtk::Label,
    clipboard_check: gtk::CheckButton,
    pointer_over_bar: Cell<bool>,
    video_size: Cell<(u32, u32)>,
    last_pos: Cell<(f64, f64)>,
    decoder: RefCell<Option<Box<dyn metis_decode::VideoDecoder>>>,
}

impl StreamWindow {
    fn set_status(&self, text: &str) {
        self.bar_status.set_text(text);
    }

    fn set_bar_revealed(&self, revealed: bool) {
        if revealed {
            self.bar.add_css_class("revealed");
        } else {
            self.bar.remove_css_class("revealed");
        }
    }
}

/// Stable product prefix — must match compositor
/// [`title_looks_metis_remote_session`] (do not translate; connect UI is
/// “Metis Viewer”).
pub(crate) const RUDP_SESSION_TITLE_PREFIX: &str = "Metis Remote";

/// Window title for a Metis Remote stream — always prefixed so the compositor
/// can place session windows without matching the Viewer connect UI.
pub(crate) fn rudp_session_window_title(output_id: &str, multi_monitor: bool) -> String {
    if !multi_monitor || output_id.is_empty() {
        RUDP_SESSION_TITLE_PREFIX.to_string()
    } else {
        format!("{RUDP_SESSION_TITLE_PREFIX} — {output_id}")
    }
}

/// Shared state for one Metis Remote session (all of its windows).
struct SessionUi {
    /// Owning GTK application — required so Wayland xdg `app_id` is set.
    app: Option<gtk::Application>,
    /// `host:port` shown in the connection bar.
    host_key: String,
    /// When [`ViewerPlacement::Workspace`], request true fullscreen (hides local
    /// edge bar under Metis; compositor also fullscreen-places on stamp).
    fill_dedicated_desk: bool,
    worker_tx: Rc<Sender<ToWorker>>,
    choice: MonitorChoice,
    layout: RefCell<Vec<RudpMonitorInfo>>,
    /// Last `SubscribeOutputs` list sent (`None` = never sent, host default).
    subscribed: RefCell<Option<Vec<String>>>,
    windows: RefCell<Vec<Rc<StreamWindow>>>,
    /// Outputs whose window the user closed — never re-subscribed this session.
    closed_outputs: RefCell<HashSet<String>>,
    pointer_locked: Cell<bool>,
    audio_playback: RefCell<Option<crate::rudp_audio::RudpAudioPlayback>>,
    /// Client-side clipboard sync (default on). Mute skips send/apply.
    clipboard_enabled: Cell<bool>,
    applying_remote_clip: Cell<bool>,
    last_remote_clip_serial: Cell<u64>,
    outbound_clip_serial: Cell<u64>,
    last_outbound_clip: RefCell<String>,
    clip_handler: Cell<Option<glib::SignalHandlerId>>,
    bar_revealed: Cell<bool>,
    bar_hide_timer: RefCell<Option<glib::SourceId>>,
    finished: Cell<bool>,
}

impl SessionUi {
    /// Build the session UI (one placeholder window) and start the event pump.
    fn start(
        parent: &gtk::Window,
        host_key: String,
        worker_tx: Rc<Sender<ToWorker>>,
        gtk_rx: Rc<Mutex<Receiver<ToGtk>>>,
        choice: MonitorChoice,
        placement: ViewerPlacement,
    ) {
        let app = parent.application();
        let ui = Rc::new(Self {
            app,
            host_key,
            fill_dedicated_desk: placement == ViewerPlacement::Workspace,
            worker_tx,
            choice,
            layout: RefCell::new(Vec::new()),
            subscribed: RefCell::new(None),
            windows: RefCell::new(Vec::new()),
            closed_outputs: RefCell::new(HashSet::new()),
            pointer_locked: Cell::new(false),
            audio_playback: RefCell::new(None),
            clipboard_enabled: Cell::new(true),
            applying_remote_clip: Cell::new(false),
            last_remote_clip_serial: Cell::new(0),
            outbound_clip_serial: Cell::new(0),
            last_outbound_clip: RefCell::new(String::new()),
            clip_handler: Cell::new(None),
            bar_revealed: Cell::new(false),
            bar_hide_timer: RefCell::new(None),
            finished: Cell::new(false),
        });
        let first = ui.add_window();
        first.picture.grab_focus();
        ui.connect_clipboard();

        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            let Ok(rx) = gtk_rx.lock() else {
                return glib::ControlFlow::Continue;
            };
            for _ in 0..32 {
                match rx.try_recv() {
                    Ok(ToGtk::Event(ev)) => {
                        if matches!(ev, SessionEvent::Disconnected) {
                            ui.finish(&tr("Disconnected"));
                            return glib::ControlFlow::Break;
                        }
                        ui.handle_event(&ev);
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        ui.finish(&tr("Disconnected"));
                        return glib::ControlFlow::Break;
                    }
                }
            }
            if ui.finished.get() || ui.windows.borrow().is_empty() {
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    fn send_input(&self, msg: RudpControlMsg) {
        let _ = self.worker_tx.send(ToWorker::Input(msg));
    }

    fn monitor_for(&self, output_id: &str) -> Option<RudpMonitorInfo> {
        self.layout
            .borrow()
            .iter()
            .find(|m| m.id == output_id)
            .cloned()
    }

    fn window_title(&self, output_id: &str) -> String {
        rudp_session_window_title(output_id, self.layout.borrow().len() >= 2)
    }

    fn refresh_titles(&self) {
        for sw in self.windows.borrow().iter() {
            let title = self.window_title(&sw.output_id.borrow());
            sw.win.set_title(Some(&title));
        }
    }

    /// Session over: leave windows open with a final status, drop shared
    /// resources, break Rc cycles.
    fn finish(&self, status: &str) {
        if self.finished.replace(true) {
            return;
        }
        *self.audio_playback.borrow_mut() = None;
        self.disconnect_clipboard();
        self.cancel_bar_hide();
        for sw in self.windows.borrow().iter() {
            sw.waiting.set_visible(false);
            sw.set_status(status);
        }
        self.set_bars_revealed(true);
    }

    fn disconnect_clipboard(&self) {
        if let Some(id) = self.clip_handler.take()
            && let Some(display) = gdk::Display::default()
        {
            display.clipboard().disconnect(id);
        }
    }

    fn shutdown(&self) {
        self.finished.set(true);
        *self.audio_playback.borrow_mut() = None;
        self.disconnect_clipboard();
        self.cancel_bar_hide();
        metis_config::clear_viewer_pending_placement();
        let _ = self.worker_tx.send(ToWorker::Shutdown);
    }

    /// End the whole session (all monitor windows + Quinn worker).
    fn disconnect_session(self: &Rc<Self>) {
        let wins: Vec<gtk::Window> = self
            .windows
            .borrow()
            .iter()
            .map(|w| w.win.clone())
            .collect();
        for win in wins {
            win.close();
        }
        if !self.finished.get() {
            self.shutdown();
        }
    }

    fn any_pointer_over_bar(&self) -> bool {
        self.windows
            .borrow()
            .iter()
            .any(|w| w.pointer_over_bar.get())
    }

    fn set_bars_revealed(&self, revealed: bool) {
        self.bar_revealed.set(revealed);
        for sw in self.windows.borrow().iter() {
            sw.set_bar_revealed(revealed);
        }
    }

    fn reveal_bars(self: &Rc<Self>) {
        self.cancel_bar_hide();
        self.set_bars_revealed(true);
    }

    fn toggle_bars(self: &Rc<Self>) {
        if self.bar_revealed.get() {
            self.cancel_bar_hide();
            self.set_bars_revealed(false);
        } else {
            self.reveal_bars();
            self.schedule_bar_hide();
        }
    }

    fn cancel_bar_hide(&self) {
        if let Some(id) = self.bar_hide_timer.borrow_mut().take() {
            id.remove();
        }
    }

    fn schedule_bar_hide(self: &Rc<Self>) {
        // Do not reset an in-flight timer — video motion would keep the bar open.
        if self.bar_hide_timer.borrow().is_some() {
            return;
        }
        let ui = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(
            std::time::Duration::from_millis(CONN_BAR_HIDE_MS),
            move || {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                *ui.bar_hide_timer.borrow_mut() = None;
                if !ui.any_pointer_over_bar() {
                    ui.set_bars_revealed(false);
                }
            },
        );
        *self.bar_hide_timer.borrow_mut() = Some(id);
    }

    fn set_clipboard_enabled(self: &Rc<Self>, enabled: bool) {
        if self.clipboard_enabled.replace(enabled) == enabled {
            return;
        }
        if enabled {
            self.connect_clipboard();
        } else {
            self.disconnect_clipboard();
        }
        for sw in self.windows.borrow().iter() {
            if sw.clipboard_check.is_active() != enabled {
                sw.clipboard_check.set_active(enabled);
            }
        }
    }

    /// Re-send `SubscribeOutputs` for the windows that remain after a close.
    fn resubscribe_remaining(&self) {
        let remaining: Vec<String> = self
            .windows
            .borrow()
            .iter()
            .map(|w| w.output_id.borrow().clone())
            .filter(|id| !id.is_empty())
            .collect();
        if remaining.is_empty() {
            return;
        }
        self.send_input(RudpControlMsg::SubscribeOutputs {
            output_ids: remaining.clone(),
        });
        *self.subscribed.borrow_mut() = Some(remaining);
    }

    fn on_window_closed(self: &Rc<Self>, sw: &Rc<StreamWindow>) {
        let output = sw.output_id.borrow().clone();
        self.windows.borrow_mut().retain(|w| !Rc::ptr_eq(w, sw));
        if self.windows.borrow().is_empty() {
            self.shutdown();
            return;
        }
        if !output.is_empty() {
            self.closed_outputs.borrow_mut().insert(output);
            self.resubscribe_remaining();
        }
    }

    /// Create one monitor window with its input controllers.
    fn add_window(self: &Rc<Self>) -> Rc<StreamWindow> {
        // No transient_for: Metis must be able to move these onto the ephemeral
        // remote desk; dialogs keep an explicit parent separately.
        let win = gtk::Window::builder()
            .title(RUDP_SESSION_TITLE_PREFIX)
            .default_width(1280)
            .default_height(720)
            .modal(false)
            .build();
        // Advertise `com.metis.Viewer` as xdg app_id (plain Window omits it).
        if let Some(app) = self.app.as_ref() {
            win.set_application(Some(app));
        }
        win.add_css_class("metis-viewer-window");

        let picture = gtk::Picture::new();
        // Dedicated fullscreen: Fill so the stream covers the output (Contain
        // letterboxing caused a cascading “hall of mirrors” on loopback and left
        // gaps where local chrome showed through). Window mode keeps Contain.
        picture.set_content_fit(if self.fill_dedicated_desk {
            gtk::ContentFit::Fill
        } else {
            gtk::ContentFit::Contain
        });
        picture.set_hexpand(true);
        picture.set_vexpand(true);
        picture.set_can_focus(true);

        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&picture));

        let waiting = gtk::Label::new(Some(&tr("Waiting for video…")));
        waiting.add_css_class("metis-viewer-waiting");
        waiting.set_halign(gtk::Align::Start);
        waiting.set_valign(gtk::Align::Start);
        waiting.set_margin_start(12);
        waiting.set_margin_top(12);
        overlay.add_overlay(&waiting);

        let bar = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        bar.add_css_class("metis-viewer-conn-bar");
        bar.set_halign(gtk::Align::Fill);
        bar.set_valign(gtk::Align::Start);
        bar.set_hexpand(true);
        // Keep the strip above the video for hit-testing while slid off-screen.
        bar.set_can_target(true);

        let title = gtk::Label::new(Some(RUDP_SESSION_TITLE_PREFIX));
        title.add_css_class("metis-viewer-conn-bar-title");
        title.set_xalign(0.0);
        let host = gtk::Label::new(Some(&self.host_key));
        host.add_css_class("metis-viewer-conn-bar-host");
        host.set_xalign(0.0);
        let title_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        title_col.append(&title);
        title_col.append(&host);
        bar.append(&title_col);

        let bar_status = gtk::Label::new(Some(&tr("Waiting for video…")));
        bar_status.add_css_class("metis-viewer-conn-bar-status");
        bar_status.set_xalign(0.0);
        bar_status.set_hexpand(true);
        bar_status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        bar.append(&bar_status);

        let clipboard_check = gtk::CheckButton::with_label(&tr("Clipboard"));
        clipboard_check.set_active(self.clipboard_enabled.get());
        clipboard_check.set_tooltip_text(Some(&tr(
            "Sync text clipboard with the remote. Off: local mute only (no protocol change).",
        )));
        bar.append(&clipboard_check);

        let disconnect = gtk::Button::with_label(&tr("Disconnect"));
        disconnect.add_css_class("destructive-action");
        disconnect.set_tooltip_text(Some(&tr(
            "End this Metis Remote session (all monitor windows). Super+Q also closes the focused window.",
        )));
        bar.append(&disconnect);

        overlay.add_overlay(&bar);
        win.set_child(Some(&overlay));

        let sw = Rc::new(StreamWindow {
            stream_id: Cell::new(None),
            output_id: RefCell::new(String::new()),
            win: win.clone(),
            picture: picture.clone(),
            waiting,
            bar: bar.clone(),
            bar_status,
            clipboard_check: clipboard_check.clone(),
            pointer_over_bar: Cell::new(false),
            video_size: Cell::new((0, 0)),
            last_pos: Cell::new((0.0, 0.0)),
            decoder: RefCell::new(None),
        });
        sw.set_bar_revealed(self.bar_revealed.get());
        self.windows.borrow_mut().push(Rc::clone(&sw));

        {
            let ui = Rc::downgrade(self);
            let sw_weak = Rc::downgrade(&sw);
            win.connect_close_request(move |_| {
                if let (Some(ui), Some(sw)) = (ui.upgrade(), sw_weak.upgrade()) {
                    ui.on_window_closed(&sw);
                }
                glib::Propagation::Proceed
            });
        }
        {
            let ui = Rc::downgrade(self);
            disconnect.connect_clicked(move |_| {
                if let Some(ui) = ui.upgrade() {
                    ui.disconnect_session();
                }
            });
        }
        {
            let ui = Rc::downgrade(self);
            clipboard_check.connect_toggled(move |check| {
                if let Some(ui) = ui.upgrade() {
                    ui.set_clipboard_enabled(check.is_active());
                }
            });
        }
        {
            let ui = Rc::downgrade(self);
            let sw_weak = Rc::downgrade(&sw);
            let bar_motion = gtk::EventControllerMotion::new();
            bar_motion.connect_enter(move |_, _, _| {
                if let (Some(ui), Some(sw)) = (ui.upgrade(), sw_weak.upgrade()) {
                    sw.pointer_over_bar.set(true);
                    ui.reveal_bars();
                }
            });
            let ui = Rc::downgrade(self);
            let sw_weak = Rc::downgrade(&sw);
            bar_motion.connect_leave(move |_| {
                if let (Some(ui), Some(sw)) = (ui.upgrade(), sw_weak.upgrade()) {
                    sw.pointer_over_bar.set(false);
                    ui.schedule_bar_hide();
                }
            });
            bar.add_controller(bar_motion);
        }
        {
            // Top-edge peek + Ctrl+Alt+Home (Windows RDP connection-bar chord).
            let ui = Rc::downgrade(self);
            let edge = gtk::EventControllerMotion::new();
            edge.connect_motion(move |_, _, y| {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                if y <= CONN_BAR_PEEK_PX {
                    ui.reveal_bars();
                } else if ui.bar_revealed.get() && !ui.any_pointer_over_bar() {
                    ui.schedule_bar_hide();
                }
            });
            win.add_controller(edge);

            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            let ui = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, mods| {
                let ctrl = mods.contains(gdk::ModifierType::CONTROL_MASK);
                let alt = mods.contains(gdk::ModifierType::ALT_MASK);
                if ctrl && alt && key == gdk::Key::Home {
                    if let Some(ui) = ui.upgrade() {
                        ui.toggle_bars();
                    }
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            win.add_controller(keys);
        }
        self.attach_input(&sw);
        win.present();
        if self.fill_dedicated_desk {
            // True fullscreen so the stream covers the output (Metis also
            // fullscreen-places + hides the edge bar; this covers non-Metis /
            // race before place).
            win.fullscreen();
        }
        sw
    }

    fn attach_input(self: &Rc<Self>, sw: &Rc<StreamWindow>) {
        let picture = &sw.picture;
        {
            let ui: Weak<SessionUi> = Rc::downgrade(self);
            let sw_weak = Rc::downgrade(sw);
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(move |_, x, y| {
                let (Some(ui), Some(sw)) = (ui.upgrade(), sw_weak.upgrade()) else {
                    return;
                };
                let video = sw.video_size.get();
                if video.0 == 0 || video.1 == 0 {
                    return;
                }
                let area = (
                    f64::from(sw.picture.width().max(1)),
                    f64::from(sw.picture.height().max(1)),
                );
                let monitor = ui.monitor_for(&sw.output_id.borrow());
                let fill = ui.fill_dedicated_desk;
                if ui.pointer_locked.get() {
                    let (lx, ly) = sw.last_pos.get();
                    sw.last_pos.set((x, y));
                    let (dx, dy) =
                        map_relative(x - lx, y - ly, area, video, monitor.as_ref(), fill);
                    if dx != 0.0 || dy != 0.0 {
                        ui.send_input(RudpControlMsg::PointerRelative { dx, dy });
                    }
                } else {
                    sw.last_pos.set((x, y));
                    let (ax, ay) = map_absolute(x, y, area, video, monitor.as_ref(), fill);
                    ui.send_input(RudpControlMsg::PointerAbsolute { x: ax, y: ay });
                }
            });
            picture.add_controller(motion);
        }

        {
            let click = gtk::GestureClick::new();
            click.set_button(0);
            let ui = Rc::downgrade(self);
            click.connect_pressed(move |g, _, _, _| {
                if let Some(ui) = ui.upgrade() {
                    ui.send_input(RudpControlMsg::PointerButton {
                        button: map_gdk_button(g.current_button()),
                        pressed: true,
                    });
                }
            });
            let ui = Rc::downgrade(self);
            click.connect_released(move |g, _, _, _| {
                if let Some(ui) = ui.upgrade() {
                    ui.send_input(RudpControlMsg::PointerButton {
                        button: map_gdk_button(g.current_button()),
                        pressed: false,
                    });
                }
            });
            picture.add_controller(click);
        }

        {
            let ui = Rc::downgrade(self);
            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            scroll.connect_scroll(move |_, dx, dy| {
                if let Some(ui) = ui.upgrade() {
                    ui.send_input(RudpControlMsg::PointerScroll { dx, dy: -dy });
                }
                glib::Propagation::Stop
            });
            picture.add_controller(scroll);
        }

        {
            // Keyboard events reach only the focused window — that is the
            // "active monitor" for typing.
            let keys = gtk::EventControllerKey::new();
            let ui = Rc::downgrade(self);
            keys.connect_key_pressed(move |_, key, _, _| {
                if let (Some(ui), Some(evdev)) = (ui.upgrade(), gdk_key_to_evdev(key)) {
                    ui.send_input(RudpControlMsg::Key {
                        keycode: evdev,
                        pressed: true,
                    });
                }
                glib::Propagation::Proceed
            });
            let ui = Rc::downgrade(self);
            keys.connect_key_released(move |_, key, _, _| {
                if let (Some(ui), Some(evdev)) = (ui.upgrade(), gdk_key_to_evdev(key)) {
                    ui.send_input(RudpControlMsg::Key {
                        keycode: evdev,
                        pressed: false,
                    });
                }
            });
            picture.add_controller(keys);
        }
    }

    /// Mirror the local clipboard to the host (one watcher for all windows).
    fn connect_clipboard(self: &Rc<Self>) {
        if !self.clipboard_enabled.get() {
            return;
        }
        // Already watching — Cell has no Copy get for SignalHandlerId.
        let existing = self.clip_handler.take();
        let already = existing.is_some();
        self.clip_handler.set(existing);
        if already {
            return;
        }
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let clipboard = display.clipboard();
        let ui = Rc::downgrade(self);
        let id = clipboard.connect_changed(move |cb| {
            let Some(strong) = ui.upgrade() else {
                return;
            };
            if !strong.clipboard_enabled.get() || strong.applying_remote_clip.get() {
                return;
            }
            let ui = ui.clone();
            cb.read_text_async(gio::Cancellable::NONE, move |result| {
                let Some(ui) = ui.upgrade() else {
                    return;
                };
                if !ui.clipboard_enabled.get() || ui.applying_remote_clip.get() {
                    return;
                }
                let Ok(Some(text)) = result else {
                    return;
                };
                let text = truncate_clipboard_text(text.as_str()).to_string();
                if text.is_empty() || *ui.last_outbound_clip.borrow() == text {
                    return;
                }
                *ui.last_outbound_clip.borrow_mut() = text.clone();
                let serial = ui.outbound_clip_serial.get().wrapping_add(1);
                ui.outbound_clip_serial.set(serial);
                ui.send_input(RudpControlMsg::ClipboardSet {
                    mime: "text/plain;charset=utf-8".into(),
                    text,
                    serial,
                });
            });
        });
        self.clip_handler.set(Some(id));
    }

    fn handle_event(self: &Rc<Self>, ev: &SessionEvent) {
        match ev {
            SessionEvent::DesktopLayout { monitors } => self.on_layout(monitors),
            SessionEvent::VideoReady {
                width,
                height,
                codec,
                stream_id,
                output_id,
            } => self.on_video_ready(*width, *height, codec, *stream_id, output_id),
            SessionEvent::AudioReady {
                sample_rate,
                channels,
                codec: _,
            } => match crate::rudp_audio::RudpAudioPlayback::start(*sample_rate, *channels) {
                Ok(playback) => {
                    *self.audio_playback.borrow_mut() = Some(playback);
                }
                Err(err) => {
                    tracing::warn!(%err, "rudp audio: playback unavailable");
                }
            },
            SessionEvent::AudioPacket {
                seq,
                pts_us: _,
                data,
            } => {
                if let Some(playback) = self.audio_playback.borrow().as_ref() {
                    playback.push_packet(*seq, data);
                }
            }
            SessionEvent::PointerLock { locked } => {
                self.pointer_locked.set(*locked);
                if *locked {
                    for sw in self.windows.borrow().iter() {
                        sw.set_status(&tr("Pointer lock — relative mouse"));
                    }
                }
            }
            SessionEvent::ClipboardSet {
                mime: _,
                text,
                serial,
            } => {
                if !self.clipboard_enabled.get() {
                    return;
                }
                if *serial != 0 && self.last_remote_clip_serial.get() == *serial {
                    return;
                }
                self.last_remote_clip_serial.set(*serial);
                // Seed before set_text so a delayed changed→read_text doesn't echo.
                *self.last_outbound_clip.borrow_mut() = text.clone();
                if let Some(display) = gdk::Display::default() {
                    self.applying_remote_clip.set(true);
                    display.clipboard().set_text(text);
                    self.applying_remote_clip.set(false);
                }
            }
            SessionEvent::AccessUnit(au) => self.on_access_unit(au),
            SessionEvent::Disconnected => self.finish(&tr("Disconnected")),
        }
    }

    /// Host topology: remember it, pick the outputs to stream, retitle windows.
    fn on_layout(&self, monitors: &[RudpMonitorInfo]) {
        *self.layout.borrow_mut() = monitors.to_vec();
        self.refresh_titles();
        for sw in self.windows.borrow().iter() {
            let output = sw.output_id.borrow();
            if !output.is_empty() && !monitors.iter().any(|m| m.id == *output) {
                sw.set_status(&tr("Monitor disconnected"));
            }
        }

        let closed = self.closed_outputs.borrow();
        let wanted = self.choice.wanted(monitors).map(|mut ids| {
            ids.retain(|id| !closed.contains(id));
            ids
        });
        drop(closed);
        let Some(wanted) = wanted.filter(|ids| !ids.is_empty()) else {
            return;
        };
        if self.subscribed.borrow().as_ref() == Some(&wanted) {
            return;
        }
        tracing::info!(outputs = ?wanted, "metis-viewer: subscribing to outputs");
        self.send_input(RudpControlMsg::SubscribeOutputs {
            output_ids: wanted.clone(),
        });
        *self.subscribed.borrow_mut() = Some(wanted);
    }

    fn window_for_stream(&self, stream_id: u8) -> Option<Rc<StreamWindow>> {
        self.windows
            .borrow()
            .iter()
            .find(|w| w.stream_id.get() == Some(stream_id))
            .cloned()
    }

    fn on_video_ready(
        self: &Rc<Self>,
        width: u32,
        height: u32,
        codec: &str,
        stream_id: u8,
        output_id: &str,
    ) {
        if self.closed_outputs.borrow().contains(output_id) {
            return;
        }
        let existing = self.window_for_stream(stream_id);
        let sw = match existing {
            Some(sw) => sw,
            None => {
                // Reuse the placeholder window first, then open more.
                let placeholder = self
                    .windows
                    .borrow()
                    .iter()
                    .find(|w| w.stream_id.get().is_none())
                    .cloned();
                match placeholder {
                    Some(sw) => sw,
                    None => self.add_window(),
                }
            }
        };
        sw.stream_id.set(Some(stream_id));
        *sw.output_id.borrow_mut() = output_id.to_string();
        sw.win.set_title(Some(&self.window_title(output_id)));
        sw.video_size.set((width, height));
        sw.waiting.set_visible(false);

        let codec_label = codec.to_string();
        let status = if self.layout.borrow().len() >= 2 && !output_id.is_empty() {
            format!("{output_id} · ")
        } else {
            String::new()
        };
        let opened = open_decoder(decoder_codec(codec), width, height).ok();
        match opened {
            Some(dec) => {
                if !self.pointer_locked.get() {
                    sw.set_status(&format!(
                        "{status}{width}×{height} {codec_label} · {}",
                        dec.info().decoder_name
                    ));
                }
                *sw.decoder.borrow_mut() = Some(dec);
            }
            None => {
                *sw.decoder.borrow_mut() = None;
                sw.set_status(&format!("{status}{width}×{height} {codec_label}"));
            }
        }
    }

    fn on_access_unit(&self, au: &metis_rudp_client::AccessUnitEvent) {
        // VideoReady for this stream hasn't arrived (or its window was closed).
        let Some(sw) = self.window_for_stream(au.stream_id) else {
            return;
        };
        let mut slot = sw.decoder.borrow_mut();
        if slot.is_none() {
            let (w, h) = sw.video_size.get();
            if w == 0 {
                return;
            }
            let Ok(d) = open_decoder(decoder_codec(&au.codec), w, h) else {
                return;
            };
            if !self.pointer_locked.get() {
                sw.set_status(&format!("{w}×{h} {} · {}", au.codec, d.info().decoder_name));
            }
            *slot = Some(d);
        }
        if let Some(dec) = slot.as_mut()
            && let Ok(Some(frame)) = dec.push_au(&au.data)
        {
            sw.video_size.set((frame.width, frame.height));
            present_rgba(&sw.picture, &frame.rgba, frame.width, frame.height);
        }
    }
}

fn present_rgba(picture: &gtk::Picture, rgba: &[u8], width: u32, height: u32) {
    let stride = (width * 4) as usize;
    let bytes = glib::Bytes::from(rgba);
    let texture = gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        stride,
    );
    picture.set_paintable(Some(&texture));
}

fn map_gdk_button(btn: u32) -> u32 {
    match btn {
        1 => 0x110,
        3 => 0x111,
        2 => 0x112,
        n => 0x110 + n.saturating_sub(1),
    }
}

fn gdk_key_to_evdev(key: gdk::Key) -> Option<u32> {
    use gdk::Key;
    match key {
        Key::Escape => Some(1),
        Key::Return | Key::KP_Enter => Some(28),
        Key::space => Some(57),
        Key::Left => Some(105),
        Key::Right => Some(106),
        Key::Up => Some(103),
        Key::Down => Some(108),
        Key::BackSpace => Some(14),
        Key::Tab => Some(15),
        k => {
            let name = k.name()?.to_ascii_lowercase();
            if name.len() != 1 {
                return None;
            }
            let c = name.chars().next()?;
            if c.is_ascii_lowercase() {
                Some(30 + (c as u32 - b'a' as u32))
            } else if c.is_ascii_digit() {
                Some(if c == '0' {
                    11
                } else {
                    2 + (c as u32 - b'1' as u32)
                })
            } else {
                None
            }
        }
    }
}

async fn trust_dialog(parent: &gtk::Window, fingerprint: &str) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(tr("Trust this Metis Remote host?"))
        .detail(format!(
            "{}\n\nSHA-256:\n{fingerprint}",
            tr("First connection — pin this fingerprint to continue.")
        ))
        .buttons([tr("Cancel"), tr("Trust")])
        .default_button(1)
        .cancel_button(0)
        .build();
    matches!(dialog.choose_future(Some(parent)).await, Ok(1))
}

async fn clear_pin_dialog(parent: &gtk::Window, got: &str, pinned: &str) -> bool {
    let dialog = gtk::AlertDialog::builder()
        .modal(true)
        .message(tr("Host fingerprint changed"))
        .detail(format!(
            "{}\n\n{}:\n{got}\n\n{}:\n{pinned}",
            tr("The remote certificate does not match the pinned fingerprint."),
            tr("New"),
            tr("Pinned")
        ))
        .buttons([tr("Cancel"), tr("Clear pin")])
        .default_button(0)
        .cancel_button(0)
        .build();
    matches!(dialog.choose_future(Some(parent)).await, Ok(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(id: &str, x: i32, y: i32, w: u32, h: u32, primary: bool) -> RudpMonitorInfo {
        RudpMonitorInfo {
            id: id.into(),
            name: id.into(),
            x,
            y,
            width: w,
            height: h,
            scale: 1.0,
            primary,
        }
    }

    #[test]
    fn contain_rect_letterboxes() {
        // 16:9 video in a square widget → bars top and bottom.
        let (x, y, w, h) = contain_rect(1000.0, 1000.0, 1920.0, 1080.0);
        assert!(x.abs() < 1e-9);
        assert!((w - 1000.0).abs() < 1e-9);
        assert!((h - 562.5).abs() < 1e-9);
        assert!((y - 218.75).abs() < 1e-9);
        // Degenerate input never divides by zero.
        let (_, _, w, h) = contain_rect(0.0, 0.0, 0.0, 0.0);
        assert!(w >= 1.0 && h >= 1.0);
    }

    #[test]
    fn absolute_pointer_adds_monitor_origin() {
        let left = mon("DP-1", 0, 0, 1920, 1080, true);
        let right = mon("DP-2", 1920, 0, 2560, 1440, false);
        let area = (1920.0, 1080.0);
        // Centre of the left window.
        let (x, y) = map_absolute(960.0, 540.0, area, (1920, 1080), Some(&left), false);
        assert!((x - 959.5).abs() < 1.0 && (y - 539.5).abs() < 1.0);
        // Top-left of the right monitor lands at its layout origin.
        let (x, y) = map_absolute(
            0.0,
            0.0,
            (2560.0, 1440.0),
            (2560, 1440),
            Some(&right),
            false,
        );
        assert_eq!((x, y), (1920.0, 0.0));
        // Bottom-right stays on the right monitor (last pixel, not the next).
        let (x, y) = map_absolute(
            2560.0,
            1440.0,
            (2560.0, 1440.0),
            (2560, 1440),
            Some(&right),
            false,
        );
        assert_eq!((x, y), (1920.0 + 2559.0, 1439.0));
    }

    #[test]
    fn absolute_pointer_uses_logical_not_physical_size() {
        // HiDPI: 3840x2160 video, 1920x1080 logical monitor at (100, 50).
        let hidpi = RudpMonitorInfo {
            scale: 2.0,
            ..mon("eDP-1", 100, 50, 1920, 1080, true)
        };
        let (x, y) = map_absolute(
            0.0,
            0.0,
            (1920.0, 1080.0),
            (3840, 2160),
            Some(&hidpi),
            false,
        );
        assert_eq!((x, y), (100.0, 50.0));
        let (x, _) = map_absolute(
            1920.0,
            0.0,
            (1920.0, 1080.0),
            (3840, 2160),
            Some(&hidpi),
            false,
        );
        assert_eq!(x, 100.0 + 1919.0);
    }

    #[test]
    fn absolute_pointer_without_layout_falls_back_to_video_size() {
        let (x, y) = map_absolute(0.0, 0.0, (960.0, 540.0), (1920, 1080), None, false);
        assert_eq!((x, y), (0.0, 0.0));
        let (x, y) = map_absolute(960.0, 540.0, (960.0, 540.0), (1920, 1080), None, false);
        assert_eq!((x, y), (1919.0, 1079.0));
    }

    #[test]
    fn pointer_ignores_letterbox_bars() {
        // Click inside the top bar clamps to the video's top edge.
        let m = mon("DP-1", 0, 0, 1920, 1080, true);
        let (_, y) = map_absolute(500.0, 10.0, (1000.0, 1000.0), (1920, 1080), Some(&m), false);
        assert_eq!(y, 0.0);
    }

    #[test]
    fn relative_pointer_scales_to_logical() {
        let m = mon("DP-1", 0, 0, 1920, 1080, true);
        let (dx, dy) = map_relative(10.0, -5.0, (960.0, 540.0), (1920, 1080), Some(&m), false);
        assert!((dx - 20.0).abs() < 1e-9 && (dy + 10.0).abs() < 1e-9);
    }

    #[test]
    fn monitor_choice_primary_sends_nothing() {
        let layout = vec![mon("DP-1", 0, 0, 1920, 1080, true)];
        assert_eq!(MonitorChoice::Primary.wanted(&layout), None);
    }

    #[test]
    fn monitor_choice_all_is_primary_first_and_capped() {
        let layout = vec![
            mon("A", 0, 0, 1, 1, false),
            mon("B", 1, 0, 1, 1, false),
            mon("C", 2, 0, 1, 1, true),
            mon("D", 3, 0, 1, 1, false),
            mon("E", 4, 0, 1, 1, false),
        ];
        let got = MonitorChoice::All.wanted(&layout).expect("some");
        assert_eq!(got.len(), RUDP_MAX_SUBSCRIBED_OUTPUTS);
        assert_eq!(got[0], "C", "primary window first");
    }

    #[test]
    fn monitor_choice_named_skips_unknown() {
        let layout = vec![
            mon("DP-1", 0, 0, 1, 1, true),
            mon("DP-2", 1, 0, 1, 1, false),
        ];
        let named = MonitorChoice::Named(vec!["DP-2".into(), "nope".into(), "DP-2".into()]);
        assert_eq!(named.wanted(&layout), Some(vec!["DP-2".to_string()]));
        let none = MonitorChoice::Named(vec!["nope".into()]);
        assert_eq!(none.wanted(&layout), None, "falls back to host primary");
    }

    #[test]
    fn monitor_choice_cli_beats_preference() {
        assert_eq!(MonitorChoice::from_cli(&[], false), None);
        assert_eq!(MonitorChoice::from_cli(&[], true), Some(MonitorChoice::All));
        assert_eq!(
            MonitorChoice::from_cli(&["DP-1".into()], false),
            Some(MonitorChoice::Named(vec!["DP-1".into()]))
        );
        assert_eq!(
            MonitorChoice::from_preference(false),
            MonitorChoice::Primary
        );
        assert_eq!(MonitorChoice::from_preference(true), MonitorChoice::All);
    }

    #[test]
    fn session_window_title_always_prefixes_metis_remote() {
        assert_eq!(
            rudp_session_window_title("", false),
            RUDP_SESSION_TITLE_PREFIX
        );
        assert_eq!(
            rudp_session_window_title("DP-1", false),
            RUDP_SESSION_TITLE_PREFIX
        );
        assert_eq!(
            rudp_session_window_title("DP-2", true),
            format!("{RUDP_SESSION_TITLE_PREFIX} — DP-2")
        );
        assert!(
            !rudp_session_window_title("DP-2", true).starts_with("DP-"),
            "must not use bare connector name"
        );
    }
}
