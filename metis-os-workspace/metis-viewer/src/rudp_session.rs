//! Metis Remote (RUDP) session window — decode + present + input.

use std::cell::Cell;
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};

use gtk::gdk;
use gtk::glib;
use gtk::prelude::*;
use metis_decode::{RudpCodec, open_decoder};
use metis_i18n::tr;
use metis_protocol::RudpControlMsg;
use metis_rudp_client::{
    ClientError, RudpClientConfig, SessionEvent, TofuMode, clear_host_pin, connect, pin_host,
};
use zeroize::Zeroize;

enum ToGtk {
    Connected,
    Event(SessionEvent),
    Failed(ConnectFail),
}

enum ToWorker {
    Input(RudpControlMsg),
    Shutdown,
}

enum ConnectFail {
    Message(String),
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

/// Open an RUDP session: TOFU (interactive), decode, present, send input.
pub fn open_rudp_session(
    parent: &impl IsA<gtk::Window>,
    host: String,
    port: u16,
    username: String,
    mut password: String,
) {
    let host_key = format!("{host}:{port}");
    let addr: SocketAddr = match format!("{host}:{port}").parse() {
        Ok(a) => a,
        Err(e) => {
            show_alert(parent, &format!("{}: {e}", tr("Invalid host address")));
            password.zeroize();
            return;
        }
    };

    let (gtk_tx, gtk_rx) = mpsc::channel::<ToGtk>();
    let (worker_tx, worker_rx) = mpsc::channel::<ToWorker>();
    let pass = password.clone();
    password.zeroize();
    let host_key_worker = host_key.clone();

    std::thread::Builder::new()
        .name("metis-viewer-rudp".into())
        .spawn(move || {
            worker_main(addr, host_key_worker, username, pass, gtk_tx, worker_rx);
        })
        .ok();

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
                    build_session_window(&parent, worker_tx.clone(), gtk_rx.clone());
                }
                // Session window owns further event polling.
                glib::ControlFlow::Break
            }
            Ok(ToGtk::Failed(fail)) => {
                drop(rx);
                handle_connect_fail(&parent, fail);
                glib::ControlFlow::Break
            }
            Ok(ToGtk::Event(_)) => glib::ControlFlow::Continue,
            Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
        }
    });
}

fn worker_main(
    addr: SocketAddr,
    host_key: String,
    username: String,
    mut password: String,
    gtk_tx: Sender<ToGtk>,
    worker_rx: Receiver<ToWorker>,
) {
    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = gtk_tx.send(ToGtk::Failed(ConnectFail::Message(e.to_string())));
            return;
        }
    };

    let cfg = RudpClientConfig {
        addr,
        host_key: host_key.clone(),
        username,
        password: password.clone(),
        tofu: TofuMode::Interactive,
    };
    password.zeroize();

    let mut session = match rt.block_on(connect(cfg)) {
        Ok(s) => s,
        Err(ClientError::TofuUnknown { fingerprint }) => {
            let _ = gtk_tx.send(ToGtk::Failed(ConnectFail::TofuUnknown {
                fingerprint,
                host_key,
            }));
            return;
        }
        Err(ClientError::TofuMismatch { got, pinned }) => {
            let _ = gtk_tx.send(ToGtk::Failed(ConnectFail::TofuMismatch {
                got,
                pinned,
                host_key,
            }));
            return;
        }
        Err(e) => {
            let _ = gtk_tx.send(ToGtk::Failed(ConnectFail::Message(e.to_string())));
            return;
        }
    };

    let _ = gtk_tx.send(ToGtk::Connected);
    println_session_ok(session.session_id());

    loop {
        // Drain input without blocking the event pump too long.
        while let Ok(msg) = worker_rx.try_recv() {
            match msg {
                ToWorker::Input(m) => {
                    let _ = rt.block_on(session.send_input(m));
                }
                ToWorker::Shutdown => return,
            }
        }
        match rt.block_on(async {
            tokio::select! {
                biased;
                ev = session.recv_event() => ev,
                _ = tokio::time::sleep(std::time::Duration::from_millis(5)) => None,
            }
        }) {
            Some(ev) => {
                let done = matches!(ev, SessionEvent::Disconnected);
                if gtk_tx.send(ToGtk::Event(ev)).is_err() {
                    return;
                }
                if done {
                    return;
                }
            }
            None => {
                if worker_rx.try_recv().ok().is_none() {
                    // idle
                }
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

fn println_session_ok(id: &str) {
    tracing::info!(%id, "metis-viewer: RUDP SessionOk");
}

fn handle_connect_fail(parent: &gtk::Window, fail: ConnectFail) {
    match fail {
        ConnectFail::Message(m) => show_alert(parent, &m),
        ConnectFail::TofuUnknown {
            fingerprint,
            host_key,
        } => {
            let parent = parent.clone();
            glib::spawn_future_local(async move {
                if trust_dialog(&parent, &fingerprint).await
                    && pin_host(&host_key, &fingerprint).is_ok()
                {
                    show_alert(
                        &parent,
                        &tr("Host trusted. Connect again to open the session."),
                    );
                }
            });
        }
        ConnectFail::TofuMismatch {
            got,
            pinned,
            host_key,
        } => {
            let parent = parent.clone();
            glib::spawn_future_local(async move {
                if clear_pin_dialog(&parent, &got, &pinned).await {
                    let _ = clear_host_pin(&host_key);
                    show_alert(
                        &parent,
                        &tr("Pin cleared. Connect again to trust the new fingerprint."),
                    );
                }
            });
        }
    }
}

fn build_session_window(
    parent: &gtk::Window,
    worker_tx: Rc<Sender<ToWorker>>,
    gtk_rx: Rc<Mutex<Receiver<ToGtk>>>,
) {
    let win = gtk::Window::builder()
        .transient_for(parent)
        .title(tr("Metis Remote"))
        .default_width(1280)
        .default_height(720)
        .modal(false)
        .build();
    win.add_css_class("metis-viewer-window");

    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_hexpand(true);
    picture.set_vexpand(true);
    picture.set_can_focus(true);

    let overlay = gtk::Overlay::new();
    overlay.set_child(Some(&picture));
    let status = gtk::Label::new(Some(&tr("Waiting for video…")));
    status.set_halign(gtk::Align::Start);
    status.set_valign(gtk::Align::Start);
    status.set_margin_start(12);
    status.set_margin_top(8);
    overlay.add_overlay(&status);
    win.set_child(Some(&overlay));
    win.present();
    picture.grab_focus();

    let pointer_locked = Rc::new(Cell::new(false));
    let video_size = Rc::new(Cell::new((0u32, 0u32)));
    let last_pos = Rc::new(Cell::new((0.0f64, 0.0f64)));
    let decoder: Rc<Mutex<Option<Box<dyn metis_decode::VideoDecoder>>>> = Rc::new(Mutex::new(None));

    {
        let worker_tx = worker_tx.clone();
        win.connect_close_request(move |_| {
            let _ = worker_tx.send(ToWorker::Shutdown);
            glib::Propagation::Proceed
        });
    }

    {
        let picture = picture.clone();
        let status = status.clone();
        let pointer_locked = pointer_locked.clone();
        let video_size = video_size.clone();
        let decoder = decoder.clone();
        let win = win.clone();
        glib::timeout_add_local(std::time::Duration::from_millis(8), move || {
            let Ok(rx) = gtk_rx.lock() else {
                return glib::ControlFlow::Continue;
            };
            for _ in 0..16 {
                match rx.try_recv() {
                    Ok(ToGtk::Event(ev)) => {
                        if matches!(ev, SessionEvent::Disconnected) {
                            status.set_text(&tr("Disconnected"));
                            return glib::ControlFlow::Break;
                        }
                        apply_event(
                            &ev,
                            &picture,
                            &status,
                            &pointer_locked,
                            &video_size,
                            &decoder,
                        );
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        status.set_text(&tr("Disconnected"));
                        return glib::ControlFlow::Break;
                    }
                }
            }
            if !win.is_visible() {
                return glib::ControlFlow::Break;
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let worker_tx = worker_tx.clone();
        let pointer_locked = pointer_locked.clone();
        let video_size = video_size.clone();
        let last_pos = last_pos.clone();
        let picture_motion = picture.clone();
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(move |_, x, y| {
            let (vw, vh) = video_size.get();
            if vw == 0 || vh == 0 {
                return;
            }
            let aw = picture_motion.width().max(1) as f64;
            let ah = picture_motion.height().max(1) as f64;
            if pointer_locked.get() {
                let (lx, ly) = last_pos.get();
                let dx = (x - lx) * (vw as f64 / aw);
                let dy = (y - ly) * (vh as f64 / ah);
                last_pos.set((x, y));
                if dx != 0.0 || dy != 0.0 {
                    let _ =
                        worker_tx.send(ToWorker::Input(RudpControlMsg::PointerRelative { dx, dy }));
                }
            } else {
                last_pos.set((x, y));
                let _ = worker_tx.send(ToWorker::Input(RudpControlMsg::PointerAbsolute {
                    x: (x / aw) * vw as f64,
                    y: (y / ah) * vh as f64,
                }));
            }
        });
        picture.add_controller(motion);
    }

    {
        let click = gtk::GestureClick::new();
        click.set_button(0);
        let worker_tx_press = worker_tx.clone();
        click.connect_pressed(move |g, _, _, _| {
            let _ = worker_tx_press.send(ToWorker::Input(RudpControlMsg::PointerButton {
                button: map_gdk_button(g.current_button()),
                pressed: true,
            }));
        });
        let worker_tx_rel = worker_tx.clone();
        click.connect_released(move |g, _, _, _| {
            let _ = worker_tx_rel.send(ToWorker::Input(RudpControlMsg::PointerButton {
                button: map_gdk_button(g.current_button()),
                pressed: false,
            }));
        });
        picture.add_controller(click);
    }

    {
        let worker_tx = worker_tx.clone();
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        scroll.connect_scroll(move |_, dx, dy| {
            let _ = worker_tx.send(ToWorker::Input(RudpControlMsg::PointerScroll {
                dx,
                dy: -dy,
            }));
            glib::Propagation::Stop
        });
        picture.add_controller(scroll);
    }

    {
        let keys = gtk::EventControllerKey::new();
        let worker_tx_down = worker_tx.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if let Some(evdev) = gdk_key_to_evdev(key) {
                let _ = worker_tx_down.send(ToWorker::Input(RudpControlMsg::Key {
                    keycode: evdev,
                    pressed: true,
                }));
            }
            glib::Propagation::Proceed
        });
        let worker_tx_up = worker_tx.clone();
        keys.connect_key_released(move |_, key, _, _| {
            if let Some(evdev) = gdk_key_to_evdev(key) {
                let _ = worker_tx_up.send(ToWorker::Input(RudpControlMsg::Key {
                    keycode: evdev,
                    pressed: false,
                }));
            }
        });
        picture.add_controller(keys);
    }
}

fn apply_event(
    ev: &SessionEvent,
    picture: &gtk::Picture,
    status: &gtk::Label,
    pointer_locked: &Rc<Cell<bool>>,
    video_size: &Rc<Cell<(u32, u32)>>,
    decoder: &Rc<Mutex<Option<Box<dyn metis_decode::VideoDecoder>>>>,
) {
    match ev {
        SessionEvent::VideoReady {
            width,
            height,
            codec,
        } => {
            video_size.set((*width, *height));
            status.set_text(&format!("{width}×{height} {codec}"));
            let codec = match codec.as_str() {
                "h264" => RudpCodec::H264,
                _ => RudpCodec::Hevc,
            };
            if let Ok(dec) = open_decoder(codec, *width, *height)
                && let Ok(mut g) = decoder.lock()
            {
                *g = Some(dec);
            }
        }
        SessionEvent::PointerLock { locked } => {
            pointer_locked.set(*locked);
            if *locked {
                status.set_text(&tr("Pointer lock — relative mouse"));
            }
        }
        SessionEvent::AccessUnit(au) => {
            let Ok(mut g) = decoder.lock() else {
                return;
            };
            if g.is_none() {
                let (w, h) = video_size.get();
                if w == 0 {
                    return;
                }
                let codec = match au.codec.as_str() {
                    "h264" => RudpCodec::H264,
                    _ => RudpCodec::Hevc,
                };
                if let Ok(d) = open_decoder(codec, w, h) {
                    *g = Some(d);
                } else {
                    return;
                }
            }
            if let Some(dec) = g.as_mut()
                && let Ok(Some(frame)) = dec.push_au(&au.data)
            {
                video_size.set((frame.width, frame.height));
                present_rgba(picture, &frame.rgba, frame.width, frame.height);
            }
        }
        SessionEvent::Disconnected => status.set_text(&tr("Disconnected")),
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

fn show_alert(parent: &impl IsA<gtk::Window>, msg: &str) {
    let d = gtk::AlertDialog::builder()
        .modal(true)
        .message(tr("Metis Remote"))
        .detail(msg)
        .buttons([tr("OK")])
        .build();
    d.show(Some(parent));
}
