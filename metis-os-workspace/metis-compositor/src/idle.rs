//! Idle detection, screen blanking (DPMS), and idle inhibition.
//!
//! The compositor owns the display, so it is the only thing that can detect an
//! idle seat and power the panel down. This module ties together three inputs:
//!
//! * a blank timeout from `power.json` ([`metis_config::PowerConfig`]),
//! * the Wayland `zwp_idle_inhibit` protocol (native apps — video players,
//!   presentations — that mark a surface as "keep awake"), and
//! * external `org.freedesktop.ScreenSaver` inhibitors forwarded from the portal
//!   over IPC (Chromium/Electron/SDL games/browsers use this).
//!
//! While any inhibitor is held the blank timer is suspended and a blanked screen
//! is woken. Any input activity also wakes the screen and restarts the countdown.
//! `ext_idle_notify` clients (e.g. swayidle) are kept in sync via
//! [`IdleNotifierState`].

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::wayland::idle_inhibit::IdleInhibitHandler;
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};

use crate::state::MetisState;

/// Runtime idle/inhibit bookkeeping stored on [`MetisState`].
pub struct IdleManager {
    /// Blank the screen after this much inactivity. `None` = never blank.
    blank_after: Option<Duration>,
    /// Timestamp of the last observed input activity.
    last_activity: Instant,
    /// calloop token for the pending blank check, if one is armed.
    timer_token: Option<RegistrationToken>,
    /// wl_surfaces holding a `zwp_idle_inhibitor_v1`.
    wl_inhibitors: HashSet<WlSurface>,
    /// External inhibitor cookies (D-Bus `org.freedesktop.ScreenSaver`) → label.
    external_inhibitors: HashMap<u32, String>,
    /// Whether the outputs are currently powered down (DPMS off).
    blanked: bool,
    /// A `systemd-inhibit` child holding a logind `idle` lock while any inhibitor
    /// is active, so the session can't auto-suspend under a running game/video.
    /// Present exactly while [`Self::is_inhibited`] is true (best-effort — absent
    /// if `systemd-inhibit` is unavailable).
    suspend_inhibit: Option<std::process::Child>,
}

impl IdleManager {
    pub fn new(blank_after_minutes: u32) -> Self {
        Self {
            blank_after: minutes_to_duration(blank_after_minutes),
            last_activity: Instant::now(),
            timer_token: None,
            wl_inhibitors: HashSet::new(),
            external_inhibitors: HashMap::new(),
            blanked: false,
            suspend_inhibit: None,
        }
    }

    /// Start or stop the logind `idle` inhibitor to match the aggregate inhibit
    /// state. Holding a `--mode=block --what=idle` lock stops logind's automatic
    /// idle action (auto-suspend) while a game/media app keeps us awake, without
    /// blocking a manual suspend. Failure to spawn is non-fatal.
    fn reconcile_suspend_inhibitor(&mut self, inhibited: bool) {
        // Reap a child that exited on its own (e.g. `systemd-inhibit` missing).
        if let Some(child) = self.suspend_inhibit.as_mut()
            && matches!(child.try_wait(), Ok(Some(_)))
        {
            self.suspend_inhibit = None;
        }
        match (inhibited, self.suspend_inhibit.is_some()) {
            (true, false) => match std::process::Command::new("systemd-inhibit")
                .args([
                    "--what=idle",
                    "--who=Metis",
                    "--why=Application requested the screen stay awake",
                    "--mode=block",
                    "sleep",
                    "infinity",
                ])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(child) => {
                    tracing::info!("idle: holding logind idle inhibitor");
                    self.suspend_inhibit = Some(child);
                }
                Err(err) => {
                    tracing::debug!(
                        ?err,
                        "idle: systemd-inhibit unavailable; suspend not blocked"
                    )
                }
            },
            (false, true) => {
                if let Some(mut child) = self.suspend_inhibit.take() {
                    let _ = child.kill();
                    let _ = child.wait();
                    tracing::info!("idle: released logind idle inhibitor");
                }
            }
            _ => {}
        }
    }

    /// Update the blank timeout live (e.g. after the Settings power page changes
    /// `power.json`). The caller reschedules afterwards.
    pub fn set_blank_after_minutes(&mut self, minutes: u32) {
        self.blank_after = minutes_to_duration(minutes);
    }

    pub fn is_inhibited(&self) -> bool {
        !self.wl_inhibitors.is_empty() || !self.external_inhibitors.is_empty()
    }

    pub fn is_blanked(&self) -> bool {
        self.blanked
    }
}

fn minutes_to_duration(minutes: u32) -> Option<Duration> {
    (minutes > 0).then(|| Duration::from_secs(u64::from(minutes) * 60))
}

impl MetisState {
    /// Record input activity: wake a blanked screen and restart the countdown.
    /// Cheap enough to call on every input event — it only re-arms the calloop
    /// timer when one is not already pending (the timer self-corrects for the
    /// exact remaining time when it fires).
    pub fn idle_notify_activity(&mut self) {
        self.idle.last_activity = Instant::now();
        if self.idle.blanked {
            self.idle_set_blank(false);
        }
        // Keep ext-idle-notify clients (swayidle, etc.) consistent.
        let seat = self.seat.clone();
        self.idle_notifier_state.notify_activity(&seat);
        if !self.idle.is_inhibited() {
            self.idle_arm_timer_if_needed();
        }
    }

    /// Arm the blank timer for the full timeout, unless one is already pending,
    /// blanking is disabled, or an inhibitor is held.
    fn idle_arm_timer_if_needed(&mut self) {
        if self.idle.timer_token.is_some() {
            return;
        }
        let Some(after) = self.idle.blank_after else {
            return;
        };
        self.idle_insert_timer(after);
    }

    fn idle_insert_timer(&mut self, after: Duration) {
        let timer = Timer::from_duration(after);
        match self
            .loop_handle
            .insert_source(timer, |_, _, state: &mut MetisState| {
                state.idle_on_timeout();
                TimeoutAction::Drop
            }) {
            Ok(token) => self.idle.timer_token = Some(token),
            Err(err) => tracing::warn!(?err, "idle: failed to arm blank timer"),
        }
    }

    /// Cancel and (unless inhibited) re-arm the blank countdown from now. Used
    /// when inhibitors clear or the timeout preference changes.
    pub fn idle_reschedule(&mut self) {
        if let Some(token) = self.idle.timer_token.take() {
            self.loop_handle.remove(token);
        }
        self.idle.last_activity = Instant::now();
        if !self.idle.is_inhibited() {
            self.idle_arm_timer_if_needed();
        }
    }

    fn idle_on_timeout(&mut self) {
        self.idle.timer_token = None;
        if self.idle.is_inhibited() {
            return;
        }
        let Some(after) = self.idle.blank_after else {
            return;
        };
        let elapsed = self.idle.last_activity.elapsed();
        if elapsed >= after {
            self.idle_set_blank(true);
        } else {
            // Activity happened after the timer was armed; wait out the remainder.
            self.idle_insert_timer(after - elapsed);
        }
    }

    fn idle_set_blank(&mut self, blank: bool) {
        if self.idle.blanked == blank {
            return;
        }
        // Lock the session as it blanks, if configured, so waking the panel shows
        // the lock screen rather than the live desktop.
        if blank && self.lock_on_idle_blank() {
            self.lock_session();
        }
        self.idle.blanked = blank;
        self.set_outputs_dpms(!blank);
        if !blank {
            // Force a full repaint so the woken panel shows current content; the
            // heartbeat picks up `damaged` on its next tick.
            self.damaged = true;
        }
        tracing::info!(blanked = blank, "idle: screen blank state changed");
    }

    // --- Wayland zwp_idle_inhibit ---------------------------------------------

    fn idle_add_wl_inhibitor(&mut self, surface: WlSurface) {
        if self.idle.wl_inhibitors.insert(surface) {
            self.idle_inhibit_changed();
        }
    }

    fn idle_remove_wl_inhibitor(&mut self, surface: &WlSurface) {
        if self.idle.wl_inhibitors.remove(surface) {
            self.idle_inhibit_changed();
        }
    }

    // --- External (D-Bus org.freedesktop.ScreenSaver) inhibitors --------------

    /// Take an external idle inhibitor identified by `cookie`. Called from the
    /// IPC handler when the portal forwards a `Inhibit` request.
    pub fn idle_add_external_inhibitor(&mut self, cookie: u32, label: String) {
        let existed = self
            .idle
            .external_inhibitors
            .insert(cookie, label)
            .is_some();
        if !existed {
            tracing::info!(cookie, "idle: external inhibitor engaged");
            self.idle_inhibit_changed();
        }
    }

    /// Release an external idle inhibitor. Unknown cookies are ignored.
    pub fn idle_remove_external_inhibitor(&mut self, cookie: u32) {
        if self.idle.external_inhibitors.remove(&cookie).is_some() {
            tracing::info!(cookie, "idle: external inhibitor released");
            self.idle_inhibit_changed();
        }
    }

    fn idle_inhibit_changed(&mut self) {
        let inhibited = self.idle.is_inhibited();
        self.idle_notifier_state.set_is_inhibited(inhibited);
        self.idle.reconcile_suspend_inhibitor(inhibited);
        if inhibited {
            if let Some(token) = self.idle.timer_token.take() {
                self.loop_handle.remove(token);
            }
            if self.idle.blanked {
                self.idle_set_blank(false);
            }
        } else {
            self.idle_reschedule();
        }
    }
}

impl IdleInhibitHandler for MetisState {
    fn inhibit(&mut self, surface: WlSurface) {
        self.idle_add_wl_inhibitor(surface);
    }

    fn uninhibit(&mut self, surface: WlSurface) {
        self.idle_remove_wl_inhibitor(&surface);
    }
}

impl IdleNotifierHandler for MetisState {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_notifier_state
    }
}
