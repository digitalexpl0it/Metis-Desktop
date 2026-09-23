//! Native Metis screenshot overlay (Phase 12).

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_capture::{CaptureOptions, capture_png};
use metis_config::{
    AfterCaptureAction, ScreenshotConfig, ScreenshotMode, expand_save_dir, load_screenshot_config,
    parse_hex_rgb, save_default_screenshot_config, save_screenshot_config,
};
use metis_protocol::{CompositorCommand, OutputInfo, PixelRect, WindowInfo};

use crate::ui::theme::active_tokens;

use crate::services::{BarNotification, NotificationKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    Interactive,
    InstantFull,
    Window,
    Record,
}

#[derive(Debug, Clone, Copy, Default)]
struct DragRect {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl DragRect {
    fn normalized(&self) -> PixelRect {
        let x = self.x1.min(self.x2).round() as i32;
        let y = self.y1.min(self.y2).round() as i32;
        let width = (self.x2 - self.x1).abs().round() as i32;
        let height = (self.y2 - self.y1).abs().round() as i32;
        PixelRect {
            x,
            y,
            width,
            height,
        }
    }

    fn valid(&self) -> bool {
        let r = self.normalized();
        r.width >= 4 && r.height >= 4
    }
}

struct Overlay {
    window: gtk::Window,
    canvas: gtk::DrawingArea,
    size_label: gtk::Label,
    toolbar: gtk::Box,
    mode: Cell<ScreenshotMode>,
    draw_cursor: Cell<bool>,
    delay_seconds: Cell<u32>,
    after_capture: Cell<AfterCaptureAction>,
    drag: RefCell<Option<DragRect>>,
    hover_window: RefCell<Option<PixelRect>>,
    /// Last window highlighted in Window mode — kept when the pointer leaves the
    /// canvas (e.g. moving to the Capture button).
    picked_window: RefCell<Option<PixelRect>>,
    /// Click on a window locks the pick until mode changes or empty canvas click.
    window_locked: Cell<bool>,
    monitor_origin: (i32, i32),
    output_index: usize,
    connector: Option<String>,
    /// When true, Capture hands the selection rect to `metis-screenshot --mode record`.
    record_handoff: Cell<bool>,
    config: ScreenshotConfig,
    windows: RefCell<Vec<WindowInfo>>,
}

thread_local! {
    static OVERLAY: RefCell<Option<Rc<Overlay>>> = const { RefCell::new(None) };
}

pub fn init() {
    if let Err(err) = save_default_screenshot_config() {
        tracing::warn!(%err, "failed to write default screenshot.json");
    }
}

#[allow(dead_code)] // screenshot overlay state query
pub fn is_active() -> bool {
    OVERLAY.with(|o| o.borrow().is_some())
}

pub fn on_theme_changed() {
    OVERLAY.with(|o| {
        if let Some(overlay) = o.borrow().as_ref() {
            overlay.canvas.queue_draw();
            overlay.toolbar.queue_draw();
        }
    });
}

pub fn show(mode: LaunchMode, connector: Option<String>) {
    if OVERLAY.with(|o| o.borrow().is_some()) {
        return;
    }
    // Leave bar popovers, Control Center, and Notification Center mapped so
    // hide-before-capture can include them. The Overlay picker paints above
    // and owns the keyboard via Exclusive layer-shell.

    let config = load_screenshot_config();
    let connector = resolve_connector(connector, &config);
    match mode {
        LaunchMode::InstantFull => {
            run_instant_capture(ScreenshotMode::Screen, config, connector);
        }
        LaunchMode::Window => {
            show_interactive(ScreenshotMode::Window, config, connector);
        }
        LaunchMode::Record => {
            show_interactive_record(config, connector);
        }
        LaunchMode::Interactive => {
            show_interactive(config.default_mode, config, connector);
        }
    }
}

fn show_interactive(
    initial_mode: ScreenshotMode,
    config: ScreenshotConfig,
    connector: Option<String>,
) {
    let _ = send_compositor(CompositorCommand::BeginScreenshotOverlay);
    // Park Exclusive Overlay shell chrome under this picker so it stays visible
    // for capture without covering the selection UI or stealing keyboard focus.
    crate::ui::notification_center::set_below_screenshot(true);
    crate::ui::dashboard::set_below_screenshot(true);
    crate::ui::bar::widgets::set_menu_below_screenshot(true);

    let (monitor_origin, output_index, connector) = monitor_context(connector.as_deref());
    let windows = list_windows_best_effort();

    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Screenshot"))
        .decorated(false)
        .build();
    window.add_css_class("metis-screenshot-window");
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-screenshot"));
    if let Some(monitor) = gdk_monitor_for_output(&connector, monitor_origin) {
        window.set_monitor(Some(&monitor));
    }
    for edge in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true);
        window.set_exclusive_zone(-1);
        window.set_margin(edge, 0);
    }

    let chrome = gtk::Overlay::new();
    window.set_child(Some(&chrome));

    let canvas = gtk::DrawingArea::new();
    canvas.add_css_class("metis-screenshot-canvas");
    canvas.set_hexpand(true);
    canvas.set_vexpand(true);
    canvas.set_can_focus(true);
    chrome.set_child(Some(&canvas));

    let size_label = gtk::Label::new(None);
    size_label.add_css_class("metis-screenshot-size");
    size_label.set_visible(false);
    chrome.add_overlay(&size_label);

    let toolbar_wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    toolbar_wrap.add_css_class("metis-screenshot-toolbar-wrap");
    toolbar_wrap.set_halign(gtk::Align::Center);
    toolbar_wrap.set_valign(gtk::Align::End);
    toolbar_wrap.set_margin_bottom(28);
    chrome.add_overlay(&toolbar_wrap);

    let interactive_action = config.interactive_action();
    let (toolbar, mode_buttons, options_widgets, capture_btn) = build_toolbar(
        initial_mode,
        config.draw_cursor,
        config.delay_seconds,
        interactive_action,
    );
    toolbar_wrap.append(&toolbar);

    let overlay = Rc::new(Overlay {
        window: window.clone(),
        canvas: canvas.clone(),
        size_label: size_label.clone(),
        toolbar: toolbar.clone(),
        mode: Cell::new(initial_mode),
        draw_cursor: Cell::new(config.draw_cursor),
        delay_seconds: Cell::new(config.delay_seconds),
        after_capture: Cell::new(interactive_action),
        drag: RefCell::new(None),
        hover_window: RefCell::new(None),
        picked_window: RefCell::new(None),
        window_locked: Cell::new(false),
        monitor_origin,
        output_index,
        connector,
        record_handoff: Cell::new(false),
        config,
        windows: RefCell::new(windows),
    });

    wire_canvas(&overlay, &canvas, &size_label);
    wire_toolbar(&overlay, &mode_buttons, &options_widgets, capture_btn);
    wire_keyboard(&window, &chrome);

    canvas.set_draw_func({
        let overlay = overlay.clone();
        move |_area, cr, width, height| {
            draw_scene(&overlay, cr, width, height);
        }
    });

    OVERLAY.with(|o| *o.borrow_mut() = Some(overlay.clone()));

    glib::idle_add_local_once(move || {
        window.set_visible(true);
        window.present();
        if !window.grab_focus() {
            canvas.grab_focus();
        }
    });
}

fn show_interactive_record(config: ScreenshotConfig, connector: Option<String>) {
    show_interactive(ScreenshotMode::Selection, config, connector);
    OVERLAY.with(|o| {
        if let Some(overlay) = o.borrow().as_ref() {
            overlay.record_handoff.set(true);
            // Prefer a capture button label that matches the handoff.
            if let Some(child) = overlay.toolbar.last_child()
                && let Ok(btn) = child.downcast::<gtk::Button>()
            {
                btn.set_label(&metis_i18n::tr("Record"));
            }
        }
    });
}

#[derive(Clone)]
struct ModeButtons {
    selection: gtk::ToggleButton,
    screen: gtk::ToggleButton,
    window: gtk::ToggleButton,
}

struct OptionsWidgets {
    pointer: gtk::Switch,
    delay: gtk::SpinButton,
    after_copy: gtk::ToggleButton,
    after_save: gtk::ToggleButton,
    after_both: gtk::ToggleButton,
    after_edit: gtk::ToggleButton,
}

fn build_toolbar(
    initial_mode: ScreenshotMode,
    draw_cursor: bool,
    delay_seconds: u32,
    after_capture: AfterCaptureAction,
) -> (gtk::Box, ModeButtons, OptionsWidgets, gtk::Button) {
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    toolbar.add_css_class("metis-screenshot-toolbar");

    let mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    mode_box.add_css_class("metis-screenshot-mode");

    let selection_btn = mode_button("edit-select-symbolic", &metis_i18n::tr("Selection"));
    let screen_btn = mode_button("view-fullscreen-symbolic", &metis_i18n::tr("Full screen"));
    let window_btn = mode_button("window-new-symbolic", &metis_i18n::tr("Window"));
    mode_box.append(&selection_btn);
    mode_box.append(&screen_btn);
    mode_box.append(&window_btn);
    toolbar.append(&mode_box);

    let options_btn = gtk::MenuButton::new();
    options_btn.set_icon_name("preferences-system-symbolic");
    options_btn.add_css_class("metis-screenshot-icon");
    options_btn.set_tooltip_text(Some(&metis_i18n::tr("Options")));

    let pointer_switch = gtk::Switch::new();
    pointer_switch.set_active(draw_cursor);
    let delay_spin = gtk::SpinButton::with_range(0.0, 30.0, 1.0);
    delay_spin.set_value(delay_seconds as f64);
    delay_spin.set_digits(0);
    let after_seg = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    after_seg.add_css_class("metis-screenshot-after-seg");
    after_seg.add_css_class("linked");

    let (after_edit, after_copy, after_save, after_both) = {
        let labels = [
            metis_i18n::tr("Edit"),
            metis_i18n::tr("Copy"),
            metis_i18n::tr("Save"),
            metis_i18n::tr("Both"),
        ];
        let mut buttons = Vec::new();
        let mut leader: Option<gtk::ToggleButton> = None;
        for label in labels {
            let btn = gtk::ToggleButton::with_label(&label);
            btn.add_css_class("metis-screenshot-after-btn");
            if let Some(ref head) = leader {
                btn.set_group(Some(head));
            } else {
                leader = Some(btn.clone());
            }
            after_seg.append(&btn);
            buttons.push(btn);
        }
        (
            buttons[0].clone(),
            buttons[1].clone(),
            buttons[2].clone(),
            buttons[3].clone(),
        )
    };
    sync_after_buttons(
        &after_copy,
        &after_save,
        &after_both,
        &after_edit,
        after_capture,
    );

    let options_widgets = OptionsWidgets {
        pointer: pointer_switch.clone(),
        delay: delay_spin.clone(),
        after_copy: after_copy.clone(),
        after_save: after_save.clone(),
        after_both: after_both.clone(),
        after_edit: after_edit.clone(),
    };

    let popover = build_options_popover(&pointer_switch, &delay_spin, &after_seg);
    options_btn.set_popover(Some(&popover));
    toolbar.append(&options_btn);

    let record_btn = gtk::Button::from_icon_name("media-record-symbolic");
    record_btn.add_css_class("metis-screenshot-icon");
    record_btn.set_tooltip_text(Some(&metis_i18n::tr("Record selection")));
    toolbar.append(&record_btn);

    let capture_btn = gtk::Button::with_label(&metis_i18n::tr("Capture"));
    capture_btn.add_css_class("metis-screenshot-capture");
    toolbar.append(&capture_btn);

    let mode_buttons = ModeButtons {
        selection: selection_btn,
        screen: screen_btn,
        window: window_btn,
    };
    sync_mode_buttons(&mode_buttons, initial_mode);

    // Wire record as a one-shot handoff without a mode enum value on the picker.
    {
        let capture_btn = capture_btn.clone();
        record_btn.connect_clicked(move |_| {
            OVERLAY.with(|o| {
                if let Some(overlay) = o.borrow().as_ref() {
                    overlay.record_handoff.set(true);
                    overlay.mode.set(ScreenshotMode::Selection);
                    capture_btn.set_label(&metis_i18n::tr("Record"));
                    toast_message(&metis_i18n::tr("Drag a region, then Record"));
                }
            });
        });
    }

    (toolbar, mode_buttons, options_widgets, capture_btn)
}

fn mode_button(icon_name: &str, tooltip: &str) -> gtk::ToggleButton {
    let btn = gtk::ToggleButton::new();
    let image = crate::ui::icons::image(icon_name);
    image.set_pixel_size(18);
    btn.set_child(Some(&image));
    btn.set_tooltip_text(Some(tooltip));
    btn.add_css_class("metis-screenshot-mode-btn");
    // Square hit target so border-radius: 999px reads as a circle, not a stretched pill.
    btn.set_size_request(36, 36);
    btn.set_halign(gtk::Align::Center);
    btn.set_valign(gtk::Align::Center);
    btn
}

fn sync_mode_buttons(buttons: &ModeButtons, mode: ScreenshotMode) {
    buttons
        .selection
        .set_active(mode == ScreenshotMode::Selection);
    buttons.screen.set_active(mode == ScreenshotMode::Screen);
    buttons.window.set_active(mode == ScreenshotMode::Window);
}

fn sync_after_buttons(
    copy: &gtk::ToggleButton,
    save: &gtk::ToggleButton,
    both: &gtk::ToggleButton,
    edit: &gtk::ToggleButton,
    action: AfterCaptureAction,
) {
    copy.set_active(action == AfterCaptureAction::Copy);
    save.set_active(action == AfterCaptureAction::Save);
    both.set_active(action == AfterCaptureAction::CopyAndSave);
    edit.set_active(matches!(
        action,
        AfterCaptureAction::Edit | AfterCaptureAction::Open
    ));
}

fn after_from_buttons(
    copy: &gtk::ToggleButton,
    save: &gtk::ToggleButton,
    both: &gtk::ToggleButton,
    edit: &gtk::ToggleButton,
) -> AfterCaptureAction {
    if edit.is_active() {
        AfterCaptureAction::Edit
    } else if save.is_active() {
        AfterCaptureAction::Save
    } else if both.is_active() {
        AfterCaptureAction::CopyAndSave
    } else if copy.is_active() {
        AfterCaptureAction::Copy
    } else {
        AfterCaptureAction::Edit
    }
}

fn build_options_popover(
    pointer_switch: &gtk::Switch,
    delay_spin: &gtk::SpinButton,
    after_seg: &gtk::Box,
) -> gtk::Popover {
    let popover = gtk::Popover::new();
    popover.add_css_class("metis-screenshot-popover");
    popover.set_has_arrow(true);

    let panel = gtk::Box::new(gtk::Orientation::Vertical, 10);
    panel.add_css_class("metis-screenshot-options");
    panel.set_margin_start(12);
    panel.set_margin_end(12);
    panel.set_margin_top(10);
    panel.set_margin_bottom(10);
    panel.set_size_request(280, -1);

    panel.append(&option_row(
        &metis_i18n::tr("Include pointer"),
        pointer_switch,
    ));
    panel.append(&option_row(&metis_i18n::tr("Delay (seconds)"), delay_spin));

    let after_label = gtk::Label::new(Some(&metis_i18n::tr("After capture")));
    after_label.set_halign(gtk::Align::Start);
    after_label.add_css_class("metis-screenshot-option-label");
    panel.append(&after_label);
    panel.append(after_seg);

    popover.set_child(Some(&panel));
    popover
}

fn option_row(label: &str, control: &impl IsA<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let title = gtk::Label::new(Some(label));
    title.set_hexpand(true);
    title.set_halign(gtk::Align::Start);
    title.add_css_class("metis-screenshot-option-label");
    row.append(&title);
    row.append(control);
    row
}

fn wire_after_buttons(
    overlay: &Rc<Overlay>,
    copy: &gtk::ToggleButton,
    save: &gtk::ToggleButton,
    both: &gtk::ToggleButton,
    edit: &gtk::ToggleButton,
) {
    let sync = {
        let overlay = overlay.clone();
        let copy = copy.clone();
        let save = save.clone();
        let both = both.clone();
        let edit = edit.clone();
        move || {
            // Only commit when a button is active. During radio-group switches GTK
            // fires the deactivation first; treating that as Copy would overwrite
            // the Edit default.
            if !copy.is_active() && !save.is_active() && !both.is_active() && !edit.is_active() {
                return;
            }
            overlay
                .after_capture
                .set(after_from_buttons(&copy, &save, &both, &edit));
            persist_settings(&overlay);
        }
    };
    copy.connect_toggled({
        let sync = sync.clone();
        move |btn| {
            if btn.is_active() {
                sync();
            }
        }
    });
    save.connect_toggled({
        let sync = sync.clone();
        move |btn| {
            if btn.is_active() {
                sync();
            }
        }
    });
    both.connect_toggled({
        let sync = sync.clone();
        move |btn| {
            if btn.is_active() {
                sync();
            }
        }
    });
    edit.connect_toggled({
        let sync = sync.clone();
        move |btn| {
            if btn.is_active() {
                sync();
            }
        }
    });
}

fn persist_settings(overlay: &Overlay) {
    let cfg = ScreenshotConfig {
        default_mode: overlay.mode.get(),
        draw_cursor: overlay.draw_cursor.get(),
        delay_seconds: overlay.delay_seconds.get(),
        after_capture: overlay.after_capture.get(),
        interactive_after_capture: None,
        instant_after_capture: overlay.config.instant_after_capture,
        prefer_pointer_output: overlay.config.prefer_pointer_output,
        save_dir: overlay.config.save_dir.clone(),
    };
    if let Err(err) = save_screenshot_config(&cfg) {
        tracing::warn!(%err, "failed to save screenshot.json");
    }
}

fn wire_toolbar(
    overlay: &Rc<Overlay>,
    mode_buttons: &ModeButtons,
    options: &OptionsWidgets,
    capture_btn: gtk::Button,
) {
    options.pointer.connect_active_notify({
        let overlay = overlay.clone();
        move |sw| {
            overlay.draw_cursor.set(sw.is_active());
            persist_settings(&overlay);
        }
    });
    options.delay.connect_value_changed({
        let overlay = overlay.clone();
        move |spin| {
            overlay
                .delay_seconds
                .set(spin.value().max(0.0).round() as u32);
            persist_settings(&overlay);
        }
    });
    wire_after_buttons(
        overlay,
        &options.after_copy,
        &options.after_save,
        &options.after_both,
        &options.after_edit,
    );

    let apply_mode = {
        let overlay = overlay.clone();
        let mode_buttons = ModeButtons {
            selection: mode_buttons.selection.clone(),
            screen: mode_buttons.screen.clone(),
            window: mode_buttons.window.clone(),
        };
        move |mode: ScreenshotMode| {
            overlay.mode.set(mode);
            overlay.record_handoff.set(false);
            sync_mode_buttons(&mode_buttons, mode);
            overlay.drag.replace(None);
            overlay.hover_window.replace(None);
            overlay.picked_window.replace(None);
            overlay.window_locked.set(false);
            overlay.canvas.queue_draw();
            overlay.size_label.set_visible(false);
            persist_settings(&overlay);
        }
    };

    let wire_mode = |btn: &gtk::ToggleButton, mode: ScreenshotMode| {
        let overlay = overlay.clone();
        let mode_buttons = ModeButtons {
            selection: mode_buttons.selection.clone(),
            screen: mode_buttons.screen.clone(),
            window: mode_buttons.window.clone(),
        };
        let apply_mode = apply_mode.clone();
        btn.connect_toggled(move |btn| {
            if btn.is_active() {
                apply_mode(mode);
            } else {
                sync_mode_buttons(&mode_buttons, overlay.mode.get());
            }
        });
    };
    wire_mode(&mode_buttons.selection, ScreenshotMode::Selection);
    wire_mode(&mode_buttons.screen, ScreenshotMode::Screen);
    wire_mode(&mode_buttons.window, ScreenshotMode::Window);

    capture_btn.connect_clicked({
        let overlay = overlay.clone();
        move |_| overlay.clone().request_capture()
    });
}

fn on_overlay_key(key: gdk::Key, modifier: gdk::ModifierType) -> glib::Propagation {
    if key == gdk::Key::Escape {
        dismiss();
        glib::Propagation::Stop
    } else if key == gdk::Key::Return || key == gdk::Key::KP_Enter {
        if !modifier.contains(gdk::ModifierType::SHIFT_MASK) {
            if let Some(overlay) = OVERLAY.with(|o| o.borrow().clone()) {
                overlay.request_capture();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    } else {
        glib::Propagation::Proceed
    }
}

fn wire_keyboard(window: &gtk::Window, root: &gtk::Overlay) {
    for widget in [
        window.upcast_ref::<gtk::Widget>(),
        root.upcast_ref::<gtk::Widget>(),
    ] {
        let controller = gtk::EventControllerKey::new();
        controller.set_propagation_phase(gtk::PropagationPhase::Capture);
        controller.connect_key_pressed(|_, key, _, modifier| on_overlay_key(key, modifier));
        widget.add_controller(controller);
    }
}

fn wire_canvas(overlay: &Rc<Overlay>, canvas: &gtk::DrawingArea, size_label: &gtk::Label) {
    let gesture = gtk::GestureDrag::new();
    gesture.set_button(0);
    gesture.connect_drag_begin({
        let overlay = overlay.clone();
        move |_, x, y| {
            if !matches!(overlay.mode.get(), ScreenshotMode::Selection) {
                return;
            }
            overlay.drag.replace(Some(DragRect {
                x1: x,
                y1: y,
                x2: x,
                y2: y,
            }));
            overlay.size_label.set_visible(false);
        }
    });
    gesture.connect_drag_update({
        let overlay = overlay.clone();
        let size_label = size_label.clone();
        move |_, offset_x, offset_y| {
            if !matches!(overlay.mode.get(), ScreenshotMode::Selection) {
                return;
            }
            let Some(mut drag) = *overlay.drag.borrow() else {
                return;
            };
            drag.x2 = drag.x1 + offset_x;
            drag.y2 = drag.y1 + offset_y;
            overlay.drag.replace(Some(drag));
            if drag.valid() {
                let rect = drag.normalized();
                size_label.set_text(&format!("{} × {}", rect.width, rect.height));
                size_label.set_visible(true);
                position_size_label(&size_label, &drag);
            }
            overlay.canvas.queue_draw();
        }
    });
    canvas.add_controller(gesture);

    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion({
        let overlay = overlay.clone();
        move |_, x, y| {
            if overlay.mode.get() == ScreenshotMode::Window {
                let hit = window_at(&overlay, x, y);
                if overlay.window_locked.get() {
                    overlay.hover_window.replace(hit);
                } else {
                    overlay.hover_window.replace(hit);
                    if hit.is_some() {
                        overlay.picked_window.replace(hit);
                    }
                }
                overlay.canvas.queue_draw();
            }
        }
    });
    canvas.add_controller(motion);

    let click = gtk::GestureClick::new();
    click.set_button(0);
    click.connect_pressed({
        let overlay = overlay.clone();
        move |_, _n, x, y| {
            if overlay.mode.get() == ScreenshotMode::Window {
                if let Some(rect) = window_at(&overlay, x, y) {
                    overlay.picked_window.replace(Some(rect));
                    overlay.hover_window.replace(Some(rect));
                    overlay.window_locked.set(true);
                    overlay.canvas.queue_draw();
                } else {
                    overlay.window_locked.set(false);
                    overlay.picked_window.replace(None);
                    overlay.hover_window.replace(None);
                    overlay.canvas.queue_draw();
                }
            }
        }
    });
    canvas.add_controller(click);

    let right = gtk::GestureClick::new();
    right.set_button(3);
    right.connect_pressed(|_, _, _, _| dismiss());
    canvas.add_controller(right);
}

impl Overlay {
    fn request_capture(self: Rc<Self>) {
        let crop = self.capture_rect();
        let Some(crop) = crop else {
            let msg = match self.mode.get() {
                ScreenshotMode::Window => metis_i18n::tr("Select a window to capture"),
                _ => metis_i18n::tr("Select an area to capture"),
            };
            toast_message(&msg);
            return;
        };
        let draw_cursor = self.draw_cursor.get();
        let delay = self.delay_seconds.get();
        let after_capture = self.after_capture.get();
        let config = self.config.clone();
        let output_index = self.output_index;
        let connector = self.connector.clone();
        let record = self.record_handoff.get();
        self.window.set_visible(false);

        glib::timeout_add_local_once(Duration::from_millis(80), move || {
            if record {
                dismiss();
                launch_recorder(&config, connector.as_deref(), crop);
                return;
            }
            std::thread::spawn(move || {
                if delay > 0 {
                    std::thread::sleep(Duration::from_secs(delay as u64));
                }
                let result = perform_capture(
                    crop,
                    draw_cursor,
                    output_index,
                    connector.as_deref(),
                    &config,
                );
                glib::idle_add_once(move || {
                    dismiss();
                    match result {
                        Ok(path) => after_capture_action(&config, &path, after_capture),
                        Err(err) => {
                            tracing::warn!(%err, "screenshot capture failed");
                            toast_message(&metis_i18n::tr("Screenshot failed"));
                        }
                    }
                });
            });
        });
    }

    fn capture_rect(&self) -> Option<PixelRect> {
        match self.mode.get() {
            ScreenshotMode::Selection => {
                let drag = (*self.drag.borrow())?;
                if drag.valid() {
                    let local = drag.normalized();
                    Some(PixelRect {
                        x: self.monitor_origin.0 + local.x,
                        y: self.monitor_origin.1 + local.y,
                        width: local.width,
                        height: local.height,
                    })
                } else {
                    None
                }
            }
            ScreenshotMode::Screen => {
                let (mx, my) = self.monitor_origin;
                let w = self.canvas.width();
                let h = self.canvas.height();
                Some(PixelRect {
                    x: mx,
                    y: my,
                    width: w,
                    height: h,
                })
            }
            ScreenshotMode::Window => {
                if self.window_locked.get() {
                    *self.picked_window.borrow()
                } else {
                    (*self.picked_window.borrow()).or_else(|| *self.hover_window.borrow())
                }
            }
        }
    }
}

fn draw_scene(overlay: &Overlay, cr: &gtk::cairo::Context, width: i32, height: i32) {
    let tokens = active_tokens();
    let is_light = tokens.mode.eq_ignore_ascii_case("light");
    cr.set_source_rgba(0.0, 0.0, 0.0, if is_light { 0.28 } else { 0.45 });
    cr.paint().ok();

    let highlight = match overlay.mode.get() {
        ScreenshotMode::Selection => (*overlay.drag.borrow())
            .filter(|d| d.valid())
            .map(|d| d.normalized()),
        ScreenshotMode::Screen => Some(PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        }),
        ScreenshotMode::Window => window_highlight_rect(overlay),
    };

    if let Some(rect) = highlight {
        cr.set_operator(gtk::cairo::Operator::Clear);
        cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        cr.fill().ok();
        cr.set_operator(gtk::cairo::Operator::Over);

        let (ar, ag, ab) = {
            let rgb = parse_hex_rgb(tokens.accent_primary());
            (
                rgb[0] as f64 / 255.0,
                rgb[1] as f64 / 255.0,
                rgb[2] as f64 / 255.0,
            )
        };
        cr.set_source_rgb(ar, ag, ab);
        cr.set_line_width(2.0);
        cr.set_dash(&[8.0, 6.0], 0.0);
        cr.rectangle(
            rect.x as f64,
            rect.y as f64,
            rect.width as f64,
            rect.height as f64,
        );
        cr.stroke().ok();
        cr.set_dash(&[], 0.0);
    }
}

fn position_size_label(label: &gtk::Label, drag: &DragRect) {
    let rect = drag.normalized();
    label.set_margin_start((rect.x + 8).max(0));
    label.set_margin_top((rect.y + 8).max(0));
    label.set_halign(gtk::Align::Start);
    label.set_valign(gtk::Align::Start);
}

fn window_highlight_rect(overlay: &Overlay) -> Option<PixelRect> {
    let global = if overlay.window_locked.get() {
        *overlay.picked_window.borrow()
    } else {
        (*overlay.picked_window.borrow()).or_else(|| *overlay.hover_window.borrow())
    }?;
    Some(PixelRect {
        x: global.x - overlay.monitor_origin.0,
        y: global.y - overlay.monitor_origin.1,
        width: global.width,
        height: global.height,
    })
}

fn window_at(overlay: &Overlay, x: f64, y: f64) -> Option<PixelRect> {
    let gx = overlay.monitor_origin.0 + x.round() as i32;
    let gy = overlay.monitor_origin.1 + y.round() as i32;
    overlay
        .windows
        .borrow()
        .iter()
        .filter(|w| !w.minimized)
        .find(|w| point_in_rect(gx, gy, w.rect))
        .map(|w| w.rect)
}

fn point_in_rect(x: i32, y: i32, rect: PixelRect) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height
}

fn primary_monitor_geometry() -> gdk::Rectangle {
    if let Some(display) = gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gdk::Monitor>()
    {
        return monitor.geometry();
    }
    gdk::Rectangle::new(0, 0, 1920, 1080)
}

fn run_instant_capture(mode: ScreenshotMode, config: ScreenshotConfig, connector: Option<String>) {
    let (origin, output_index, connector) = monitor_context(connector.as_deref());
    let crop = match mode {
        ScreenshotMode::Screen => PixelRect {
            x: origin.0,
            y: origin.1,
            width: list_outputs_best_effort()
                .get(output_index)
                .map(|o| o.rect.width)
                .unwrap_or(1920),
            height: list_outputs_best_effort()
                .get(output_index)
                .map(|o| o.rect.height)
                .unwrap_or(1080),
        },
        _ => PixelRect {
            x: origin.0,
            y: origin.1,
            width: 1920,
            height: 1080,
        },
    };
    let draw_cursor = config.draw_cursor;
    let action = config.instant_action();
    std::thread::spawn(move || {
        if let Ok(path) = perform_capture(
            crop,
            draw_cursor,
            output_index,
            connector.as_deref(),
            &config,
        ) {
            glib::idle_add_once(move || after_capture_action(&config, &path, action));
        } else {
            tracing::warn!("instant screenshot failed");
            glib::idle_add_once(|| toast_message(&metis_i18n::tr("Screenshot failed")));
        }
    });
}

fn perform_capture(
    crop: PixelRect,
    draw_cursor: bool,
    output_index: usize,
    connector: Option<&str>,
    config: &ScreenshotConfig,
) -> Result<PathBuf, String> {
    let path = capture_path(config);
    let (ox, oy) = output_origin(output_index, connector);
    let local = PixelRect {
        x: crop.x - ox,
        y: crop.y - oy,
        width: crop.width,
        height: crop.height,
    };
    capture_png(
        CaptureOptions {
            draw_cursor,
            output_index,
            connector: connector.map(str::to_string),
        },
        Some(local),
        &path,
    )?;
    Ok(path)
}

fn launch_recorder(config: &ScreenshotConfig, connector: Option<&str>, crop: PixelRect) {
    let (ox, oy) = output_origin(0, connector);
    let local = PixelRect {
        x: crop.x - ox,
        y: crop.y - oy,
        width: crop.width,
        height: crop.height,
    };
    let mut argv = vec![
        "metis-screenshot".into(),
        "--mode".into(),
        "record".into(),
        "--crop".into(),
        format!("{},{},{},{}", local.x, local.y, local.width, local.height),
    ];
    if let Some(name) = connector {
        argv.push("--connector".into());
        argv.push(name.to_string());
    }
    let _ = config; // save_dir used inside recorder defaults
    if let Err(err) = crate::compositor::launch_argv(argv) {
        tracing::warn!(%err, "failed to launch metis-screenshot record");
        toast_message(&metis_i18n::tr("Recording failed to start"));
    } else {
        toast_message(&metis_i18n::tr("Recording… click Stop when done"));
    }
}

fn after_capture_action(config: &ScreenshotConfig, path: &PathBuf, action: AfterCaptureAction) {
    match action {
        AfterCaptureAction::Copy | AfterCaptureAction::CopyAndSave => {
            if let Err(err) = crate::compositor::set_clipboard(
                "image/png".into(),
                None,
                Some(path.display().to_string()),
            ) {
                tracing::warn!(%err, "failed to copy screenshot to clipboard");
            }
        }
        _ => {}
    }
    if matches!(
        action,
        AfterCaptureAction::Save | AfterCaptureAction::CopyAndSave
    ) {
        let save_dir = expand_save_dir(&config.save_dir);
        if let Err(err) = std::fs::create_dir_all(&save_dir) {
            tracing::warn!(%err, "failed to create screenshot save dir");
        } else {
            let dest = save_dir.join(path.file_name().unwrap_or_default());
            if dest != *path
                && let Err(err) = std::fs::copy(path, &dest)
            {
                tracing::warn!(%err, "failed to save screenshot copy");
            }
        }
    }
    if matches!(action, AfterCaptureAction::Open) {
        let _ =
            crate::compositor::launch_argv(["xdg-open".to_string(), path.display().to_string()]);
    }
    if matches!(action, AfterCaptureAction::Edit)
        && let Err(err) = crate::compositor::launch_argv([
            "metis-screenshot".to_string(),
            "--mode".to_string(),
            "edit".to_string(),
            "--path".to_string(),
            path.display().to_string(),
        ])
    {
        tracing::warn!(%err, "failed to launch metis-screenshot editor");
        toast_message(&metis_i18n::tr("Could not open screenshot editor"));
        return;
    }
    toast_message(&metis_i18n::tr("Screenshot captured"));
}

fn toast_message(message: &str) {
    crate::ui::toast::show(&BarNotification::internal(
        NotificationKind::Information,
        metis_i18n::tr("Screenshot"),
        message,
    ));
}

fn capture_path(config: &ScreenshotConfig) -> PathBuf {
    let dir = expand_save_dir(&config.save_dir);
    let _ = std::fs::create_dir_all(&dir);
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dir.join(format!("metis-{stamp}.png"))
}

pub fn dismiss() {
    OVERLAY.with(|o| {
        if let Some(overlay) = o.borrow_mut().take() {
            overlay.window.set_visible(false);
            overlay.window.destroy();
        }
    });
    let _ = send_compositor(CompositorCommand::EndScreenshotOverlay);
    crate::ui::notification_center::set_below_screenshot(false);
    crate::ui::dashboard::set_below_screenshot(false);
    crate::ui::bar::widgets::set_menu_below_screenshot(false);
}

fn resolve_connector(hint: Option<String>, config: &ScreenshotConfig) -> Option<String> {
    if !config.prefer_pointer_output {
        return None;
    }
    hint.filter(|s| !s.trim().is_empty())
}

fn monitor_context(connector: Option<&str>) -> ((i32, i32), usize, Option<String>) {
    let outputs = list_outputs_best_effort();
    if let Some(name) = connector
        && let Some((idx, out)) = outputs
            .iter()
            .enumerate()
            .find(|(_, o)| o.name.eq_ignore_ascii_case(name))
    {
        return ((out.rect.x, out.rect.y), idx, Some(out.name.clone()));
    }
    if let Some((idx, out)) = outputs.iter().enumerate().find(|(_, o)| o.primary) {
        return ((out.rect.x, out.rect.y), idx, Some(out.name.clone()));
    }
    if let Some((idx, out)) = outputs.iter().enumerate().next() {
        return ((out.rect.x, out.rect.y), idx, Some(out.name.clone()));
    }
    let g = primary_monitor_geometry();
    ((g.x(), g.y()), 0, None)
}

fn output_origin(output_index: usize, connector: Option<&str>) -> (i32, i32) {
    let outputs = list_outputs_best_effort();
    if let Some(name) = connector
        && let Some(out) = outputs.iter().find(|o| o.name.eq_ignore_ascii_case(name))
    {
        return (out.rect.x, out.rect.y);
    }
    outputs
        .get(output_index)
        .map(|o| (o.rect.x, o.rect.y))
        .unwrap_or((0, 0))
}

fn gdk_monitor_for_output(connector: &Option<String>, origin: (i32, i32)) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    let monitors = display.monitors();
    let n = monitors.n_items();
    for i in 0..n {
        let Ok(monitor) = monitors.item(i)?.downcast::<gdk::Monitor>() else {
            continue;
        };
        if let Some(name) = connector.as_deref() {
            if monitor
                .connector()
                .map(|c| c.eq_ignore_ascii_case(name))
                .unwrap_or(false)
            {
                return Some(monitor);
            }
            if monitor
                .model()
                .map(|m| m.eq_ignore_ascii_case(name))
                .unwrap_or(false)
            {
                return Some(monitor);
            }
        }
        let g = monitor.geometry();
        if g.x() == origin.0 && g.y() == origin.1 {
            return Some(monitor);
        }
    }
    None
}

fn list_outputs_best_effort() -> Vec<OutputInfo> {
    match send_compositor(CompositorCommand::ListOutputs) {
        Ok(metis_protocol::CompositorEvent::OutputList { outputs }) => outputs,
        _ => Vec::new(),
    }
}

fn list_windows_best_effort() -> Vec<WindowInfo> {
    crate::compositor::list_windows().unwrap_or_default()
}

fn send_compositor(
    cmd: CompositorCommand,
) -> Result<metis_protocol::CompositorEvent, std::io::Error> {
    metis_protocol::send_compositor_command(&cmd)
}
