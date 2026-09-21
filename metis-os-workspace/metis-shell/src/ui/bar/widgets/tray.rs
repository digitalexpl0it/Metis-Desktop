//! System tray widget for the edge bar (StatusNotifierItem host UI).

use std::cell::RefCell;
use std::rc::Rc;

use gtk::gdk;
use gtk::prelude::*;

use crate::config::TrayIconMode;
use crate::services::{
    self, send_command, MenuType, TrayCommand, TrayItem, TrayMenu, TrayMenuItem, TraySnapshot,
};

const TRAY_ICON_SIZE: i32 = 18;
/// Overlay grid: grow with icon count up to this many columns × rows, then scroll.
const TRAY_PANEL_COLS: u32 = 5;
const TRAY_PANEL_ROWS_MAX: u32 = 3;
/// Button `size_request` (icon+8) plus CSS padding on `.metis-bar-tray-item`.
const TRAY_CELL: i32 = TRAY_ICON_SIZE + 12;
const TRAY_GAP: i32 = 8;
/// Compact empty hint — panel width still meets the "Background apps" title.
const TRAY_EMPTY_WIDTH: i32 = 120;

thread_local! {
    /// Nested context menu opened from a tray icon while the tray popover stays up.
    static TRAY_ITEM_MENU: RefCell<Option<gtk::Popover>> = const { RefCell::new(None) };
    /// Right-click target waiting for a fresh DBusMenu layout from the dbus thread.
    static PENDING_CONTEXT_MENU: RefCell<Option<(String, String, glib::WeakRef<gtk::Button>)>> =
        const { RefCell::new(None) };
    /// After a context-menu row click, ignore tray Activate briefly so popover
    /// teardown does not re-trigger screenshot on the icon underneath.
    static SUPPRESS_TRAY_ACTIVATE_UNTIL: RefCell<Option<std::time::Instant>> =
        const { RefCell::new(None) };
}

const TRAY_ACTIVATE_SUPPRESS_MS: u64 = 500;
const TRAY_MENU_CLOSE_DELAY_MS: u32 = 400;

struct TrayTooltipCtx {
    overlay: gtk::Overlay,
    tip: gtk::Label,
}

pub struct TrayWidget {
    root: gtk::Box,
    mode: Rc<RefCell<TrayIconMode>>,
    refresh: Rc<dyn Fn()>,
}

impl TrayWidget {
    pub fn new(mode: TrayIconMode) -> Self {
        let root = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        root.add_css_class("metis-bar-tray");

        let pinned_row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        pinned_row.add_css_class("metis-bar-tray-pinned");
        root.append(&pinned_row);

        let toggle = gtk::Button::builder().has_frame(false).build();
        toggle.add_css_class("metis-bar-widget");
        toggle.add_css_class("metis-bar-sys-icon");
        toggle.add_css_class("metis-bar-tray-toggle");
        toggle.set_tooltip_text(Some(&metis_i18n::tr("System tray")));
        let toggle_icon = gtk::Image::from_icon_name("pan-up-symbolic");
        toggle_icon.set_pixel_size(TRAY_ICON_SIZE);
        toggle.set_child(Some(&toggle_icon));
        root.append(&toggle);

        let panel = gtk::Box::new(gtk::Orientation::Vertical, 6);
        panel.add_css_class("metis-bar-dropdown-panel");
        panel.add_css_class("metis-bar-tray-panel");
        let panel_title = gtk::Label::new(Some(&metis_i18n::tr("Background apps")));
        panel_title.set_xalign(0.0);
        panel_title.add_css_class("metis-bar-section-title");
        panel.append(&panel_title);
        let panel_flow = gtk::FlowBox::new();
        panel_flow.set_selection_mode(gtk::SelectionMode::None);
        panel_flow.set_homogeneous(true);
        panel_flow.set_min_children_per_line(1);
        panel_flow.set_max_children_per_line(1);
        panel_flow.set_column_spacing(TRAY_GAP as u32);
        panel_flow.set_row_spacing(TRAY_GAP as u32);
        panel_flow.set_halign(gtk::Align::Start);
        panel_flow.set_valign(gtk::Align::Start);
        panel_flow.add_css_class("metis-bar-tray-flow");
        let panel_empty = gtk::Label::new(Some(&metis_i18n::tr("No background apps")));
        panel_empty.add_css_class("dim-label");
        panel_empty.set_xalign(0.0);
        panel_empty.set_wrap(false);
        panel_empty.set_halign(gtk::Align::Start);
        let panel_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .child(&panel_flow)
            .build();
        // Initial size for an empty / single-icon panel; rebuild adjusts to count.
        size_tray_panel_scroll(&panel_scroll, &panel_flow, 0);
        panel.append(&panel_scroll);
        panel.append(&panel_empty);

        let tip = gtk::Label::new(None);
        tip.add_css_class("metis-menu-tooltip-label");
        tip.set_visible(false);
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&panel));
        overlay.add_overlay(&tip);
        overlay.set_clip_overlay(&tip, false);
        tip.set_halign(gtk::Align::Start);
        tip.set_valign(gtk::Align::Start);
        tip.set_can_target(false);
        let tooltip_ctx = Rc::new(TrayTooltipCtx {
            overlay: overlay.clone(),
            tip: tip.clone(),
        });

        let mode = Rc::new(RefCell::new(mode));

        let rebuild: Rc<dyn Fn()> = {
            let pinned_row = pinned_row.clone();
            let panel_flow = panel_flow.clone();
            let panel_scroll = panel_scroll.clone();
            let panel_empty = panel_empty.clone();
            let mode = mode.clone();
            let tooltip_ctx = tooltip_ctx.clone();
            Rc::new(move || {
                rebuild_tray(
                    &pinned_row,
                    &panel_flow,
                    &panel_scroll,
                    &panel_empty,
                    *mode.borrow(),
                    &services::tray_snapshot(),
                    Some(tooltip_ctx.as_ref()),
                );
            })
        };

        services::register_tray_refresh(rebuild.clone());
        services::register_context_menu_ready(Rc::new(on_context_menu_ready));
        wire_tray_toggle(&toggle, &toggle_icon, &overlay, {
            let rebuild = rebuild.clone();
            move || {
                services::sync_tray();
                rebuild();
            }
        });

        // Paint from the current store immediately — a bar rebuild drops the old
        // widget without a fresh dbus event, so pinned mode would stay empty.
        rebuild();

        Self {
            root,
            mode,
            refresh: rebuild,
        }
    }

    pub fn root(&self) -> &gtk::Box {
        &self.root
    }

    pub fn set_mode(&self, mode: TrayIconMode) {
        *self.mode.borrow_mut() = mode;
        (self.refresh)();
    }

    #[allow(dead_code)] // bar tray refresh hook
    pub fn update(&self) {
        (self.refresh)();
    }
}

fn rebuild_tray(
    pinned_row: &gtk::Box,
    panel_flow: &gtk::FlowBox,
    panel_scroll: &gtk::ScrolledWindow,
    panel_empty: &gtk::Label,
    mode: TrayIconMode,
    snap: &TraySnapshot,
    panel_tooltip: Option<&TrayTooltipCtx>,
) {
    dismiss_tray_item_menu();
    while let Some(child) = pinned_row.first_child() {
        pinned_row.remove(&child);
    }
    while let Some(child) = panel_flow.first_child() {
        panel_flow.remove(&child);
    }

    pinned_row.set_visible(mode == TrayIconMode::Pinned && !snap.items.is_empty());

    let empty = snap.items.is_empty();
    panel_empty.set_visible(empty);
    panel_scroll.set_visible(!empty);

    for item in &snap.items {
        if mode == TrayIconMode::Pinned {
            pinned_row.append(&build_tray_button(item, TRAY_ICON_SIZE, None));
        }
        panel_flow.append(&build_tray_button(item, TRAY_ICON_SIZE, panel_tooltip));
    }

    size_tray_panel_scroll(panel_scroll, panel_flow, snap.items.len());
}

/// Size the tray popover to the icon grid: start compact, grow with each icon up
/// to 5×3, then keep that viewport and scroll anything beyond (Windows 11-style).
fn size_tray_panel_scroll(scroll: &gtk::ScrolledWindow, flow: &gtk::FlowBox, count: usize) {
    let cols = if count == 0 {
        1
    } else {
        count.min(TRAY_PANEL_COLS as usize).max(1) as u32
    };
    // Cap columns to the *current* icon count so FlowBox doesn't natural-size as
    // if five slots were already occupied (empty/sparse panels were huge before).
    flow.set_max_children_per_line(cols);
    flow.set_min_children_per_line(1);

    let (width, height) = tray_panel_content_size(count);
    let max_h = tray_panel_content_size(TRAY_PANEL_COLS as usize * TRAY_PANEL_ROWS_MAX as usize).1;
    // Lock width to the content needed *now* — grow on rebuild, never pre-size to 5 cols.
    scroll.set_min_content_width(width);
    scroll.set_max_content_width(width);
    scroll.set_min_content_height(height);
    scroll.set_max_content_height(max_h);
    scroll.set_propagate_natural_width(true);
    scroll.set_propagate_natural_height(true);
    // Hide the scrollbar until the 5×3 grid is full — avoids a tiny gutter when
    // the panel simply needs to grow a row.
    let needs_scroll = count > (TRAY_PANEL_COLS as usize * TRAY_PANEL_ROWS_MAX as usize);
    scroll.set_policy(
        gtk::PolicyType::Never,
        if needs_scroll {
            gtk::PolicyType::Automatic
        } else {
            gtk::PolicyType::Never
        },
    );
}

fn tray_panel_content_size(count: usize) -> (i32, i32) {
    if count == 0 {
        return (TRAY_EMPTY_WIDTH, TRAY_CELL);
    }
    let cols = count.min(TRAY_PANEL_COLS as usize);
    let rows = count
        .div_ceil(TRAY_PANEL_COLS as usize)
        .min(TRAY_PANEL_ROWS_MAX as usize);
    let width = (cols as i32) * TRAY_CELL + (cols.saturating_sub(1) as i32) * TRAY_GAP;
    let height = (rows as i32) * TRAY_CELL + (rows.saturating_sub(1) as i32) * TRAY_GAP;
    (width, height)
}

fn build_tray_button(
    item: &TrayItem,
    icon_size: i32,
    panel_tooltip: Option<&TrayTooltipCtx>,
) -> gtk::Button {
    let btn = gtk::Button::builder().has_frame(false).build();
    btn.add_css_class("metis-bar-widget");
    btn.add_css_class("metis-bar-tray-item");
    btn.set_size_request(icon_size + 8, icon_size + 8);

    let image = gtk::Image::new();
    image.set_can_target(false);
    image.set_pixel_size(icon_size);
    set_tray_icon_image(&image, item);
    btn.set_child(Some(&image));

    let tip_text = if item.title.is_empty() {
        item.id.as_str()
    } else {
        item.title.as_str()
    };
    match panel_tooltip {
        Some(ctx) => attach_tray_tooltip(&btn, tip_text, &ctx.overlay, &ctx.tip),
        None => btn.set_tooltip_text(Some(tip_text)),
    }

    wire_tray_button(&btn, item);
    btn
}

fn wire_tray_button(btn: &gtk::Button, item: &TrayItem) {
    let item = item.clone();
    let btn_weak = btn.downgrade();

    {
        let item = item.clone();
        btn.connect_clicked(move |b| {
            if tray_activate_suppressed() {
                tracing::debug!(bus = %item.bus_name, "tray: ignoring spurious activate");
                return;
            }
            let coords = tray_screen_coords(b, b.width() as f64 / 2.0, b.height() as f64 / 2.0);
            tracing::debug!(
                bus = %item.bus_name,
                x = coords.0,
                y = coords.1,
                "tray: left click activate"
            );
            let bus_name = item.bus_name.clone();
            let object_path = item.object_path.clone();
            glib::idle_add_local_once(move || {
                send_command(TrayCommand::Activate {
                    bus_name,
                    object_path,
                    x: coords.0,
                    y: coords.1,
                });
            });
            glib::timeout_add_local_once(
                std::time::Duration::from_millis(TRAY_MENU_CLOSE_DELAY_MS as u64),
                super::super::dropdown::close_all,
            );
        });
    }

    let gesture = gtk::GestureClick::builder()
        .button(gdk::BUTTON_SECONDARY)
        .propagation_phase(gtk::PropagationPhase::Capture)
        .build();
    {
        let item = item.clone();
        gesture.connect_pressed(move |_, _, x, y| {
            let Some(btn) = btn_weak.upgrade() else {
                return;
            };
            let coords = tray_screen_coords(&btn, x, y);
            handle_tray_secondary_click(&btn, &item, coords);
        });
    }
    btn.add_controller(gesture);
}

fn handle_tray_secondary_click(btn: &gtk::Button, item: &TrayItem, coords: (i32, i32)) {
    // Prefer a freshly synced snapshot in case the menu arrived after the panel opened.
    let live = services::tray_snapshot()
        .items
        .into_iter()
        .find(|i| i.bus_name == item.bus_name && i.object_path == item.object_path)
        .unwrap_or_else(|| item.clone());

    if let Some(menu) = &live.menu {
        if !menu.submenus.is_empty() {
            tracing::debug!(bus = %live.bus_name, entries = menu.submenus.len(), "tray: open context menu");
            show_context_menu(btn, &live, menu);
            return;
        }
    }

    if live.menu_path.is_some() {
        tracing::debug!(bus = %live.bus_name, "tray: fetching context menu");
        PENDING_CONTEXT_MENU.with(|cell| {
            *cell.borrow_mut() = Some((
                live.bus_name.clone(),
                live.object_path.clone(),
                btn.downgrade(),
            ));
        });
        send_command(TrayCommand::OpenContextMenu {
            bus_name: live.bus_name.clone(),
            object_path: live.object_path.clone(),
        });
        return;
    }

    tracing::debug!(bus = %live.bus_name, "tray: secondary activate / context_menu");
    send_command(TrayCommand::SecondaryActivate {
        bus_name: live.bus_name,
        object_path: live.object_path,
        x: coords.0,
        y: coords.1,
    });
}

fn on_context_menu_ready(item: &TrayItem) -> bool {
    let pending = PENDING_CONTEXT_MENU.with(|cell| cell.borrow().clone());
    let Some((bus_name, object_path, btn_weak)) = pending else {
        return false;
    };
    if item.bus_name != bus_name || item.object_path != object_path {
        return false;
    }
    PENDING_CONTEXT_MENU.with(|cell| *cell.borrow_mut() = None);
    let Some(menu) = item.menu.as_ref().filter(|m| !m.submenus.is_empty()) else {
        tracing::warn!(bus = %item.bus_name, "tray: context menu fetch returned no entries");
        return false;
    };
    let Some(btn) = btn_weak.upgrade() else {
        return false;
    };
    tracing::info!(bus = %item.bus_name, entries = menu.submenus.len(), "tray: showing fetched context menu");
    show_context_menu(&btn, item, menu);
    true
}
fn tray_activate_suppressed() -> bool {
    SUPPRESS_TRAY_ACTIVATE_UNTIL.with(|cell| {
        cell.borrow()
            .is_some_and(|until| until > std::time::Instant::now())
    })
}

fn suppress_tray_activate_briefly() {
    SUPPRESS_TRAY_ACTIVATE_UNTIL.with(|cell| {
        *cell.borrow_mut() = Some(
            std::time::Instant::now() + std::time::Duration::from_millis(TRAY_ACTIVATE_SUPPRESS_MS),
        );
    });
}

fn schedule_tray_popover_close() {
    glib::timeout_add_local_once(
        std::time::Duration::from_millis(TRAY_MENU_CLOSE_DELAY_MS as u64),
        || {
            dismiss_tray_item_menu();
            super::super::dropdown::close_all();
        },
    );
}

fn send_tray_menu_click(item: &TrayItem, submenu_id: i32, label: String) {
    suppress_tray_activate_briefly();
    let Some(menu_path) = item.menu_path.clone() else {
        return;
    };
    let cmd = TrayCommand::MenuClicked {
        bus_name: item.bus_name.clone(),
        menu_path,
        submenu_id,
        label,
    };
    glib::idle_add_local_once(move || send_command(cmd));
    schedule_tray_popover_close();
}

fn tray_screen_coords(widget: &impl IsA<gtk::Widget>, wx: f64, wy: f64) -> (i32, i32) {
    let widget = widget.as_ref();
    if let Some(native) = widget.native() {
        if let Some((x, y)) = widget.translate_coordinates(&native, wx, wy) {
            return (x.round() as i32, y.round() as i32);
        }
    }
    (wx.round() as i32, wy.round() as i32)
}

fn show_context_menu(anchor: &gtk::Button, item: &TrayItem, menu: &TrayMenu) {
    dismiss_tray_item_menu();

    let panel = super::super::dropdown::build_panel();
    panel.add_css_class("metis-bar-tray-menu");
    panel.set_spacing(2);
    panel.set_width_request(200);
    panel.set_margin_top(4);
    panel.set_margin_bottom(4);
    panel.set_margin_start(4);
    panel.set_margin_end(4);

    let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list.add_css_class("metis-bar-tray-menu-list");
    append_menu_items(&list, item, &menu.submenus);
    panel.append(&list);

    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(&panel)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(anchor);

    super::super::dropdown::register(&popover);
    TRAY_ITEM_MENU.with(|cell| *cell.borrow_mut() = Some(popover.clone()));

    let weak = popover.downgrade();
    popover.connect_closed(move |_| {
        let weak = weak.clone();
        glib::idle_add_local_once(move || {
            if let Some(p) = weak.upgrade() {
                if p.parent().is_some() {
                    p.unparent();
                }
            }
        });
    });

    glib::idle_add_local_once(move || popover.popup());
}

fn dismiss_tray_item_menu() {
    TRAY_ITEM_MENU.with(|cell| {
        if let Some(p) = cell.borrow_mut().take() {
            p.popdown();
            if p.parent().is_some() {
                p.unparent();
            }
        }
    });
}

/// In-popover tooltips: GTK's native tooltips render behind our layer-shell popover.
fn attach_tray_tooltip(
    widget: &impl IsA<gtk::Widget>,
    text: &str,
    overlay: &gtk::Overlay,
    tip: &gtk::Label,
) {
    widget
        .as_ref()
        .update_property(&[gtk::accessible::Property::Label(text)]);

    let timer: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
    let motion = gtk::EventControllerMotion::new();
    {
        let widget_weak = widget.clone().upcast::<gtk::Widget>().downgrade();
        let overlay_weak = overlay.downgrade();
        let tip = tip.clone();
        let text = text.to_string();
        let timer = timer.clone();
        motion.connect_enter(move |_, _, _| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            let widget_weak = widget_weak.clone();
            let overlay_weak = overlay_weak.clone();
            let tip = tip.clone();
            let text = text.clone();
            let timer_inner = timer.clone();
            let id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(450), move || {
                    *timer_inner.borrow_mut() = None;
                    let (Some(w), Some(ov)) = (widget_weak.upgrade(), overlay_weak.upgrade())
                    else {
                        return;
                    };
                    tip.set_label(&text);
                    if let Some((x, y)) = w.translate_coordinates(&ov, w.width() as f64 / 2.0, 0.0)
                    {
                        tip.set_margin_start((x as i32 - 28).max(0));
                        tip.set_margin_top((y as i32 - 30).max(0));
                    }
                    tip.set_visible(true);
                    if let Some(overlay) =
                        tip.parent().and_then(|p| p.downcast::<gtk::Overlay>().ok())
                    {
                        overlay.set_clip_overlay(&tip, false)
                    }
                });
            *timer.borrow_mut() = Some(id);
        });
    }
    {
        let tip = tip.clone();
        let timer = timer.clone();
        motion.connect_leave(move |_| {
            if let Some(id) = timer.borrow_mut().take() {
                id.remove();
            }
            tip.set_visible(false);
        });
    }
    widget.add_controller(motion);
}

fn append_menu_items(list: &gtk::Box, item: &TrayItem, entries: &[TrayMenuItem]) {
    for entry in entries {
        if !entry.visible {
            continue;
        }
        match entry.menu_type {
            MenuType::Separator => {
                let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
                sep.set_margin_top(4);
                sep.set_margin_bottom(4);
                list.append(&sep);
            }
            MenuType::Standard => {
                let is_submenu_container = !entry.submenu.is_empty()
                    && entry
                        .children_display
                        .as_deref()
                        .is_some_and(|d| d.eq_ignore_ascii_case("submenu"));
                if is_submenu_container {
                    append_menu_items(list, item, &entry.submenu);
                    continue;
                }

                let row = gtk::Button::builder()
                    .label(&entry.label)
                    .has_frame(false)
                    .build();
                row.add_css_class("metis-bar-tray-menu-item");
                row.set_sensitive(entry.enabled);
                row.set_halign(gtk::Align::Fill);
                let click_item = item.clone();
                let submenu_id = entry.id;
                let click_label = entry.label.clone();
                row.connect_clicked(move |_| {
                    send_tray_menu_click(&click_item, submenu_id, click_label.clone());
                });
                list.append(&row);

                if !entry.submenu.is_empty() {
                    append_menu_items(list, item, &entry.submenu);
                }
            }
        }
    }
}

fn set_tray_icon_image(image: &gtk::Image, item: &TrayItem) {
    image.add_css_class("metis-bar-tray-icon");

    if let Some(texture) = pixmap_texture(item) {
        image.add_css_class("metis-bar-tray-pixmap");
        image.set_from_paintable(Some(&texture));
        return;
    }
    if let Some(texture) = theme_path_texture(item) {
        image.add_css_class("metis-bar-tray-pixmap");
        image.set_from_paintable(Some(&texture));
        return;
    }
    if let Some(name) = item.icon_name.as_deref().filter(|n| !n.is_empty()) {
        set_themed_tray_icon(image, name);
        return;
    }
    image.set_from_icon_name(Some("application-x-executable-symbolic"));
}

/// Prefer a symbolic icon name so GTK tints it with the bar foreground colour.
fn set_themed_tray_icon(image: &gtk::Image, name: &str) {
    let candidates: Vec<String> = if name.ends_with("-symbolic") {
        vec![name.to_string()]
    } else {
        vec![format!("{name}-symbolic"), name.to_string()]
    };
    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        for candidate in &candidates {
            if theme.has_icon(candidate) {
                image.set_from_icon_name(Some(candidate));
                return;
            }
        }
    }
    image.set_from_icon_name(Some(name));
}

fn bar_is_light_mode() -> bool {
    crate::ui::theme::active_tokens()
        .mode
        .eq_ignore_ascii_case("light")
}

/// Chromium/Electron tray icons ship PNG files under `IconThemePath`.
fn theme_path_texture(item: &TrayItem) -> Option<gdk::Texture> {
    let theme = item.icon_theme_path.as_ref()?;
    let name = item.icon_name.as_ref()?;
    for path in [
        format!("{theme}/{name}.png"),
        format!("{theme}/{name}"),
        format!("{theme}/{name}@2x.png"),
    ] {
        if std::path::Path::new(&path).is_file() {
            match gdk::Texture::from_filename(&path) {
                Ok(texture) => return Some(texture),
                Err(err) => tracing::warn!(%err, path, "tray: failed to load theme-path icon"),
            }
        }
    }
    None
}

fn pixmap_texture(item: &TrayItem) -> Option<gdk::Texture> {
    item.icon_pixmap.as_ref().map(pixmap_to_texture)
}

/// Decode SNI IconPixmap bytes (BGRA / little-endian ARGB32) to a GTK texture.
fn pixmap_to_texture(pixmap: &crate::services::IconPixmap) -> gdk::Texture {
    let w = pixmap.width as usize;
    let h = pixmap.height as usize;
    if w == 0 || h == 0 {
        return gdk::MemoryTexture::new(
            1,
            1,
            gdk::MemoryFormat::R8g8b8a8,
            &glib::Bytes::from_static(&[0, 0, 0, 0]),
            4,
        )
        .into();
    }
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let src = (y * w + x) * 4;
            if src + 3 >= pixmap.pixels.len() {
                continue;
            }
            let b = pixmap.pixels[src];
            let g = pixmap.pixels[src + 1];
            let r = pixmap.pixels[src + 2];
            let a = pixmap.pixels[src + 3];
            let dst = (y * w + x) * 4;
            rgba[dst] = r;
            rgba[dst + 1] = g;
            rgba[dst + 2] = b;
            rgba[dst + 3] = a;
        }
    }
    if bar_is_light_mode() && pixmap_is_mostly_light(&rgba) {
        invert_opaque_rgba(&mut rgba);
    }
    gdk::MemoryTexture::new(
        pixmap.width,
        pixmap.height,
        gdk::MemoryFormat::R8g8b8a8,
        &glib::Bytes::from(&rgba),
        w * 4,
    )
    .into()
}

fn pixmap_is_mostly_light(rgba: &[u8]) -> bool {
    let mut light = 0u32;
    let mut opaque = 0u32;
    for px in rgba.as_chunks::<4>().0 {
        if px[3] < 48 {
            continue;
        }
        opaque += 1;
        let lum = (px[0] as u32 * 299 + px[1] as u32 * 587 + px[2] as u32 * 114) / 1000;
        if lum >= 200 {
            light += 1;
        }
    }
    opaque > 0 && light * 100 / opaque >= 55
}

fn invert_opaque_rgba(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        if px[3] < 16 {
            continue;
        }
        px[0] = 255 - px[0];
        px[1] = 255 - px[1];
        px[2] = 255 - px[2];
    }
}

fn wire_tray_toggle(
    button: &gtk::Button,
    icon: &gtk::Image,
    panel: &gtk::Overlay,
    prepare: impl Fn() + 'static,
) {
    let popover = gtk::Popover::builder()
        .autohide(false)
        .has_arrow(true)
        .position(super::super::popover_position())
        .child(panel)
        .build();
    popover.add_css_class("metis-bar-popover");
    popover.set_parent(button);

    {
        let btn = button.clone();
        let icon = icon.clone();
        popover.connect_map(move |_| {
            btn.add_css_class("metis-bar-dropdown-active");
            icon.set_from_icon_name(Some("pan-down-symbolic"));
        });
    }
    {
        let btn = button.clone();
        let icon = icon.clone();
        popover.connect_unmap(move |_| {
            dismiss_tray_item_menu();
            btn.remove_css_class("metis-bar-dropdown-active");
            icon.set_from_icon_name(Some("pan-up-symbolic"));
        });
    }

    super::super::dropdown::register(&popover);

    let popover_weak = popover.downgrade();
    button.connect_clicked(move |_| {
        let Some(popover) = popover_weak.upgrade() else {
            return;
        };
        if popover.is_visible() {
            glib::idle_add_local_once(move || popover.popdown());
            return;
        }
        super::super::dropdown::close_all();
        prepare();
        glib::idle_add_local_once(move || popover.popup());
    });
}
