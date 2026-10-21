//! Compositor IPC command handlers (`handle_ipc` / `handle_ipc_with_caps`).
//! Capability gating stays in `ipc_dispatch`; this module owns the match body.
//! Sibling `impl MetisState` extracted from `state.rs` (mechanical split; no
//! behavior change).

use metis_protocol::CompositorCommand;

use crate::ipc_dispatch::IpcCaps;
use crate::state::MetisState;

impl MetisState {
    pub fn handle_ipc(&mut self, cmd: CompositorCommand) -> metis_protocol::CompositorEvent {
        self.handle_ipc_with_caps(cmd, IpcCaps::Full)
    }

    pub fn handle_ipc_with_caps(
        &mut self,
        cmd: CompositorCommand,
        caps: IpcCaps,
    ) -> metis_protocol::CompositorEvent {
        use metis_protocol::CompositorEvent;
        // Widgets allowlist + lock denylist (pure helpers in ipc_dispatch).
        if let crate::ipc_dispatch::IpcGate::Reject(message) =
            crate::ipc_dispatch::gate_ipc_command(&cmd, caps, self.session_is_locked())
        {
            return CompositorEvent::Error {
                message: message.into(),
            };
        }
        match cmd {
            CompositorCommand::Ping => CompositorEvent::Pong,
            CompositorCommand::GetMonitor => CompositorEvent::Monitor { rect: self.monitor },
            CompositorCommand::ListOutputs => {
                let cfg = self.output_runtime.cached();
                let mirror_source = self.resolve_mirror_source_name();
                let primary = if self.mirror_mode_active() {
                    mirror_source.clone()
                } else {
                    cfg.primary_output.clone().or_else(|| {
                        self.space
                            .outputs()
                            .find(|o| o.name() != "metis-render")
                            .map(|o| o.name())
                    })
                };
                let mut outputs: Vec<_> = self.connected_outputs();
                outputs.sort_by(|a, b| {
                    let a_pri = primary.as_deref() == Some(a.name().as_str());
                    let b_pri = primary.as_deref() == Some(b.name().as_str());
                    b_pri.cmp(&a_pri).then_with(|| {
                        let a_key = crate::output_prefs::output_geometry(self, a)
                            .map(|g| (g.loc.x, g.loc.y, a.name()))
                            .unwrap_or((0, 0, a.name()));
                        let b_key = crate::output_prefs::output_geometry(self, b)
                            .map(|g| (g.loc.x, g.loc.y, b.name()))
                            .unwrap_or((0, 0, b.name()));
                        a_key.cmp(&b_key)
                    })
                });
                let mirror_ref = mirror_source.as_deref();
                let primary_ref = primary.as_deref();
                let outputs = outputs
                    .iter()
                    .map(|o| crate::output_prefs::output_info_for(self, o, primary_ref, mirror_ref))
                    .collect();
                CompositorEvent::OutputList { outputs }
            }
            CompositorCommand::ListOutputModes { output } => {
                let (modes, current) = crate::output_modes::list_output_modes(self, &output);
                CompositorEvent::OutputModes { modes, current }
            }
            CompositorCommand::GetLayout => {
                let key = self.primary_key();
                let layout = self
                    .desk(&key)
                    .map(|d| d.layout.clone())
                    .unwrap_or_else(|| self.default_layout.clone());
                CompositorEvent::LayoutChanged {
                    layout,
                    gutter_px: self.gutter_px,
                    metrics: self.grid_metrics(),
                }
            }
            CompositorCommand::ListWindows => {
                // Use the full registry (includes minimized/unmapped). Walking
                // `space.elements()` only sees mapped windows and previously
                // skipped minimized ones, so shell reconcile wiped them from the
                // task dock and the user could not restore them. Hidden scratchpad
                // members are filtered in `annotate_window_infos`.
                let focused = self.focused_window_id();
                let mut windows = self.windows.list();
                self.annotate_window_infos(&mut windows);
                for w in &mut windows {
                    w.focused = focused == Some(w.id);
                }
                // Front-ish order: focused first, then non-minimized, then minimized.
                windows.sort_by_key(|w| {
                    (
                        if focused == Some(w.id) { 0 } else { 1 },
                        if w.minimized { 2 } else { 1 },
                        w.id,
                    )
                });
                CompositorEvent::WindowList { windows }
            }
            CompositorCommand::CaptureWindowThumbs { ids } => {
                for id in &ids {
                    self.queue_window_thumb(*id);
                }
                CompositorEvent::WindowThumbs {
                    thumbs: self.existing_window_thumbs(&ids),
                }
            }
            CompositorCommand::CaptureWorkspaceThumbs { output, workspaces } => {
                for ws in &workspaces {
                    self.queue_workspace_thumb(output.clone(), *ws);
                }
                CompositorEvent::WorkspaceThumbs {
                    thumbs: self.existing_workspace_thumbs(&output, &workspaces),
                }
            }
            CompositorCommand::MoveWindow { id, rect } => {
                self.windows.set_target_rect(id, rect);
                self.apply_window_rect(id);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::CloseWindow { id } => {
                self.close_window(id);
                CompositorEvent::WindowClosed { id }
            }
            CompositorCommand::FocusWindow { id } => match self.windows.get(id).cloned() {
                Some(record) => {
                    self.note_window_focus(id);
                    self.space.raise_element(&record.window, true);
                    let serial = smithay::utils::SERIAL_COUNTER.next_serial();
                    match self.seat.get_keyboard() {
                        Some(keyboard) => {
                            keyboard.set_focus(self, Some(record.window.clone().into()), serial);
                        }
                        _ => {
                            tracing::warn!("FocusWindow: seat has no keyboard");
                        }
                    }
                    self.event_bus.emit(&CompositorEvent::WindowFocused { id });
                    self.schedule_redraw();
                    CompositorEvent::WindowFocused { id }
                }
                _ => CompositorEvent::Error {
                    message: format!("window {id} not found"),
                },
            },
            CompositorCommand::SetMinimized { id, minimized } => {
                if self.windows.get(id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                } else {
                    if minimized {
                        self.minimize_by_id(id);
                    } else {
                        self.activate_window_by_id(id);
                    }
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::ToggleScratchpad => {
                self.toggle_scratchpad();
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SetScratchpad { id, scratchpad } => {
                if self.windows.get(id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                } else {
                    self.set_scratchpad(id, scratchpad);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::ActivateWindow { id } => {
                if self.windows.get(id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {id} not found"),
                    }
                } else {
                    self.activate_window_by_id(id);
                    CompositorEvent::WindowFocused { id }
                }
            }
            CompositorCommand::SetFullscreen { id, enabled } => {
                self.set_fullscreen(id, enabled, None);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::ApplyLayout { layout, gutter_px } => {
                self.apply_grid_layout(layout, gutter_px);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SetTileMode { tile_id, mode } => {
                self.set_tile_mode(&tile_id, mode);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SwitchWorkspace { output, id } => {
                let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                    self.output_under_pointer()
                        .map(|o| o.name())
                        .unwrap_or_else(|| self.primary_key())
                });
                self.switch_workspace_routed(&key, id);
                CompositorEvent::WorkspaceChanged {
                    output: key.clone(),
                    active: self.active_workspace_for(&key),
                    count: self.workspace_count(),
                    ephemeral_remote: self.ephemeral_remote_workspace(&key),
                }
            }
            CompositorCommand::MoveWindowToWorkspace {
                window_id,
                workspace,
            } => {
                if self.windows.get(window_id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {window_id} not found"),
                    }
                } else {
                    self.move_window_to_workspace(window_id, workspace);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::MoveWindowToOutput { window_id, output } => {
                if self.windows.get(window_id).is_none() {
                    CompositorEvent::Error {
                        message: format!("window {window_id} not found"),
                    }
                } else {
                    let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                        self.output_under_pointer()
                            .map(|o| o.name())
                            .unwrap_or_else(|| self.primary_key())
                    });
                    self.move_window_to_output(window_id, &key);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::MoveWorkspaceToOutput {
                output,
                workspace,
                target_output,
            } => {
                if target_output.is_empty() {
                    CompositorEvent::Error {
                        message: "target_output is required".into(),
                    }
                } else if self.workspace_mode() != metis_config::WorkspaceMode::Separate {
                    CompositorEvent::Error {
                        message: "MoveWorkspaceToOutput requires independent per-output workspaces"
                            .into(),
                    }
                } else {
                    let source = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                        self.output_under_pointer()
                            .map(|o| o.name())
                            .unwrap_or_else(|| self.primary_key())
                    });
                    let ws = workspace.unwrap_or_else(|| self.active_workspace_for(&source));
                    self.move_workspace_to_output(&source, ws, &target_output);
                    CompositorEvent::LayoutApplied
                }
            }
            CompositorCommand::SetWorkspaceLayout {
                output,
                workspace,
                kind,
            } => {
                let key = output.filter(|o| !o.is_empty()).unwrap_or_else(|| {
                    self.output_under_pointer()
                        .map(|o| o.name())
                        .unwrap_or_else(|| self.primary_key())
                });
                // A specific non-active workspace is set quietly (it's hidden);
                // otherwise act on the output's active workspace (rebuilds the
                // strip + repositions live).
                match workspace {
                    Some(ws) if ws != self.active_workspace_for(&key) => {
                        self.set_layout_kind_on(&key, ws, kind);
                    }
                    _ => self.set_layout_kind(&key, kind),
                }
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SetDefaultLayout { kind } => {
                self.set_layout_kind_all(kind);
                CompositorEvent::LayoutApplied
            }
            CompositorCommand::SubscribeEvents => CompositorEvent::Pong,
            CompositorCommand::Launch { argv, program } => {
                let argv = metis_protocol::launch_argv(&argv, &program);
                self.spawn_client_argv(&argv);
                CompositorEvent::Pong
            }
            CompositorCommand::EndSession => {
                tracing::info!("EndSession requested");
                self.end_compositor_session();
                CompositorEvent::Pong
            }
            CompositorCommand::ApplyBackground => {
                self.wallpaper.apply_config();
                let (full, regions) = self.wallpaper_layout();
                self.wallpaper.set_layout(full, regions);
                self.wallpaper.start_async_decode();
                // Force a re-sample once the new compose lands.
                self.ambient.last_gen = 0;
                self.ambient.last_rgb = None;
                self.damaged = true;
                self.request_redraw();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadInput => {
                let cfg = self.input_runtime.reload_from_disk();
                crate::device_input::apply_keyboard(self, &cfg);
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadCursors => {
                self.reload_cursors_from_config();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadKeybinds => {
                self.keybinds.reload();
                CompositorEvent::Pong
            }
            CompositorCommand::SetKeybindCapture { active } => {
                crate::keybinds::set_capture_active(active);
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadOutputs => {
                self.schedule_outputs_reload();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadPower => {
                let cfg = metis_config::load_power_config();
                self.idle.set_blank_after_minutes(cfg.blank_after_minutes);
                self.idle_reschedule();
                if self.battery_dim.apply_config(cfg.dim_on_battery) {
                    self.damaged = true;
                    self.request_redraw();
                }
                if self.battery_dim.poll_battery_now() {
                    self.damaged = true;
                    self.request_redraw();
                }
                tracing::info!(
                    blank_after_minutes = cfg.blank_after_minutes,
                    dim_on_battery = cfg.dim_on_battery,
                    on_battery = self.battery_dim.on_battery,
                    "reloaded power config; idle blank + battery dim updated"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadRudp => {
                self.reload_rudp_host();
                CompositorEvent::Pong
            }
            CompositorCommand::LockSession => {
                self.lock_session();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadLock => {
                self.lock_reload();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadGaming => {
                self.gaming_config = metis_config::load_gaming_config();
                tracing::info!(
                    graphics_mode = ?self.gaming_config.graphics_mode,
                    "reloaded gaming config"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadDecorations => {
                self.decoration_overrides.reload();
                self.refresh_all_window_decoration_modes();
                CompositorEvent::Pong
            }
            CompositorCommand::ReloadLocale => {
                metis_i18n::reload();
                self.lock.clear_gpu_cache();
                tracing::info!(
                    locale = %metis_i18n::locale_info().tag,
                    "reloaded locale; cleared lock text cache"
                );
                CompositorEvent::Pong
            }
            CompositorCommand::SetClipboard {
                mime,
                text,
                image_path,
            } => {
                if let Err(message) = self.set_clipboard_from_command(mime, text, image_path) {
                    CompositorEvent::Error { message }
                } else {
                    CompositorEvent::Pong
                }
            }
            CompositorCommand::InhibitIdle {
                cookie,
                app_name,
                reason,
            } => {
                let label = match (app_name, reason) {
                    (Some(app), Some(r)) => format!("{app}: {r}"),
                    (Some(app), None) => app,
                    (None, Some(r)) => r,
                    (None, None) => "external".to_string(),
                };
                self.idle_add_external_inhibitor(cookie, label);
                CompositorEvent::Pong
            }
            CompositorCommand::UninhibitIdle { cookie } => {
                self.idle_remove_external_inhibitor(cookie);
                CompositorEvent::Pong
            }
            CompositorCommand::BeginCaptureOverlay { app_id } => {
                self.begin_capture_overlay_portal(app_id);
                CompositorEvent::Pong
            }
            CompositorCommand::EndCaptureOverlay { app_id } => {
                self.end_capture_overlay_portal(app_id);
                CompositorEvent::Pong
            }
            CompositorCommand::BeginScreenshotOverlay => {
                self.begin_screenshot_overlay();
                CompositorEvent::Pong
            }
            CompositorCommand::EndScreenshotOverlay => {
                self.end_screenshot_overlay();
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerAbsolute { x, y } => {
                self.inject_remote_pointer_absolute(x, y);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerRelative { dx, dy } => {
                self.inject_remote_pointer_relative(dx, dy);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerButton { button, pressed } => {
                self.inject_remote_pointer_button(button, pressed);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemotePointerScroll { dx, dy } => {
                self.inject_remote_pointer_scroll(dx, dy);
                CompositorEvent::Pong
            }
            CompositorCommand::InjectRemoteKey { keycode, pressed } => {
                self.inject_remote_key(keycode, pressed);
                CompositorEvent::Pong
            }
        }
    }
}
