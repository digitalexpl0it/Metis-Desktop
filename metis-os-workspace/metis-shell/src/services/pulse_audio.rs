//! Default sink/source volume via libpulse (no `pactl` forks on the bar path).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use libpulse_binding::callbacks::ListResult;
use libpulse_binding::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use libpulse_binding::mainloop::standard::{IterateResult, Mainloop};
use libpulse_binding::volume::{ChannelVolumes, Volume};

static AUDIO_DIRTY: AtomicBool = AtomicBool::new(true);
static PULSE_WARNED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, Default)]
pub struct VolumeSnapshot {
    pub sink_percent: Option<u8>,
    pub sink_muted: Option<bool>,
    pub source_percent: Option<u8>,
    pub source_muted: Option<bool>,
}

enum Cmd {
    Snapshot(Sender<VolumeSnapshot>),
    SetSinkVolumeAbs(u8),
    SetSinkVolumeRel(i8),
    SetSinkMute(bool),
    SetSourceVolumeAbs(u8),
    SetSourceMute(bool),
}

static TX: OnceLock<Mutex<Option<Sender<Cmd>>>> = OnceLock::new();

fn cmd_tx() -> Option<Sender<Cmd>> {
    ensure_worker();
    TX.get().and_then(|m| m.lock().ok().and_then(|g| g.clone()))
}

fn ensure_worker() {
    let slot = TX.get_or_init(|| Mutex::new(None));
    let mut guard = match slot.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        return;
    }
    let (tx, rx) = mpsc::channel::<Cmd>();
    *guard = Some(tx);
    drop(guard);
    if let Err(err) = thread::Builder::new()
        .name("metis-pulse".into())
        .spawn(move || worker(rx))
    {
        tracing::warn!(%err, "pulse: failed to spawn worker");
        if let Ok(mut g) = slot.lock() {
            *g = None;
        }
    }
}

pub fn take_dirty() -> bool {
    ensure_worker();
    AUDIO_DIRTY.swap(false, Ordering::Relaxed)
}

pub fn mark_dirty() {
    AUDIO_DIRTY.store(true, Ordering::Relaxed);
}

pub fn snapshot() -> VolumeSnapshot {
    let Some(tx) = cmd_tx() else {
        return VolumeSnapshot::default();
    };
    let (reply_tx, reply_rx) = mpsc::channel();
    if tx.send(Cmd::Snapshot(reply_tx)).is_err() {
        return VolumeSnapshot::default();
    }
    reply_rx
        .recv_timeout(Duration::from_millis(400))
        .unwrap_or_default()
}

pub fn set_sink_volume_absolute(percent: u8) {
    if let Some(tx) = cmd_tx() {
        let _ = tx.send(Cmd::SetSinkVolumeAbs(percent.min(100)));
        mark_dirty();
    }
}

pub fn set_sink_volume_relative(delta: i8) {
    if let Some(tx) = cmd_tx() {
        let _ = tx.send(Cmd::SetSinkVolumeRel(delta));
        mark_dirty();
    }
}

pub fn set_sink_mute(muted: bool) {
    if let Some(tx) = cmd_tx() {
        let _ = tx.send(Cmd::SetSinkMute(muted));
        mark_dirty();
    }
}

pub fn set_source_volume_absolute(percent: u8) {
    if let Some(tx) = cmd_tx() {
        let _ = tx.send(Cmd::SetSourceVolumeAbs(percent.min(100)));
        mark_dirty();
    }
}

pub fn set_source_mute(muted: bool) {
    if let Some(tx) = cmd_tx() {
        let _ = tx.send(Cmd::SetSourceMute(muted));
        mark_dirty();
    }
}

fn worker(rx: Receiver<Cmd>) {
    let Some(mut ml) = Mainloop::new() else {
        warn_once("pulse: Mainloop::new failed");
        return;
    };
    let Some(mut ctx) = Context::new(&ml, "metis-shell") else {
        warn_once("pulse: Context::new failed");
        return;
    };
    if ctx.connect(None, ContextFlagSet::NOFLAGS, None).is_err() {
        warn_once("pulse: Context::connect failed");
        return;
    }

    loop {
        match ml.iterate(false) {
            IterateResult::Quit(_) | IterateResult::Err(_) => {
                warn_once("pulse: mainloop iterate failed during connect");
                return;
            }
            IterateResult::Success(_) => {}
        }
        match ctx.get_state() {
            ContextState::Ready => break,
            ContextState::Failed | ContextState::Terminated => {
                warn_once("pulse: context failed/terminated");
                return;
            }
            _ => thread::sleep(Duration::from_millis(5)),
        }
    }

    let _ = ctx.subscribe(
        libpulse_binding::context::subscribe::InterestMaskSet::SINK
            | libpulse_binding::context::subscribe::InterestMaskSet::SOURCE
            | libpulse_binding::context::subscribe::InterestMaskSet::SERVER,
        |_| {},
    );
    ctx.set_subscribe_callback(Some(Box::new(|_, _, _| {
        AUDIO_DIRTY.store(true, Ordering::Relaxed);
    })));

    let ml = Rc::new(RefCell::new(ml));
    let ctx = Rc::new(RefCell::new(ctx));
    let cache = Rc::new(RefCell::new(VolumeSnapshot::default()));

    loop {
        while let Ok(cmd) = rx.try_recv() {
            match cmd {
                Cmd::Snapshot(reply) => {
                    refresh_snapshot(&ml, &ctx, &cache);
                    let _ = reply.send(*cache.borrow());
                }
                Cmd::SetSinkVolumeAbs(p) => {
                    mutate_default_sink_volume(&ml, &ctx, move |vols| {
                        set_all_channels_percent(vols, p);
                    });
                }
                Cmd::SetSinkVolumeRel(d) => {
                    mutate_default_sink_volume(&ml, &ctx, move |vols| {
                        let cur = volume_percent(vols);
                        let next = (i16::from(cur) + i16::from(d)).clamp(0, 100) as u8;
                        set_all_channels_percent(vols, next);
                    });
                }
                Cmd::SetSinkMute(m) => mutate_default_sink_mute(&ml, &ctx, m),
                Cmd::SetSourceVolumeAbs(p) => {
                    mutate_default_source_volume(&ml, &ctx, move |vols| {
                        set_all_channels_percent(vols, p);
                    });
                }
                Cmd::SetSourceMute(m) => mutate_default_source_mute(&ml, &ctx, m),
            }
        }
        match ml.borrow_mut().iterate(false) {
            IterateResult::Quit(_) | IterateResult::Err(_) => break,
            IterateResult::Success(_) => {}
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn warn_once(msg: &str) {
    if !PULSE_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!("{msg}");
    }
}

fn volume_percent(vols: &ChannelVolumes) -> u8 {
    let avg = vols.avg().0;
    ((u64::from(avg) * 100) / u64::from(Volume::NORMAL.0)).min(100) as u8
}

fn set_all_channels_percent(vols: &mut ChannelVolumes, percent: u8) {
    let v = Volume((Volume::NORMAL.0 * u32::from(percent.min(100))) / 100);
    vols.set(vols.len(), v);
}

fn pump(ml: &Rc<RefCell<Mainloop>>, done: &Rc<RefCell<bool>>) {
    let start = std::time::Instant::now();
    while !*done.borrow() && start.elapsed() < Duration::from_millis(350) {
        match ml.borrow_mut().iterate(false) {
            IterateResult::Quit(_) | IterateResult::Err(_) => break,
            IterateResult::Success(_) => {}
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn refresh_snapshot(
    ml: &Rc<RefCell<Mainloop>>,
    ctx: &Rc<RefCell<Context>>,
    cache: &Rc<RefCell<VolumeSnapshot>>,
) {
    let done = Rc::new(RefCell::new(0u8));
    {
        let cache_s = cache.clone();
        let done_s = done.clone();
        ctx.borrow_mut()
            .introspect()
            .get_sink_info_by_name("@DEFAULT_SINK@", move |res| {
                if let ListResult::Item(info) = res {
                    let mut c = cache_s.borrow_mut();
                    c.sink_percent = Some(volume_percent(&info.volume));
                    c.sink_muted = Some(info.mute);
                }
                if matches!(res, ListResult::End | ListResult::Error) {
                    *done_s.borrow_mut() |= 1;
                }
            });
    }
    {
        let cache_s = cache.clone();
        let done_s = done.clone();
        ctx.borrow_mut()
            .introspect()
            .get_source_info_by_name("@DEFAULT_SOURCE@", move |res| {
                if let ListResult::Item(info) = res {
                    let mut c = cache_s.borrow_mut();
                    c.source_percent = Some(volume_percent(&info.volume));
                    c.source_muted = Some(info.mute);
                }
                if matches!(res, ListResult::End | ListResult::Error) {
                    *done_s.borrow_mut() |= 2;
                }
            });
    }
    let start = std::time::Instant::now();
    while *done.borrow() != 3 && start.elapsed() < Duration::from_millis(350) {
        match ml.borrow_mut().iterate(false) {
            IterateResult::Quit(_) | IterateResult::Err(_) => break,
            IterateResult::Success(_) => {}
        }
        thread::sleep(Duration::from_millis(2));
    }
}

fn mutate_default_sink_volume(
    ml: &Rc<RefCell<Mainloop>>,
    ctx: &Rc<RefCell<Context>>,
    edit: impl FnOnce(&mut ChannelVolumes) + 'static,
) {
    let done = Rc::new(RefCell::new(false));
    let done2 = done.clone();
    let ctx2 = ctx.clone();
    let edit = RefCell::new(Some(edit));
    ctx.borrow_mut()
        .introspect()
        .get_sink_info_by_name("@DEFAULT_SINK@", move |res| {
            if let ListResult::Item(info) = res
                && let Some(edit) = edit.borrow_mut().take()
            {
                let mut vols = info.volume;
                edit(&mut vols);
                let _ = ctx2
                    .borrow_mut()
                    .introspect()
                    .set_sink_volume_by_index(info.index, &vols, None);
            }
            if matches!(res, ListResult::End | ListResult::Error) {
                *done2.borrow_mut() = true;
            }
        });
    pump(ml, &done);
}

fn mutate_default_sink_mute(ml: &Rc<RefCell<Mainloop>>, ctx: &Rc<RefCell<Context>>, muted: bool) {
    let done = Rc::new(RefCell::new(false));
    let done2 = done.clone();
    let ctx2 = ctx.clone();
    ctx.borrow_mut()
        .introspect()
        .get_sink_info_by_name("@DEFAULT_SINK@", move |res| {
            if let ListResult::Item(info) = res {
                let _ = ctx2
                    .borrow_mut()
                    .introspect()
                    .set_sink_mute_by_index(info.index, muted, None);
            }
            if matches!(res, ListResult::End | ListResult::Error) {
                *done2.borrow_mut() = true;
            }
        });
    pump(ml, &done);
}

fn mutate_default_source_volume(
    ml: &Rc<RefCell<Mainloop>>,
    ctx: &Rc<RefCell<Context>>,
    edit: impl FnOnce(&mut ChannelVolumes) + 'static,
) {
    let done = Rc::new(RefCell::new(false));
    let done2 = done.clone();
    let ctx2 = ctx.clone();
    let edit = RefCell::new(Some(edit));
    ctx.borrow_mut()
        .introspect()
        .get_source_info_by_name("@DEFAULT_SOURCE@", move |res| {
            if let ListResult::Item(info) = res
                && let Some(edit) = edit.borrow_mut().take()
            {
                let mut vols = info.volume;
                edit(&mut vols);
                let _ = ctx2
                    .borrow_mut()
                    .introspect()
                    .set_source_volume_by_index(info.index, &vols, None);
            }
            if matches!(res, ListResult::End | ListResult::Error) {
                *done2.borrow_mut() = true;
            }
        });
    pump(ml, &done);
}

fn mutate_default_source_mute(ml: &Rc<RefCell<Mainloop>>, ctx: &Rc<RefCell<Context>>, muted: bool) {
    let done = Rc::new(RefCell::new(false));
    let done2 = done.clone();
    let ctx2 = ctx.clone();
    ctx.borrow_mut()
        .introspect()
        .get_source_info_by_name("@DEFAULT_SOURCE@", move |res| {
            if let ListResult::Item(info) = res {
                let _ = ctx2
                    .borrow_mut()
                    .introspect()
                    .set_source_mute_by_index(info.index, muted, None);
            }
            if matches!(res, ListResult::End | ListResult::Error) {
                *done2.borrow_mut() = true;
            }
        });
    pump(ml, &done);
}
