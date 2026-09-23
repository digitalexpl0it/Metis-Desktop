//! Compositor-rendered session lock (Option A).
//!
//! When locked, the compositor stops rendering and delivering input to clients
//! entirely. It draws its own lock UI (background + optional blur/dim + clock +
//! password field) using the same `fontdue` text pipeline the window
//! decorations use, and captures every key into a password buffer. Enter runs
//! PAM authentication on a worker thread (PAM blocks), and the boolean result is
//! marshaled back onto the event loop through a [`calloop`] channel so the
//! unlock / error handling runs on the main thread.
//!
//! Security posture while locked: no client is rendered or sent input, the
//! keyboard focus is cleared, screen capture is refused, and the typed password
//! is zeroized after every attempt and never logged.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::os::raw::{c_char, c_int, c_void};
use std::time::Duration;

use fontdue::Font;
use metis_config::{GradientDirection, LockBackgroundSource, LockConfig};
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::solid::SolidColorRenderElement;
use smithay::backend::renderer::element::texture::{TextureBuffer, TextureRenderElement};
use smithay::backend::renderer::element::{Id, Kind};
use smithay::backend::renderer::gles::{GlesRenderer, GlesTexture};
use smithay::backend::renderer::utils::CommitCounter;
use smithay::backend::renderer::{Color32F, ImportMem, Texture};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::calloop::channel::{Channel, Event, Sender, channel};
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::utils::{Buffer, Logical, Physical, Point, Rectangle, Size, Transform};

use crate::focus::KeyboardFocusTarget;
use crate::lock_auth_cues::{AuthCues, detect_auth_cues};
use crate::night_light::{RenderTargetInfo, premultiply};
use crate::render::OutputStack;
use crate::state::MetisState;

/// Fixed Gaussian radius (in texels) used for the frosted lock background — a
/// good deal heavier than the bar's backdrop blur.
const LOCK_BLUR_RADIUS: f32 = 28.0;

/// Result of one PAM attempt, marshaled from the worker thread back to the loop.
pub struct AuthOutcome {
    /// The attempt this result belongs to; stale results (the user typed a new
    /// password meanwhile) are ignored.
    generation: u64,
    success: bool,
}

/// A rasterized text line cached as a GPU texture keyed by its content+style.
struct CachedText {
    buffer: TextureBuffer<GlesTexture>,
    w: i32,
    h: i32,
}

/// Runtime lock-screen state stored on [`MetisState`].
pub struct LockState {
    pub locked: bool,
    cfg: LockConfig,
    /// The typed password. Zeroized whenever it is cleared.
    password: String,
    /// Human-readable error/status shown under the field (never the password).
    status: Option<String>,
    auth_in_flight: bool,
    attempts: u32,
    auth_generation: u64,
    auth_tx: Sender<AuthOutcome>,
    /// Taken once at startup and registered with the event loop.
    auth_rx: Option<Channel<AuthOutcome>>,
    /// Soft-fail fingerprint / YubiKey presence (UI cue only).
    auth_cues: AuthCues,
    /// One empty-password PAM auto-attempt already fired this lock.
    biometric_auto_tried: bool,
    /// 1 Hz repaint timer so the clock stays current while locked.
    clock_timer: Option<RegistrationToken>,
    /// Password caret visibility; flipped by [`MetisState::lock_arm_clock_timer`]
    /// so blink rate is stable even when the session is otherwise idle (wall-clock
    /// sampling at the same 500 ms cadence aliases into a harsh strobe).
    caret_on: bool,
    font: Option<Font>,
    font_data: Option<Vec<u8>>,
    font_loaded: bool,
    /// Decoded background base textures keyed by `(width, height, config-sig)`.
    /// The raw texture is kept alongside the buffer so the blur shader can sample
    /// it (a [`TextureBuffer`] hides its underlying texture).
    bg_cache: HashMap<(i32, i32, u64), (GlesTexture, TextureBuffer<GlesTexture>)>,
    /// Rasterized text lines keyed by a content+style hash.
    text_cache: HashMap<u64, CachedText>,
    dim_id: Id,
    dim_commit: CommitCounter,
    solid_id: Id,
    solid_commit: CommitCounter,
    /// Index into [`POWER_ORDER`] of the power button under the pointer, if any.
    hovered_power: Option<usize>,
}

impl LockState {
    pub fn new() -> Self {
        let (auth_tx, auth_rx) = channel::<AuthOutcome>();
        Self {
            locked: false,
            cfg: metis_config::load_lock_config(),
            password: String::new(),
            status: None,
            auth_in_flight: false,
            attempts: 0,
            auth_generation: 0,
            auth_tx,
            auth_rx: Some(auth_rx),
            auth_cues: AuthCues::default(),
            biometric_auto_tried: false,
            clock_timer: None,
            caret_on: true,
            font: None,
            font_data: None,
            font_loaded: false,
            bg_cache: HashMap::new(),
            text_cache: HashMap::new(),
            dim_id: Id::new(),
            dim_commit: CommitCounter::default(),
            solid_id: Id::new(),
            solid_commit: CommitCounter::default(),
            hovered_power: None,
        }
    }

    /// Zeroize and clear the password buffer.
    fn clear_password(&mut self) {
        use zeroize::Zeroize;
        self.password.zeroize();
        self.password.clear();
    }

    /// Drop cached GPU resources (on unlock, config reload, or locale change).
    pub(crate) fn clear_gpu_cache(&mut self) {
        self.bg_cache.clear();
        self.text_cache.clear();
    }

    /// A hash of only the *background-appearance* config (not dim/clock), used as
    /// part of the background texture cache key so a settings change re-decodes.
    fn bg_signature(&self) -> u64 {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        match self.cfg.background {
            LockBackgroundSource::Wallpaper => 0u8.hash(&mut h),
            LockBackgroundSource::Picture => {
                1u8.hash(&mut h);
                self.cfg.picture_path.hash(&mut h);
            }
            LockBackgroundSource::Solid => {
                2u8.hash(&mut h);
                self.cfg.color.hash(&mut h);
            }
            LockBackgroundSource::Gradient => {
                3u8.hash(&mut h);
                self.cfg.gradient_start.hash(&mut h);
                self.cfg.gradient_end.hash(&mut h);
                (self.cfg.gradient_direction as u8).hash(&mut h);
            }
        }
        h.finish()
    }
}

impl Default for LockState {
    fn default() -> Self {
        Self::new()
    }
}

impl MetisState {
    /// Register the PAM result channel with the event loop. Called once at
    /// startup (after the loop exists) — the receiver is consumed here.
    pub fn lock_register_auth_channel(&mut self) {
        let Some(rx) = self.lock.auth_rx.take() else {
            return;
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(rx, |event, _, state: &mut MetisState| {
                if let Event::Msg(outcome) = event {
                    state.lock_on_auth_result(outcome);
                }
            })
        {
            tracing::warn!(?err, "lock: failed to register PAM result channel");
        }
    }

    /// Enter the locked state: draw the lock UI, clear keyboard focus, and start
    /// the clock repaint timer. Idempotent.
    pub fn lock_session(&mut self) {
        if self.lock.locked {
            return;
        }
        if self.protocol_lock.is_locked() {
            tracing::info!("refusing Metis PAM lock while a protocol locker is active");
            return;
        }
        // Re-read the config on every lock so appearance edits apply next time.
        self.lock.cfg = metis_config::load_lock_config();
        self.lock.locked = true;
        self.lock.clear_password();
        self.lock.auth_cues = detect_auth_cues();
        self.lock.biometric_auto_tried = false;
        self.lock.status = lock_cue_status(self.lock.auth_cues);
        self.lock.attempts = 0;
        self.lock.hovered_power = None;
        self.lock.caret_on = true;
        self.lock.clear_gpu_cache();

        // Clear keyboard focus so no client receives further keys.
        if let Some(keyboard) = self.seat.get_keyboard() {
            let serial = smithay::utils::SERIAL_COUNTER.next_serial();
            keyboard.set_focus(self, Option::<KeyboardFocusTarget>::None, serial);
        }

        self.lock_arm_clock_timer();
        self.lock_arm_biometric_auto();
        tracing::info!(
            fingerprint = self.lock.auth_cues.fingerprint,
            security_key = self.lock.auth_cues.security_key,
            "lock: session locked"
        );
        // Pause RDP listen without clearing remote.json.enabled. Capture/inject
        // denials remain belt-and-suspenders while locked.
        spawn_metis_remote(&["pause"]);
        self.damaged = true;
        self.request_redraw();
    }

    /// Leave the locked state and repaint the live desktop.
    pub fn unlock_session(&mut self) {
        if !self.lock.locked {
            return;
        }
        self.lock.locked = false;
        self.lock.clear_password();
        self.lock.status = None;
        self.lock.auth_cues = AuthCues::default();
        self.lock.biometric_auto_tried = false;
        self.lock.clear_gpu_cache();
        if let Some(token) = self.lock.clock_timer.take() {
            self.loop_handle.remove(token);
        }
        tracing::info!("lock: session unlocked");
        // Resume RDP only when remote.json still wants sharing.
        spawn_metis_remote(&["resume"]);
        self.damaged = true;
        self.request_redraw();
    }

    /// Re-read `lock.json` and, if locked, re-decode the background live.
    pub fn lock_reload(&mut self) {
        self.lock.cfg = metis_config::load_lock_config();
        if self.lock.locked {
            self.lock.clear_gpu_cache();
            self.damaged = true;
            self.request_redraw();
        }
    }

    /// Whether idle-blank should also lock the session.
    pub fn lock_on_idle_blank(&self) -> bool {
        self.lock.cfg.lock_on_idle_blank
    }

    fn lock_arm_clock_timer(&mut self) {
        if self.lock.clock_timer.is_some() {
            return;
        }
        // Repaint at 2 Hz: clock text caches by content, caret toggles here so
        // blink stays a clean 1 Hz even when nothing else is damaging frames.
        let tick = Duration::from_millis(500);
        match self.loop_handle.insert_source(
            Timer::from_duration(tick),
            move |_, _, state: &mut MetisState| {
                if state.lock.locked {
                    state.lock.caret_on = !state.lock.caret_on;
                    state.damaged = true;
                    state.request_redraw();
                    TimeoutAction::ToDuration(tick)
                } else {
                    state.lock.clock_timer = None;
                    TimeoutAction::Drop
                }
            },
        ) {
            Ok(token) => self.lock.clock_timer = Some(token),
            Err(err) => tracing::warn!(?err, "lock: failed to arm clock timer"),
        }
    }

    /// One-shot: after a short delay, try empty-password PAM so pam_fprintd /
    /// pam_u2f can unlock without the user pressing Enter.
    fn lock_arm_biometric_auto(&mut self) {
        if !self.lock.auth_cues.any() {
            return;
        }
        let delay = Duration::from_millis(400);
        if let Err(err) = self.loop_handle.insert_source(
            Timer::from_duration(delay),
            move |_, _, state: &mut MetisState| {
                state.lock_try_biometric_auto();
                TimeoutAction::Drop
            },
        ) {
            tracing::warn!(?err, "lock: failed to arm biometric auto-attempt");
        }
    }

    fn lock_try_biometric_auto(&mut self) {
        if !self.lock.locked
            || self.lock.auth_in_flight
            || self.lock.biometric_auto_tried
            || !self.lock.auth_cues.any()
            || !self.lock.password.is_empty()
        {
            return;
        }
        self.lock.biometric_auto_tried = true;
        self.lock_submit();
    }

    // --- Password field editing (called from input while locked) --------------

    pub fn lock_push_char(&mut self, c: char) {
        if self.lock.auth_in_flight {
            return;
        }
        // Guard against runaway paste / stuck key.
        if self.lock.password.chars().count() >= 256 {
            return;
        }
        self.lock.status = None;
        self.lock.password.push(c);
        self.damaged = true;
        self.request_redraw();
    }

    pub fn lock_backspace(&mut self) {
        if self.lock.auth_in_flight {
            return;
        }
        self.lock.password.pop();
        self.lock.status = None;
        self.damaged = true;
        self.request_redraw();
    }

    pub fn lock_clear_input(&mut self) {
        if self.lock.auth_in_flight {
            return;
        }
        self.lock.clear_password();
        self.lock.status = lock_cue_status(self.lock.auth_cues);
        self.damaged = true;
        self.request_redraw();
    }

    /// Submit the typed password (or an empty buffer for biometric PAM) on a worker.
    pub fn lock_submit(&mut self) {
        if self.lock.auth_in_flight {
            return;
        }
        let username = current_username().unwrap_or_default();
        if username.is_empty() {
            self.lock.status = Some("Cannot determine user".to_string());
            self.lock.clear_password();
            self.damaged = true;
            self.request_redraw();
            return;
        }
        self.lock.auth_generation = self.lock.auth_generation.wrapping_add(1);
        let generation = self.lock.auth_generation;
        self.lock.auth_in_flight = true;
        self.lock.status = Some(metis_i18n::tr_ftl("lock-authenticating"));
        // Move the password out (leaving the field empty) so only the worker
        // thread holds it, and it is zeroized there once the attempt completes.
        // Empty password is intentional for pam_fprintd / pam_u2f.
        let password = std::mem::take(&mut self.lock.password);
        let tx = self.lock.auth_tx.clone();
        let service = pam_service();

        let spawned = std::thread::Builder::new()
            .name("metis-lock-pam".into())
            .spawn(move || {
                let success = pam_check(&service, &username, &password);
                {
                    use zeroize::Zeroize;
                    let mut password = password;
                    password.zeroize();
                }
                // Small delay on failure to throttle brute-force attempts without
                // blocking the compositor (this runs off the event-loop thread).
                if !success {
                    std::thread::sleep(Duration::from_millis(1200));
                }
                let _ = tx.send(AuthOutcome {
                    generation,
                    success,
                });
            });
        if let Err(err) = spawned {
            tracing::warn!(?err, "lock: failed to spawn PAM worker");
            self.lock.auth_in_flight = false;
            self.lock.status = Some(metis_i18n::tr_ftl("lock-auth-unavailable"));
        }
        self.damaged = true;
        self.request_redraw();
    }

    fn lock_on_auth_result(&mut self, outcome: AuthOutcome) {
        if outcome.generation != self.lock.auth_generation {
            return;
        }
        self.lock.auth_in_flight = false;
        if outcome.success {
            self.unlock_session();
        } else {
            self.lock.attempts = self.lock.attempts.wrapping_add(1);
            self.lock.status = Some(if self.lock.auth_cues.any() {
                metis_i18n::tr_ftl("lock-auth-failed")
            } else {
                metis_i18n::tr_ftl("lock-incorrect-password")
            });
            self.lock.clear_password();
            tracing::warn!(attempts = self.lock.attempts, "lock: authentication failed");
            self.damaged = true;
            self.request_redraw();
        }
    }

    // --- Rendering ------------------------------------------------------------

    /// Build the lock-screen render stack for one target. Returns ONLY lock
    /// elements (background + blur/dim + text); the normal client/layer/chrome
    /// stack is skipped entirely by [`MetisState::build_render_elements`].
    pub fn build_lock_elements(
        &mut self,
        renderer: &mut GlesRenderer,
        render_origin: Point<i32, Physical>,
        target: &RenderTargetInfo<'_>,
        scale: f64,
    ) -> Vec<OutputStack> {
        let size = target.size;
        if size.w <= 0 || size.h <= 0 {
            return Vec::new();
        }
        if !self.lock.font_loaded {
            if let Some((font, data)) = crate::decoration::load_font_with_data() {
                self.lock.font = Some(font);
                self.lock.font_data = Some(data);
            }
            self.lock.font_loaded = true;
        }

        let mut elems: Vec<OutputStack> = Vec::new();
        let cx = size.w as f64 / 2.0;
        let cy = size.h as f64 / 2.0;

        // --- Text (topmost; pushed first) ---
        let show_clock = self.lock.cfg.show_clock;
        let now = chrono::Local::now();
        // Field/status area is anchored below the (optional) clock block.
        let field_y = if show_clock { cy + 80.0 } else { cy };

        if show_clock {
            let time = if self.lock.cfg.clock_24h {
                now.format("%H:%M").to_string()
            } else {
                // 12-hour, no leading zero, e.g. "9:05 PM".
                now.format("%-I:%M %p").to_string()
            };
            let date = now.format("%A, %B %-d").to_string();
            self.push_lock_text(
                renderer,
                &mut elems,
                &time,
                120.0,
                [1.0, 1.0, 1.0, 0.96],
                cx,
                cy - 150.0,
            );
            self.push_lock_text(
                renderer,
                &mut elems,
                &date,
                30.0,
                [1.0, 1.0, 1.0, 0.72],
                cx,
                cy - 60.0,
            );
        }

        // Greeting: the account's display name (GECOS), falling back to the login.
        if let Some(name) = current_display_name() {
            self.push_lock_text(
                renderer,
                &mut elems,
                &name,
                26.0,
                [1.0, 1.0, 1.0, 0.82],
                cx,
                field_y - 66.0,
            );
        }

        // Password entry: dots (or a placeholder) drawn over a rounded field box,
        // with a blinking caret at the insertion point (the field is always the
        // focus while locked). Visibility is owned by the 500 ms lock timer
        // (`caret_on`) — do not sample wall-clock here or idle 2 Hz redraws
        // alias into a harsh strobe.
        let count = self.lock.password.chars().count();
        let caret_gap = 6.0;
        let caret_x = if count > 0 {
            let dots: String = "•".repeat(count.min(32));
            let w = self
                .push_lock_text(
                    renderer,
                    &mut elems,
                    &dots,
                    32.0,
                    [1.0, 1.0, 1.0, 0.95],
                    cx,
                    field_y,
                )
                .map(|(w, _)| w as f64)
                .unwrap_or(0.0);
            // The rasterizer pads each side; trim it so the caret hugs the dots.
            cx + (w / 2.0 - 32.0 * 0.3) + caret_gap
        } else {
            let w = self
                .push_lock_text(
                    renderer,
                    &mut elems,
                    &metis_i18n::tr_ftl("lock-enter-password"),
                    20.0,
                    [1.0, 1.0, 1.0, 0.5],
                    cx,
                    field_y,
                )
                .map(|(w, _)| w as f64)
                .unwrap_or(0.0);
            // Caret sits just left of the placeholder.
            cx - (w / 2.0 - 20.0 * 0.3) - caret_gap
        };
        if !self.lock.auth_in_flight && self.lock.caret_on {
            let ch = 36i32;
            let key = sprite_key("caret", &[ch as i64]);
            self.push_lock_sprite(
                renderer,
                &mut elems,
                key,
                || rasterize_caret(3, ch),
                (caret_x - 1.5, field_y - ch as f64 / 2.0),
            );
        }

        // Status / error / biometric cue line below the field.
        if let Some(status) = self.lock.status.clone() {
            let color = if self.lock.auth_in_flight {
                [1.0, 1.0, 1.0, 0.72]
            } else if self.lock.attempts > 0 {
                [1.0, 0.5, 0.5, 0.95]
            } else {
                [1.0, 1.0, 1.0, 0.65]
            };
            self.push_lock_text(
                renderer,
                &mut elems,
                &status,
                20.0,
                color,
                cx,
                field_y + 60.0,
            );
        }

        // Power controls (suspend / restart / shut down) at the bottom-right.
        self.push_lock_power_buttons(renderer, &mut elems, size, scale);

        // Rounded password field box, behind the dots/placeholder pushed above.
        {
            let fw = (360.0f64).min(size.w as f64 - 80.0).max(160.0);
            let fh = 60.0f64;
            let focused = count > 0;
            let key = sprite_key("field", &[fw as i64, fh as i64, focused as i64]);
            self.push_lock_sprite(
                renderer,
                &mut elems,
                key,
                || rasterize_field(fw as i32, fh as i32, focused),
                (cx - fw / 2.0, field_y - fh / 2.0),
            );
        }

        // --- Dim overlay (below text, above background) ---
        let dim = f32::from(self.lock.cfg.dim_percent.min(100)) / 100.0;
        if dim > 0.0 {
            let color = premultiply(Color32F::new(0.0, 0.0, 0.0, dim));
            elems.push(OutputStack::Overlay(SolidColorRenderElement::new(
                self.lock.dim_id.clone(),
                Rectangle::from_size(size),
                self.lock.dim_commit,
                color,
                Kind::Unspecified,
            )));
        }

        // --- Background (bottom; pushed last) ---
        self.push_lock_background(renderer, &mut elems, render_origin, size);

        elems
    }

    /// Rasterize (or reuse) a centered text line and push it as an element.
    /// Returns the rendered `(width, height)` so callers can position adjacent
    /// elements (e.g. the password caret) precisely.
    #[allow(clippy::too_many_arguments)]
    fn push_lock_text(
        &mut self,
        renderer: &mut GlesRenderer,
        elems: &mut Vec<OutputStack>,
        text: &str,
        px: f32,
        color: [f32; 4],
        center_x: f64,
        center_y: f64,
    ) -> Option<(i32, i32)> {
        let key = text_key(text, px, color);
        if !self.lock.text_cache.contains_key(&key) {
            let font = self.lock.font.as_ref()?;
            let (pixels, w, h) = if let Some(data) = self.lock.font_data.as_ref() {
                crate::text_layout::rasterize_text_bidi(font, data, text, px, color)?
            } else {
                rasterize_text(font, text, px, color)?
            };
            let Ok(texture) =
                renderer.import_memory(&pixels, Fourcc::Abgr8888, (w, h).into(), false)
            else {
                return None;
            };
            let buffer = TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);
            // Bound the cache so a long session of typing can't grow it forever.
            if self.lock.text_cache.len() > 128 {
                self.lock.text_cache.clear();
            }
            self.lock
                .text_cache
                .insert(key, CachedText { buffer, w, h });
        }
        if let Some(cached) = self.lock.text_cache.get(&key) {
            let (w, h) = (cached.w, cached.h);
            let loc = Point::<f64, Physical>::from((
                center_x - w as f64 / 2.0,
                center_y - h as f64 / 2.0,
            ));
            elems.push(OutputStack::Wallpaper(
                TextureRenderElement::from_texture_buffer(
                    loc,
                    &cached.buffer,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                ),
            ));
            return Some((w, h));
        }
        None
    }

    /// Rasterize (or reuse) an arbitrary RGBA sprite and push it at `top_left`
    /// (render-target-local physical coordinates). `make` is only invoked on a
    /// cache miss.
    fn push_lock_sprite(
        &mut self,
        renderer: &mut GlesRenderer,
        elems: &mut Vec<OutputStack>,
        key: u64,
        make: impl FnOnce() -> Option<(Vec<u8>, i32, i32)>,
        top_left: (f64, f64),
    ) {
        if !self.lock.text_cache.contains_key(&key) {
            let Some((pixels, w, h)) = make() else {
                return;
            };
            let Ok(texture) =
                renderer.import_memory(&pixels, Fourcc::Abgr8888, (w, h).into(), false)
            else {
                return;
            };
            let buffer = TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);
            if self.lock.text_cache.len() > 160 {
                self.lock.text_cache.clear();
            }
            self.lock
                .text_cache
                .insert(key, CachedText { buffer, w, h });
        }
        if let Some(cached) = self.lock.text_cache.get(&key) {
            let loc = Point::<f64, Physical>::from(top_left);
            elems.push(OutputStack::Wallpaper(
                TextureRenderElement::from_texture_buffer(
                    loc,
                    &cached.buffer,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                ),
            ));
        }
    }

    /// Push the bottom-right power controls (suspend / restart / shut down),
    /// with a hover backdrop on the button the pointer is over.
    fn push_lock_power_buttons(
        &mut self,
        renderer: &mut GlesRenderer,
        elems: &mut Vec<OutputStack>,
        size: Size<i32, Physical>,
        scale: f64,
    ) {
        let scale = scale.max(0.1);
        let layout = power_button_layout(size.w as f64, size.h as f64, scale);
        let icon_side = ((POWER_BTN * scale) * 0.5).round().max(8.0) as i32;
        for (i, (bx, by, side)) in layout.iter().enumerate() {
            let btn = POWER_ORDER[i];
            let hovered = self.lock.hovered_power == Some(i);
            // Icon first so it sits on top of the hover backdrop pushed after it.
            let color = if hovered {
                [1.0, 1.0, 1.0, 0.98]
            } else {
                [1.0, 1.0, 1.0, 0.78]
            };
            let icon_key = sprite_key(
                "power-icon",
                &[btn as i64, icon_side as i64, hovered as i64],
            );
            let ix = bx + (side - icon_side as f64) / 2.0;
            let iy = by + (side - icon_side as f64) / 2.0;
            self.push_lock_sprite(
                renderer,
                elems,
                icon_key,
                || rasterize_power_icon(btn, icon_side, color),
                (ix, iy),
            );
            if hovered {
                let s = side.round() as i32;
                let key = sprite_key("power-hover", &[s as i64]);
                self.push_lock_sprite(renderer, elems, key, || rasterize_round_bg(s), (*bx, *by));
            }
        }

        // Tooltip for the hovered control, centered above it and forced to the
        // very top of the stack so nothing can occlude it.
        if let Some(i) = self.lock.hovered_power {
            let (bx, by, side) = layout[i];
            let label = match POWER_ORDER[i] {
                PowerButton::Suspend => "Suspend",
                PowerButton::Restart => "Restart",
                PowerButton::Shutdown => "Shut Down",
            };
            self.push_lock_tooltip(renderer, elems, label, bx + side / 2.0, by - 22.0);
        }
    }

    /// Rasterize (or reuse) a pill tooltip and insert it at the front of the
    /// stack (topmost). Used for the power-control labels on hover.
    fn push_lock_tooltip(
        &mut self,
        renderer: &mut GlesRenderer,
        elems: &mut Vec<OutputStack>,
        label: &str,
        center_x: f64,
        center_y: f64,
    ) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        label.hash(&mut hasher);
        let key = sprite_key("tooltip", &[hasher.finish() as i64]);
        if !self.lock.text_cache.contains_key(&key) {
            let Some(font) = self.lock.font.as_ref() else {
                return;
            };
            let Some((pixels, w, h)) = rasterize_tooltip(font, label) else {
                return;
            };
            let Ok(texture) =
                renderer.import_memory(&pixels, Fourcc::Abgr8888, (w, h).into(), false)
            else {
                return;
            };
            let buffer = TextureBuffer::from_texture(renderer, texture, 1, Transform::Normal, None);
            if self.lock.text_cache.len() > 160 {
                self.lock.text_cache.clear();
            }
            self.lock
                .text_cache
                .insert(key, CachedText { buffer, w, h });
        }
        if let Some(cached) = self.lock.text_cache.get(&key) {
            let loc = Point::<f64, Physical>::from((
                center_x - cached.w as f64 / 2.0,
                center_y - cached.h as f64 / 2.0,
            ));
            // Front of the vector = topmost, so the tooltip is above everything.
            elems.insert(
                0,
                OutputStack::Wallpaper(TextureRenderElement::from_texture_buffer(
                    loc,
                    &cached.buffer,
                    None,
                    None,
                    None,
                    Kind::Unspecified,
                )),
            );
        }
    }

    /// Which power button (index into [`POWER_ORDER`]) is under `loc` (global
    /// logical coordinates), if any. Uses the output the pointer is over so the
    /// hit-boxes line up with the per-output bottom-right layout in render.
    fn lock_power_button_at(&self, loc: Point<f64, Logical>) -> Option<usize> {
        let (ox, oy, aw, ah) = self
            .output_under_pointer()
            .and_then(|o| self.space.output_geometry(&o))
            .map(|g| {
                (
                    g.loc.x as f64,
                    g.loc.y as f64,
                    g.size.w as f64,
                    g.size.h as f64,
                )
            })
            .unwrap_or_else(|| {
                let b = self.desktop_bounds();
                (
                    b.loc.x as f64,
                    b.loc.y as f64,
                    b.size.w as f64,
                    b.size.h as f64,
                )
            });
        let layout = power_button_layout(aw, ah, 1.0);
        for (i, (x, y, side)) in layout.iter().enumerate() {
            let bx = ox + x;
            let by = oy + y;
            if loc.x >= bx && loc.x <= bx + side && loc.y >= by && loc.y <= by + side {
                return Some(i);
            }
        }
        None
    }

    /// Update the hovered power button from a pointer position (while locked).
    pub fn lock_update_hover(&mut self, loc: Point<f64, Logical>) {
        let idx = self.lock_power_button_at(loc);
        if self.lock.hovered_power != idx {
            self.lock.hovered_power = idx;
            self.damaged = true;
            self.request_redraw();
        }
    }

    /// Handle a left-click while locked: run the power action under the pointer,
    /// if any. Returns true when a button was hit.
    pub fn lock_pointer_click(&mut self, loc: Point<f64, Logical>) -> bool {
        match self.lock_power_button_at(loc) {
            Some(i) => {
                run_power_action(POWER_ORDER[i]);
                true
            }
            None => false,
        }
    }

    /// Push the lock background (blur/base texture or a solid fill) at the back.
    fn push_lock_background(
        &mut self,
        renderer: &mut GlesRenderer,
        elems: &mut Vec<OutputStack>,
        render_origin: Point<i32, Physical>,
        size: Size<i32, Physical>,
    ) {
        let blur = self.lock.cfg.blur;
        self.blur.ensure_program(renderer);

        match self.lock.cfg.background {
            LockBackgroundSource::Solid => {
                let rgb = metis_config::parse_hex_rgb(&self.lock.cfg.color);
                let color = Color32F::from([
                    rgb[0] as f32 / 255.0,
                    rgb[1] as f32 / 255.0,
                    rgb[2] as f32 / 255.0,
                    1.0,
                ]);
                elems.push(OutputStack::Overlay(SolidColorRenderElement::new(
                    self.lock.solid_id.clone(),
                    Rectangle::from_size(size),
                    self.lock.solid_commit,
                    color,
                    Kind::Unspecified,
                )));
            }
            LockBackgroundSource::Wallpaper => {
                self.wallpaper.poll_decode();
                self.wallpaper.ensure(renderer);
                if let Some((texture, tex_size)) = self.wallpaper.texture() {
                    if blur {
                        let geometry = Rectangle::from_size(size);
                        let src = Rectangle::<f64, Buffer>::new(
                            Point::from((render_origin.x as f64, render_origin.y as f64)),
                            Size::from((size.w as f64, size.h as f64)),
                        );
                        if let Some(el) = self.blur.lock_element(
                            geometry,
                            src,
                            texture,
                            tex_size,
                            LOCK_BLUR_RADIUS,
                        ) {
                            elems.push(OutputStack::Blur(el));
                            return;
                        }
                    }
                    // No blur (or blur unavailable): draw the wallpaper directly.
                    let loc = Point::<f64, Physical>::from((
                        -render_origin.x as f64,
                        -render_origin.y as f64,
                    ));
                    if let Some(el) = self.wallpaper.render_element_at(loc) {
                        elems.push(OutputStack::Wallpaper(el));
                        return;
                    }
                }
                // Fallback: solid dark fill when the wallpaper isn't ready.
                elems.push(OutputStack::Overlay(SolidColorRenderElement::new(
                    self.lock.solid_id.clone(),
                    Rectangle::from_size(size),
                    self.lock.solid_commit,
                    Color32F::from([0.04, 0.05, 0.07, 1.0]),
                    Kind::Unspecified,
                )));
            }
            LockBackgroundSource::Picture | LockBackgroundSource::Gradient => {
                let sig = self.lock.bg_signature();
                let key = (size.w, size.h, sig);
                if !self.lock.bg_cache.contains_key(&key)
                    && let Some(pixels) = self.decode_lock_bg_pixels(size)
                    && let Ok(texture) = renderer.import_memory(
                        &pixels,
                        Fourcc::Abgr8888,
                        (size.w, size.h).into(),
                        false,
                    )
                {
                    let buffer = TextureBuffer::from_texture(
                        renderer,
                        texture.clone(),
                        1,
                        Transform::Normal,
                        None,
                    );
                    self.lock.bg_cache.insert(key, (texture, buffer));
                }
                if let Some((texture, buffer)) = self.lock.bg_cache.get(&key) {
                    if blur {
                        let tex_size = texture.size();
                        let geometry = Rectangle::from_size(size);
                        let src = Rectangle::<f64, Buffer>::new(
                            Point::from((0.0, 0.0)),
                            Size::from((size.w as f64, size.h as f64)),
                        );
                        if let Some(el) = self.blur.lock_element(
                            geometry,
                            src,
                            texture.clone(),
                            tex_size,
                            LOCK_BLUR_RADIUS,
                        ) {
                            elems.push(OutputStack::Blur(el));
                            return;
                        }
                    }
                    elems.push(OutputStack::Wallpaper(
                        TextureRenderElement::from_texture_buffer(
                            Point::<f64, Physical>::from((0.0, 0.0)),
                            buffer,
                            None,
                            None,
                            None,
                            Kind::Unspecified,
                        ),
                    ));
                } else {
                    elems.push(OutputStack::Overlay(SolidColorRenderElement::new(
                        self.lock.solid_id.clone(),
                        Rectangle::from_size(size),
                        self.lock.solid_commit,
                        Color32F::from([0.04, 0.05, 0.07, 1.0]),
                        Kind::Unspecified,
                    )));
                }
            }
        }
    }

    /// Generate the picture/gradient lock background as premultiplied-agnostic
    /// (opaque) RGBA pixels sized to the render target.
    fn decode_lock_bg_pixels(&self, size: Size<i32, Physical>) -> Option<Vec<u8>> {
        let w = size.w.max(1) as u32;
        let h = size.h.max(1) as u32;
        match self.lock.cfg.background {
            LockBackgroundSource::Gradient => Some(gen_gradient(
                metis_config::parse_hex_rgb(&self.lock.cfg.gradient_start),
                metis_config::parse_hex_rgb(&self.lock.cfg.gradient_end),
                self.lock.cfg.gradient_direction,
                w,
                h,
            )),
            LockBackgroundSource::Picture => {
                let path = self.lock.cfg.picture_path.as_ref()?;
                match image::open(path) {
                    Ok(img) => Some(cover_crop_rgba(&img.into_rgba8(), w, h)),
                    Err(err) => {
                        tracing::warn!(path = %path, ?err, "lock: failed to open picture");
                        None
                    }
                }
            }
            _ => None,
        }
    }
}

/// PAM service to authenticate against. Prefers the Metis-specific service when
/// installed (`/etc/pam.d/metis`); otherwise falls back to a common login stack
/// so a not-yet-installed dev session can still unlock.
fn pam_service() -> String {
    for name in ["metis", "system-login", "login"] {
        if std::path::Path::new("/etc/pam.d").join(name).exists() {
            return name.to_string();
        }
    }
    "login".to_string()
}

fn lock_cue_status(cues: AuthCues) -> Option<String> {
    cues.status_ftl_id().map(metis_i18n::tr_ftl)
}

// --- Minimal libpam FFI ------------------------------------------------------
//
// We link `libpam` directly rather than pulling in the `pam` crate, whose
// `pam-sys` dependency runs `bindgen` at build time (requiring libclang). A
// hand-written binding keeps the build dependency-light and portable. Only the
// tiny subset needed for a login-style password check is declared here.

const PAM_SUCCESS: c_int = 0;
const PAM_PROMPT_ECHO_OFF: c_int = 1;
const PAM_PROMPT_ECHO_ON: c_int = 2;
const PAM_ERROR_MSG: c_int = 3;
const PAM_TEXT_INFO: c_int = 4;
const PAM_BUF_ERR: c_int = 5;
const PAM_CONV_ERR: c_int = 19;

#[repr(C)]
struct PamMessage {
    msg_style: c_int,
    msg: *const c_char,
}

#[repr(C)]
struct PamResponse {
    resp: *mut c_char,
    resp_retcode: c_int,
}

#[repr(C)]
struct PamConv {
    conv: Option<
        unsafe extern "C" fn(
            num_msg: c_int,
            msg: *mut *const PamMessage,
            resp: *mut *mut PamResponse,
            appdata_ptr: *mut c_void,
        ) -> c_int,
    >,
    appdata_ptr: *mut c_void,
}

/// Opaque PAM handle.
enum PamHandle {}

#[link(name = "pam")]
unsafe extern "C" {
    fn pam_start(
        service: *const c_char,
        user: *const c_char,
        conv: *const PamConv,
        handle: *mut *mut PamHandle,
    ) -> c_int;
    fn pam_authenticate(handle: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_acct_mgmt(handle: *mut PamHandle, flags: c_int) -> c_int;
    fn pam_end(handle: *mut PamHandle, status: c_int) -> c_int;
}

/// Credentials handed to the PAM conversation callback.
struct ConvData {
    user: std::ffi::CString,
    password: std::ffi::CString,
}

/// PAM conversation: answer the password prompt (echo off) with the typed
/// password and any echoed prompt (echo on) with the username. Info / error
/// messages from pam_fprintd / pam_u2f are acknowledged with an empty response.
/// Responses are allocated with libc so PAM can `free()` them.
unsafe extern "C" fn converse(
    num_msg: c_int,
    msg: *mut *const PamMessage,
    resp: *mut *mut PamResponse,
    appdata_ptr: *mut c_void,
) -> c_int {
    unsafe {
        if num_msg <= 0 || msg.is_null() || resp.is_null() || appdata_ptr.is_null() {
            return PAM_CONV_ERR;
        }
        let data = &*(appdata_ptr as *const ConvData);
        let n = num_msg as usize;
        let responses = libc::calloc(n, std::mem::size_of::<PamResponse>()) as *mut PamResponse;
        if responses.is_null() {
            return PAM_BUF_ERR;
        }
        for i in 0..n {
            let message = *msg.add(i);
            let out = responses.add(i);
            (*out).resp = std::ptr::null_mut();
            (*out).resp_retcode = 0;
            if message.is_null() {
                continue;
            }
            match (*message).msg_style {
                PAM_PROMPT_ECHO_OFF => {
                    (*out).resp = libc::strdup(data.password.as_ptr());
                }
                PAM_PROMPT_ECHO_ON => {
                    (*out).resp = libc::strdup(data.user.as_ptr());
                }
                PAM_TEXT_INFO | PAM_ERROR_MSG => {
                    // Acknowledge; modules may free(resp). Empty string is safe.
                    (*out).resp = libc::strdup(c"".as_ptr());
                }
                _ => {}
            }
        }
        *resp = responses;
        PAM_SUCCESS
    }
}

/// Authenticate `user`/`password` against the given PAM `service`. Returns true
/// only when both authentication and account management succeed. Never logs the
/// password.
fn pam_check(service: &str, user: &str, password: &str) -> bool {
    use std::ffi::CString;
    let (Ok(c_service), Ok(c_user_start)) = (CString::new(service), CString::new(user)) else {
        return false;
    };
    let (Ok(conv_user), Ok(conv_pass)) = (CString::new(user), CString::new(password)) else {
        return false;
    };
    let mut data = ConvData {
        user: conv_user,
        password: conv_pass,
    };
    let conv = PamConv {
        conv: Some(converse),
        appdata_ptr: &mut data as *mut ConvData as *mut c_void,
    };
    let mut handle: *mut PamHandle = std::ptr::null_mut();
    unsafe {
        let start = pam_start(
            c_service.as_ptr(),
            c_user_start.as_ptr(),
            &conv,
            &mut handle,
        );
        if start != PAM_SUCCESS || handle.is_null() {
            tracing::warn!(service, "lock: pam_start failed");
            return false;
        }
        let auth = pam_authenticate(handle, 0);
        let acct = if auth == PAM_SUCCESS {
            pam_acct_mgmt(handle, 0)
        } else {
            auth
        };
        pam_end(handle, auth);
        auth == PAM_SUCCESS && acct == PAM_SUCCESS
    }
}

/// Resolve the current user's login name (env first, then the passwd database).
fn current_username() -> Option<String> {
    for var in ["USER", "LOGNAME"] {
        if let Ok(v) = std::env::var(var)
            && !v.is_empty()
        {
            return Some(v);
        }
    }
    // Safe libc fallback: getpwuid(getuid()) then copy the name out.
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if !pw.is_null() {
            let name = (*pw).pw_name;
            if !name.is_null()
                && let Ok(s) = std::ffi::CStr::from_ptr(name).to_str()
                && !s.is_empty()
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Hash a text line's content + style into a stable cache key.
fn text_key(text: &str, px: f32, color: [f32; 4]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    px.to_bits().hash(&mut h);
    for c in color {
        c.to_bits().hash(&mut h);
    }
    h.finish()
}

/// Rasterize `text` at `font_px` into a premultiplied RGBA buffer on a fully
/// transparent background. Returns `(pixels, width, height)`.
fn rasterize_text(
    font: &Font,
    text: &str,
    font_px: f32,
    color: [f32; 4],
) -> Option<(Vec<u8>, i32, i32)> {
    if text.is_empty() {
        return None;
    }
    let pad = (font_px * 0.3).ceil() as i32;
    let mut pen = 0f32;
    let mut placements: Vec<(fontdue::Metrics, Vec<u8>, i32)> = Vec::new();
    for ch in text.chars().take(256) {
        let (metrics, bitmap) = font.rasterize(ch, font_px);
        placements.push((metrics, bitmap, pen.round() as i32));
        pen += metrics.advance_width;
    }
    let text_w = pen.ceil() as i32;
    let width = (text_w + 2 * pad).clamp(1, 8192);
    let height = ((font_px * 1.5).ceil() as i32 + 2 * pad).clamp(1, 512);
    let baseline = pad + font_px.round() as i32;

    let mut pixels = vec![0u8; (width * height * 4) as usize];
    let (cr, cg, cb, ca) = (color[0], color[1], color[2], color[3]);
    for (metrics, bitmap, pen_x) in &placements {
        let gx = pad + pen_x + metrics.xmin;
        let gy = baseline - metrics.ymin - metrics.height as i32;
        for row in 0..metrics.height as i32 {
            let py = gy + row;
            if py < 0 || py >= height {
                continue;
            }
            for col in 0..metrics.width as i32 {
                let px = gx + col;
                if px < 0 || px >= width {
                    continue;
                }
                let cov = bitmap[(row * metrics.width as i32 + col) as usize] as f32 / 255.0;
                let a = cov * ca;
                if a <= 0.0 {
                    continue;
                }
                let idx = ((py * width + px) * 4) as usize;
                // Source-over with premultiplied alpha (destination starts clear).
                let inv = 1.0 - a;
                let dr = pixels[idx] as f32 / 255.0;
                let dg = pixels[idx + 1] as f32 / 255.0;
                let db = pixels[idx + 2] as f32 / 255.0;
                let da = pixels[idx + 3] as f32 / 255.0;
                pixels[idx] = (((cr * a) + dr * inv).clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 1] = (((cg * a) + dg * inv).clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 2] = (((cb * a) + db * inv).clamp(0.0, 1.0) * 255.0) as u8;
                pixels[idx + 3] = (((a) + da * inv).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    Some((pixels, width, height))
}

/// Cover-crop an image to `out_w × out_h` (fill, center-cropped). Mirrors the
/// wallpaper runtime's cover behavior.
fn cover_crop_rgba(rgba: &image::RgbaImage, out_w: u32, out_h: u32) -> Vec<u8> {
    let (iw, ih) = (rgba.width(), rgba.height());
    if iw == 0 || ih == 0 {
        return vec![0; (out_w as usize) * (out_h as usize) * 4];
    }
    let scale = (out_w as f32 / iw as f32).max(out_h as f32 / ih as f32);
    let rw = ((iw as f32 * scale).ceil() as u32).max(1);
    let rh = ((ih as f32 * scale).ceil() as u32).max(1);
    let resized = image::imageops::resize(rgba, rw, rh, image::imageops::FilterType::Triangle);
    let x = rw.saturating_sub(out_w) / 2;
    let y = rh.saturating_sub(out_h) / 2;
    image::imageops::crop_imm(&resized, x, y, out_w, out_h)
        .to_image()
        .into_raw()
}

/// Generate a two-stop linear gradient as opaque RGBA.
fn gen_gradient(a: [u8; 3], b: [u8; 3], dir: GradientDirection, w: u32, h: u32) -> Vec<u8> {
    let w = w.max(1);
    let h = h.max(1);
    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    let wf = (w.saturating_sub(1)).max(1) as f32;
    let hf = (h.saturating_sub(1)).max(1) as f32;
    let lerp = |c0: u8, c1: u8, t: f32| (c0 as f32 + (c1 as f32 - c0 as f32) * t).round() as u8;
    for y in 0..h {
        let yt = y as f32 / hf;
        for x in 0..w {
            let xt = x as f32 / wf;
            let t = match dir {
                GradientDirection::Vertical => yt,
                GradientDirection::VerticalReverse => 1.0 - yt,
                GradientDirection::Horizontal => xt,
                GradientDirection::HorizontalReverse => 1.0 - xt,
                GradientDirection::Diagonal => (xt + yt) * 0.5,
                GradientDirection::DiagonalReverse => ((1.0 - xt) + yt) * 0.5,
            }
            .clamp(0.0, 1.0);
            let idx = ((y * w + x) * 4) as usize;
            out[idx] = lerp(a[0], b[0], t);
            out[idx + 1] = lerp(a[1], b[1], t);
            out[idx + 2] = lerp(a[2], b[2], t);
            out[idx + 3] = 255;
        }
    }
    out
}

// --- Power controls -----------------------------------------------------------

/// Bottom-right lock-screen power actions, laid out left-to-right.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PowerButton {
    Suspend,
    Restart,
    Shutdown,
}

/// Left-to-right button order (indices match hover/hit-test bookkeeping).
const POWER_ORDER: [PowerButton; 3] = [
    PowerButton::Suspend,
    PowerButton::Restart,
    PowerButton::Shutdown,
];
/// Logical button box side, gap between boxes, and margin from the screen edge.
const POWER_BTN: f64 = 48.0;
const POWER_GAP: f64 = 16.0;
const POWER_MARGIN: f64 = 40.0;

/// The three button boxes `(x, y, side)` within an `aw × ah` area, anchored to
/// the bottom-right. `scale` converts the logical metrics into the target's
/// pixel space (pass `1.0` for logical hit-testing).
fn power_button_layout(aw: f64, ah: f64, scale: f64) -> [(f64, f64, f64); 3] {
    let side = POWER_BTN * scale;
    let gap = POWER_GAP * scale;
    let margin = POWER_MARGIN * scale;
    let y = (ah - margin - side).max(0.0);
    let x2 = (aw - margin - side).max(0.0);
    let x1 = x2 - side - gap;
    let x0 = x1 - side - gap;
    [(x0, y, side), (x1, y, side), (x2, y, side)]
}

/// Run the requested power action detached. Best-effort; failures are logged.
fn run_power_action(btn: PowerButton) {
    let arg = match btn {
        PowerButton::Suspend => "suspend",
        PowerButton::Restart => "reboot",
        PowerButton::Shutdown => "poweroff",
    };
    match std::process::Command::new("systemctl")
        .arg(arg)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(_) => tracing::info!(action = arg, "lock: power action requested"),
        Err(err) => tracing::warn!(action = arg, %err, "lock: failed to run power action"),
    }
}

/// Resolve `metis-remote` next to the compositor binary (dev/install layout).
fn metis_remote_bin() -> std::path::PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("metis-remote");
        if sibling.is_file() {
            return sibling;
        }
    }
    std::path::PathBuf::from("metis-remote")
}

/// Fire-and-forget `metis-remote` (pause/resume on lock). Never blocks the compositor.
pub(crate) fn spawn_metis_remote(args: &[&str]) {
    let bin = metis_remote_bin();
    let args: Vec<String> = args.iter().map(|s| (*s).to_string()).collect();
    std::thread::Builder::new()
        .name("metis-remote-lock".into())
        .spawn(move || {
            match std::process::Command::new(&bin)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
            {
                Ok(status) if status.success() => {
                    tracing::debug!(?args, "lock: metis-remote ok");
                }
                Ok(status) => {
                    tracing::warn!(?args, %status, "lock: metis-remote exited non-zero");
                }
                Err(err) => {
                    tracing::warn!(?args, %err, "lock: failed to run metis-remote");
                }
            }
        })
        .ok();
}

/// The account's display name (GECOS first field), falling back to the login.
fn current_display_name() -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null()
            && !(*pw).pw_gecos.is_null()
            && let Ok(gecos) = std::ffi::CStr::from_ptr((*pw).pw_gecos).to_str()
        {
            let name = gecos.split(',').next().unwrap_or("").trim();
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    current_username()
}

/// Namespace + hash a sprite tag and integer parameters into a cache key
/// (distinct from [`text_key`] so text and sprites never collide).
fn sprite_key(tag: &str, params: &[i64]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    "sprite".hash(&mut h);
    tag.hash(&mut h);
    params.hash(&mut h);
    h.finish()
}

// --- Small software rasterizers (premultiplied RGBA) --------------------------

/// Source-over one pixel of straight-alpha `color` at coverage `cov`.
#[inline]
fn blend_px(pixels: &mut [u8], idx: usize, color: [f32; 4], cov: f32) {
    let a = (color[3] * cov).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    for c in 0..3 {
        let src = color[c] * a;
        let dst = pixels[idx + c] as f32 / 255.0;
        pixels[idx + c] = ((src + dst * inv).clamp(0.0, 1.0) * 255.0) as u8;
    }
    let da = pixels[idx + 3] as f32 / 255.0;
    pixels[idx + 3] = ((a + da * inv).clamp(0.0, 1.0) * 255.0) as u8;
}

/// 1px-feather anti-aliased coverage from a signed distance (positive inside).
#[inline]
fn aa(sd: f32) -> f32 {
    (sd + 0.5).clamp(0.0, 1.0)
}

/// Signed distance to a rounded rectangle centered in a `w × h` field with
/// corner `radius`; negative inside, positive outside.
#[inline]
fn rounded_rect_sd(px: f32, py: f32, w: f32, h: f32, radius: f32) -> f32 {
    let qx = (px - w / 2.0).abs() - (w / 2.0 - radius);
    let qy = (py - h / 2.0).abs() - (h / 2.0 - radius);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - radius
}

/// Rasterize the rounded password field (translucent glass fill + hairline
/// border). A brighter border marks the focused (non-empty) state.
fn rasterize_field(w: i32, h: i32, focused: bool) -> Option<(Vec<u8>, i32, i32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (w, h) = (w.min(2048), h.min(512));
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let radius = (h as f32 / 2.0).min(w as f32 / 2.0);
    let fill = [1.0, 1.0, 1.0, if focused { 0.16 } else { 0.10 }];
    let border = [1.0, 1.0, 1.0, if focused { 0.55 } else { 0.30 }];
    let bw = 1.6f32;
    for y in 0..h {
        for x in 0..w {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let sd = rounded_rect_sd(px, py, w as f32, h as f32, radius);
            let cov = aa(-sd);
            if cov > 0.0 {
                let idx = ((y * w + x) * 4) as usize;
                blend_px(&mut pixels, idx, fill, cov);
                let ring = aa(bw - sd.abs());
                if ring > 0.0 {
                    blend_px(&mut pixels, idx, border, ring);
                }
            }
        }
    }
    Some((pixels, w, h))
}

/// Rasterize the blinking password caret (a rounded vertical bar).
fn rasterize_caret(w: i32, h: i32) -> Option<(Vec<u8>, i32, i32)> {
    if w <= 0 || h <= 0 {
        return None;
    }
    let (w, h) = (w.min(64), h.min(512));
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let color = [1.0, 1.0, 1.0, 0.9];
    let radius = (w as f32 / 2.0).min(h as f32 / 2.0);
    for y in 0..h {
        for x in 0..w {
            let sd = rounded_rect_sd(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
            let cov = aa(-sd);
            if cov > 0.0 {
                blend_px(&mut pixels, ((y * w + x) * 4) as usize, color, cov);
            }
        }
    }
    Some((pixels, w, h))
}

/// Rasterize a pill tooltip: a dark rounded background with the label centered.
fn rasterize_tooltip(font: &Font, label: &str) -> Option<(Vec<u8>, i32, i32)> {
    let (txt, tw, th) = rasterize_text(font, label, 18.0, [1.0, 1.0, 1.0, 0.98])?;
    let margin_x = 12i32;
    let w = (tw + 2 * margin_x).clamp(1, 2048);
    let h = th.clamp(1, 512);
    let mut pixels = vec![0u8; (w * h * 4) as usize];
    let radius = (h as f32 / 2.0).min(16.0);
    let fill = [0.06, 0.07, 0.09, 0.88];
    for y in 0..h {
        for x in 0..w {
            let sd = rounded_rect_sd(x as f32 + 0.5, y as f32 + 0.5, w as f32, h as f32, radius);
            let cov = aa(-sd);
            if cov > 0.0 {
                blend_px(&mut pixels, ((y * w + x) * 4) as usize, fill, cov);
            }
        }
    }
    // Composite the (premultiplied) text horizontally centered via a left margin.
    for y in 0..th {
        for x in 0..tw {
            let sidx = ((y * tw + x) * 4) as usize;
            let sa = txt[sidx + 3] as f32 / 255.0;
            if sa <= 0.0 {
                continue;
            }
            let dx = x + margin_x;
            if dx >= w || y >= h {
                continue;
            }
            let didx = ((y * w + dx) * 4) as usize;
            let inv = 1.0 - sa;
            for c in 0..4 {
                let s = txt[sidx + c] as f32 / 255.0;
                let d = pixels[didx + c] as f32 / 255.0;
                pixels[didx + c] = ((s + d * inv).clamp(0.0, 1.0) * 255.0) as u8;
            }
        }
    }
    Some((pixels, w, h))
}

/// Rasterize the translucent rounded-square hover backdrop for a power button.
fn rasterize_round_bg(side: i32) -> Option<(Vec<u8>, i32, i32)> {
    if side <= 0 {
        return None;
    }
    let side = side.min(512);
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    let radius = side as f32 * 0.28;
    let fill = [1.0, 1.0, 1.0, 0.14];
    for y in 0..side {
        for x in 0..side {
            let sd = rounded_rect_sd(
                x as f32 + 0.5,
                y as f32 + 0.5,
                side as f32,
                side as f32,
                radius,
            );
            let cov = aa(-sd);
            if cov > 0.0 {
                blend_px(&mut pixels, ((y * side + x) * 4) as usize, fill, cov);
            }
        }
    }
    Some((pixels, side, side))
}

/// Rasterize a power-control glyph (moon / circular-arrow / power symbol).
fn rasterize_power_icon(
    btn: PowerButton,
    side: i32,
    color: [f32; 4],
) -> Option<(Vec<u8>, i32, i32)> {
    if side <= 0 {
        return None;
    }
    let side = side.min(512);
    let mut pixels = vec![0u8; (side * side * 4) as usize];
    match btn {
        PowerButton::Shutdown => draw_power_symbol(&mut pixels, side, color),
        PowerButton::Restart => draw_restart_symbol(&mut pixels, side, color),
        PowerButton::Suspend => draw_moon_symbol(&mut pixels, side, color),
    }
    Some((pixels, side, side))
}

/// Classic power glyph: a ring open at the top crossed by a vertical bar.
fn draw_power_symbol(pixels: &mut [u8], side: i32, color: [f32; 4]) {
    let s = side as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let r = s * 0.30;
    let sw = (s * 0.10).max(1.5);
    let bar_top = cy - r * 1.18;
    let bar_bot = cy - s * 0.02;
    let hxbar = sw / 2.0;
    for y in 0..side {
        for x in 0..side {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let mut ring = aa(sw / 2.0 - (dist - r).abs());
            let ang = dy.atan2(dx);
            if (ang + std::f32::consts::FRAC_PI_2).abs() < 0.42 {
                ring = 0.0;
            }
            let in_y = aa(py - bar_top) * aa(bar_bot - py);
            let bar = aa(hxbar - dx.abs()) * in_y;
            let cov = ring.max(bar);
            if cov > 0.0 {
                blend_px(pixels, ((y * side + x) * 4) as usize, color, cov);
            }
        }
    }
}

/// Crescent-moon glyph: a filled disk with an offset disk subtracted.
fn draw_moon_symbol(pixels: &mut [u8], side: i32, color: [f32; 4]) {
    let s = side as f32;
    let (cx, cy) = (s * 0.52, s * 0.5);
    let r = s * 0.36;
    let (ox, oy) = (cx + r * 0.55, cy - r * 0.32);
    let or = r * 0.94;
    for y in 0..side {
        for x in 0..side {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let d1 = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let d2 = ((px - ox).powi(2) + (py - oy).powi(2)).sqrt();
            let cov = (aa(r - d1) * (1.0 - aa(or - d2))).clamp(0.0, 1.0);
            if cov > 0.0 {
                blend_px(pixels, ((y * side + x) * 4) as usize, color, cov);
            }
        }
    }
}

/// Circular-arrow (reload) glyph: a ring with a gap plus a triangular arrowhead.
fn draw_restart_symbol(pixels: &mut [u8], side: i32, color: [f32; 4]) {
    let s = side as f32;
    let (cx, cy) = (s / 2.0, s / 2.0);
    let r = s * 0.30;
    let sw = (s * 0.10).max(1.5);
    let gap_center = -0.55f32;
    let gap_half = 0.6f32;
    for y in 0..side {
        for x in 0..side {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;
            let dx = px - cx;
            let dy = py - cy;
            let dist = (dx * dx + dy * dy).sqrt();
            let ang = dy.atan2(dx);
            let mut ring = aa(sw / 2.0 - (dist - r).abs());
            if (ang - gap_center).abs() < gap_half {
                ring = 0.0;
            }
            if ring > 0.0 {
                blend_px(pixels, ((y * side + x) * 4) as usize, color, ring);
            }
        }
    }
    // Arrowhead at the upper end of the arc, pointing along the (clockwise)
    // tangent so the glyph reads as "reload".
    let ang_end = gap_center - gap_half;
    let (sa, ca) = ang_end.sin_cos();
    let ex = cx + r * ca;
    let ey = cy + r * sa;
    let tdir = (-sa, ca);
    let rad = (ca, sa);
    let tip = (ex + tdir.0 * sw * 1.9, ey + tdir.1 * sw * 1.9);
    let b1 = (ex + rad.0 * sw * 1.4, ey + rad.1 * sw * 1.4);
    let b2 = (ex - rad.0 * sw * 1.4, ey - rad.1 * sw * 1.4);
    for y in 0..side {
        for x in 0..side {
            // 2×2 supersample for a smoother triangle edge.
            let mut acc = 0.0f32;
            for sy in 0..2 {
                for sx in 0..2 {
                    let sp = (
                        x as f32 + 0.25 + sx as f32 * 0.5,
                        y as f32 + 0.25 + sy as f32 * 0.5,
                    );
                    acc += point_in_tri(sp, tip, b1, b2);
                }
            }
            let cov = acc / 4.0;
            if cov > 0.0 {
                blend_px(pixels, ((y * side + x) * 4) as usize, color, cov);
            }
        }
    }
}

/// 1.0 if `p` is inside triangle `a,b,c` (either winding), else 0.0.
#[inline]
fn point_in_tri(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> f32 {
    let edge = |p: (f32, f32), a: (f32, f32), b: (f32, f32)| {
        (p.0 - a.0) * (b.1 - a.1) - (p.1 - a.1) * (b.0 - a.0)
    };
    let d1 = edge(p, a, b);
    let d2 = edge(p, b, c);
    let d3 = edge(p, c, a);
    let has_neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let has_pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    if has_neg && has_pos { 0.0 } else { 1.0 }
}
