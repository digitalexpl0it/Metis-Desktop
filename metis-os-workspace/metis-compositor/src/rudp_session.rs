//! RUDP session glue on [`MetisState`] (Phase 23 §F).
//!
//! Host spawn/reload, layout publish, lock sync, and calloop input registration.
//! Quinn/PAM live in `metis-rudp-host`; this module only bridges calloop ↔ IPC.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use smithay::utils::Logical;

use crate::state::MetisState;

impl MetisState {
    /// Apply `rudp.json` / `METIS_RUDP_HOST` — start, stop, or rebind the host.
    /// No-op under nested winit (no DRM export path).
    pub(crate) fn reload_rudp_host(&mut self) {
        if self.udev.is_none() {
            tracing::debug!("reload-rudp ignored (no DRM session)");
            return;
        }
        let render_path = self
            .udev
            .as_ref()
            .map(|u| crate::rudp_host::render_node_device_path(&u.render_node))
            .unwrap_or_else(|| std::path::PathBuf::from("/dev/dri/renderD128"));
        match crate::rudp_host::desired_host() {
            None => {
                if self.rudp_host.take().is_some() {
                    tracing::info!("rudp host stopped (disabled in rudp.json)");
                }
            }
            Some(desired) => {
                if let Some(host) = self.rudp_host.as_mut()
                    && host.bind == desired.config.bind
                    && host.render_node_path == render_path
                {
                    // Keep Quinn / authenticated sessions. Encode-only changes
                    // (bitrate, codec, backend) reopen workers live — a full
                    // restart mid-session freezes remote Settings / Viewer.
                    if host.encode_prefs != desired.encode_prefs {
                        host.apply_encode_prefs(desired.encode_prefs.clone());
                    }
                    host.apply_policy(
                        desired.allowed_users,
                        desired.lan_only,
                        desired.unlock_session_on_auth,
                    );
                    tracing::info!(
                        bind = %host.bind,
                        allowed = ?host.allowed_users,
                        unlock_on_auth = desired.unlock_session_on_auth,
                        encoder = desired.encode_prefs.backend.as_str(),
                        bitrate_kbps = desired.encode_prefs.bitrate_kbps,
                        "rudp host policy/encode refreshed (sessions kept)"
                    );
                    return;
                }
                self.rudp_host = None;
                match crate::rudp_host::RudpHostSystem::spawn(
                    desired,
                    std::sync::Arc::clone(&self.stream_export),
                    render_path,
                    crate::rudp_host::RudpCalloopBridge {
                        input_tx: self.rudp_input_tx.clone(),
                        pointer_locked: Arc::clone(&self.rudp_pointer_locked),
                        session_locked: Arc::clone(&self.rudp_session_locked),
                        layout: Arc::clone(&self.rudp_layout),
                    },
                ) {
                    Ok(host) => {
                        tracing::info!(bind = %host.bind, "rudp host (re)started");
                        self.rudp_host = Some(host);
                        self.refresh_rudp_layout();
                    }
                    Err(err) => tracing::error!(%err, "rudp host reload failed"),
                }
            }
        }
    }

    /// Desktop topology for Metis Remote clients: enabled outputs in layout
    /// order (x, then y, then name) with logical geometry. Duplicate (mirror)
    /// mode advertises only the mirror source — it is the one picture on every
    /// screen.
    fn rudp_layout_monitors(&self) -> Vec<metis_protocol::RudpMonitorInfo> {
        let mirror_source = if self.mirror_mode_active() {
            self.resolve_mirror_source()
        } else {
            None
        };
        let primary = self.primary_key();
        let mut outputs: Vec<(
            smithay::output::Output,
            smithay::utils::Rectangle<i32, Logical>,
        )> = match mirror_source {
            Some(source) => self
                .space
                .output_geometry(&source)
                .map(|g| (source, g))
                .into_iter()
                .collect(),
            None => self
                .space
                .outputs()
                .filter(|o| o.name() != "metis-render")
                .filter_map(|o| self.space.output_geometry(o).map(|g| (o.clone(), g)))
                .collect(),
        };
        outputs.sort_by(|(a, ag), (b, bg)| {
            ag.loc
                .x
                .cmp(&bg.loc.x)
                .then(ag.loc.y.cmp(&bg.loc.y))
                .then_with(|| a.name().cmp(&b.name()))
        });
        let single = outputs.len() == 1;
        let mut monitors: Vec<metis_protocol::RudpMonitorInfo> = outputs
            .into_iter()
            .map(|(o, g)| {
                let id = o.name();
                metis_protocol::RudpMonitorInfo {
                    primary: single || id == primary,
                    name: id.clone(),
                    id,
                    x: g.loc.x,
                    y: g.loc.y,
                    width: g.size.w.max(0) as u32,
                    height: g.size.h.max(0) as u32,
                    scale: o.current_scale().fractional_scale(),
                }
            })
            .collect();
        // Exactly one primary, even when `primary_key()` names a disabled output.
        if !monitors.is_empty() && !monitors.iter().any(|m| m.primary) {
            monitors[0].primary = true;
        }
        monitors
    }

    /// Re-publish the desktop topology to the RUDP host. Cheap: a no-op (no
    /// generation bump, no client traffic) unless something actually changed.
    /// Call after outputs appear / disappear / move / rescale or the primary
    /// output changes.
    pub(crate) fn refresh_rudp_layout(&self) {
        if self.rudp_host.is_none() {
            return;
        }
        if self.rudp_layout.publish(self.rudp_layout_monitors()) {
            tracing::info!("rudp: desktop layout changed — advertising to clients");
        }
    }

    /// Push PAM / protocol lock state to the RUDP host (pause stream while locked).
    pub(crate) fn sync_rudp_session_locked(&self) {
        self.rudp_session_locked
            .store(self.session_is_locked(), Ordering::Relaxed);
    }

    /// Register Quinn → calloop input inject (once at startup).
    pub fn register_rudp_input_channel(&mut self) {
        use crate::rudp_host::RudpInputEvent;
        use smithay::reexports::calloop::channel::Event;

        let Some(rx) = self.rudp_input_rx.take() else {
            return;
        };
        if let Err(err) = self
            .loop_handle
            .insert_source(rx, |event, _, state: &mut MetisState| {
                let Event::Msg(ev) = event else {
                    return;
                };
                match ev {
                    RudpInputEvent::PointerAbsolute { x, y } => {
                        state.inject_remote_pointer_absolute(x, y);
                    }
                    RudpInputEvent::PointerRelative { dx, dy } => {
                        state.inject_remote_pointer_relative(dx, dy);
                    }
                    RudpInputEvent::PointerButton { button, pressed } => {
                        state.inject_remote_pointer_button(button, pressed);
                    }
                    RudpInputEvent::PointerScroll { dx, dy } => {
                        state.inject_remote_pointer_scroll(dx, dy);
                    }
                    RudpInputEvent::Key { keycode, pressed } => {
                        state.inject_remote_key(keycode, pressed);
                    }
                    RudpInputEvent::ClipboardSet {
                        mime,
                        text,
                        serial: _,
                    } => {
                        if state.session_is_locked() {
                            return;
                        }
                        let mime = if mime.is_empty() {
                            "text/plain;charset=utf-8".into()
                        } else {
                            mime
                        };
                        let text = metis_protocol::truncate_clipboard_text(&text).to_string();
                        if text.is_empty() {
                            return;
                        }
                        if let Err(err) = state.set_clipboard_from_command(mime, Some(text), None) {
                            tracing::debug!(%err, "rudp clipboard set failed");
                        }
                    }
                    RudpInputEvent::UnlockPamSession { username } => {
                        // Metis PAM lock only — never clear ext-session-lock.
                        if !state.lock.locked {
                            return;
                        }
                        let session_user = crate::pam_auth::current_username().unwrap_or_default();
                        if session_user.is_empty() || username != session_user {
                            tracing::info!(
                                %username,
                                %session_user,
                                "rudp: skip unlock — not session owner"
                            );
                            return;
                        }
                        state.unlock_session();
                    }
                    RudpInputEvent::RefreshVideo => {
                        state.stream_export.request_full_frame();
                        state.schedule_redraw();
                    }
                    RudpInputEvent::SetStreamQuality { quality } => {
                        let Some(q) = metis_config::RudpStreamQuality::parse(&quality) else {
                            tracing::warn!(%quality, "rudp: unknown stream quality");
                            return;
                        };
                        if let Some(host) = state.rudp_host.as_mut() {
                            host.apply_stream_quality(q);
                        }
                    }
                }
            })
        {
            tracing::warn!(?err, "rudp: failed to register input channel");
        }
    }

    /// Advertise Wayland pointer-lock state to RUDP clients (Phase 7).
    pub(crate) fn set_rudp_pointer_locked(&self, locked: bool) {
        self.rudp_pointer_locked.store(locked, Ordering::Relaxed);
    }
}
