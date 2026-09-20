//! Small shared widget helpers so the settings pages share a consistent layout.

use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;

/// Metadata for a settings content page (macOS-style header).
pub struct PageHeader<'a> {
    pub title: &'a str,
    pub icon: Option<&'a str>,
    pub subtitle: Option<&'a str>,
    pub hue: Option<crate::nav::NavHue>,
}

impl<'a> PageHeader<'a> {
    pub fn new(title: &'a str) -> Self {
        Self {
            title,
            icon: None,
            subtitle: None,
            hue: None,
        }
    }

    pub fn with_hue(mut self, hue: crate::nav::NavHue) -> Self {
        self.hue = Some(hue);
        self
    }

    pub fn with_icon(mut self, icon: &'a str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn with_subtitle(mut self, subtitle: &'a str) -> Self {
        self.subtitle = Some(subtitle);
        self
    }
}

/// Build a page using sidebar metadata from [`crate::nav`].
pub fn page_for(id: &'static str) -> (gtk::ScrolledWindow, gtk::Box) {
    let meta = crate::nav::meta_for(id).unwrap_or_else(|| panic!("unknown page id: {id}"));
    let title = metis_i18n::tr(meta.title);
    let subtitle = meta.subtitle.map(metis_i18n::tr);
    let mut header = PageHeader::new(title.as_str());
    if let Some(icon) = meta.icon {
        header = header.with_icon(icon);
    }
    if let Some(ref sub) = subtitle {
        header = header.with_subtitle(sub.as_str());
    }
    if let Some(hue) = meta.hue {
        header = header.with_hue(hue);
    }
    // Keep owned strings alive for the duration of page() by leaking into static…
    // Better: change PageHeader to take Cow/String. For now extend lifetime via
    // storing on the content after build — PageHeader only borrows during page().
    let (scroller, content) = page(header);
    // Pin translations on the widget so DropDown-free pages stay valid.
    let _ = (title, subtitle);
    (scroller, content)
}

/// Build a scrollable page with a heading. Returns the outer scroller (add to the
/// stack) and the inner content box (append rows/sections to it).
pub fn page(header: PageHeader<'_>) -> (gtk::ScrolledWindow, gtk::Box) {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 20);
    content.set_margin_top(20);
    content.set_margin_bottom(28);
    content.set_margin_start(32);
    content.set_margin_end(32);
    content.add_css_class("metis-settings-page");

    let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    header_box.add_css_class("metis-settings-page-header");
    header_box.set_margin_bottom(4);

    if let Some(icon) = header.icon {
        // Equal CSS padding centers the glyph; avoid fixed width/height requests.
        let wrap = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        wrap.set_halign(gtk::Align::Start);
        wrap.set_valign(gtk::Align::Center);
        wrap.add_css_class("metis-settings-page-icon-wrap");
        if let Some(hue) = header.hue {
            wrap.add_css_class(hue.css_class());
        } else {
            wrap.add_css_class(crate::nav::NavHue::Gray.css_class());
        }
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(28);
        img.set_halign(gtk::Align::Center);
        img.set_valign(gtk::Align::Center);
        img.add_css_class("metis-settings-page-icon");
        wrap.append(&img);
        header_box.append(&wrap);
    }

    let titles = gtk::Box::new(gtk::Orientation::Vertical, 2);
    titles.set_valign(gtk::Align::Center);
    titles.set_hexpand(true);

    let heading = gtk::Label::new(Some(header.title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-settings-title");
    titles.append(&heading);

    if let Some(sub) = header.subtitle {
        let sublabel = gtk::Label::new(Some(sub));
        sublabel.set_xalign(0.0);
        sublabel.set_wrap(true);
        sublabel.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        // Wrapped labels otherwise report the full line as min-width and lock
        // the Settings window from shrinking after a language Apply rebuild.
        sublabel.set_width_chars(28);
        sublabel.set_max_width_chars(56);
        sublabel.add_css_class("metis-settings-subtitle");
        titles.append(&sublabel);
    }

    header_box.append(&titles);
    content.append(&header_box);

    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .hexpand(true)
        .vexpand(true)
        .overlay_scrolling(false)
        .propagate_natural_width(false)
        .propagate_natural_height(false)
        .child(&content)
        .build();
    scroller.set_kinetic_scrolling(false);
    wire_vertical_scroll(&scroller);
    scroller.add_css_class("metis-settings-scroller");
    wire_click_to_defocus(&content);
    // After the page is filled and mapped, forward wheel on scales/spins/dropdowns
    // so scrolling never gets stuck changing a control under the pointer.
    scroller.connect_map(|sw| {
        if let Some(child) = sw.child() {
            install_range_wheel_forwards(&child);
        }
    });
    (scroller, content)
}

/// Opaque rounded card for modal Settings sheets. Pair with a transparent
/// `metis-settings-password-dialog` / `metis-settings-widget-dialog` window so
/// pixels outside the radius stay true alpha instead of a solid grey fill.
pub fn dialog_sheet(content: &impl IsA<gtk::Widget>) -> gtk::Box {
    let sheet = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sheet.add_css_class("metis-settings-dialog-sheet");
    sheet.set_hexpand(true);
    sheet.set_vexpand(true);
    sheet.append(content);
    sheet
}

/// Drop keyboard focus (committing any editable entry) when the user clicks an
/// empty part of the page. Clicks that land on a focusable control — entries,
/// spin buttons, dropdowns, switches, buttons — are left alone so those widgets
/// keep working normally.
fn wire_click_to_defocus(content: &gtk::Box) {
    let click = gtk::GestureClick::new();
    // Respond to any mouse button, and run after child widgets so an interactive
    // control that claims the press keeps its focus.
    click.set_button(0);
    let root_ref: gtk::Widget = content.clone().upcast();
    click.connect_pressed(move |_gesture, _n_press, x, y| {
        let mut node = root_ref.pick(x, y, gtk::PickFlags::DEFAULT);
        let mut hit_focusable = false;
        while let Some(widget) = node {
            if widget.is_focusable() {
                hit_focusable = true;
                break;
            }
            if widget == root_ref {
                break;
            }
            node = widget.parent();
        }
        if !hit_focusable {
            if let Some(root) = root_ref.root() {
                root.set_focus(None::<&gtk::Widget>);
            }
        }
    });
    content.add_controller(click);
}

/// `ScrolledWindow:kinetic-scrolling` only disables *touchscreen* kinetic
/// scrolling. GTK 4.22 still sets `KINETIC` on its built-in touchpad
/// `EventControllerScroll`s, which schedules overshoot→deceleration and feels
/// like the page "locks up, then catches up".
fn strip_touchpad_kinetic(scroller: &gtk::ScrolledWindow) {
    let model = scroller.observe_controllers();
    for i in 0..model.n_items() {
        let Some(obj) = model.item(i) else {
            continue;
        };
        let Ok(scroll) = obj.downcast::<gtk::EventControllerScroll>() else {
            continue;
        };
        let flags = scroll.flags();
        if flags.contains(gtk::EventControllerScrollFlags::KINETIC) {
            scroll.set_flags(flags - gtk::EventControllerScrollFlags::KINETIC);
        }
    }
}

/// Drive vertical scrolling from wheel/touchpad events.
///
/// Capture-phase on the `ScrolledWindow` only (not its child). Attaching to the
/// content box steals events from nested scrollers (Titlebars ListView, dialog
/// lists, …) and makes those pages feel stuck. Always `Stop` once *this*
/// scroller can scroll — including at clamp edges — so GTK 4.22 cannot schedule
/// its overshoot→deceleration hitch.
///
/// Adjustment updates are applied immediately (not deferred to idle): coalescing
/// behind `idle_add` made scroll look frozen whenever the main loop was busy
/// (Display IPC poll, GL frame stalls), then “unlock” seconds later.
pub fn wire_vertical_scroll(scroller: &gtk::ScrolledWindow) {
    scroller.set_kinetic_scrolling(false);
    strip_touchpad_kinetic(scroller);

    let ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    let vadj = scroller.vadjustment();
    ctrl.connect_scroll(move |_, _, dy| {
        let page = vadj.page_size();
        let upper = vadj.upper();
        let lower = vadj.lower();
        // Non-scrolling wrapper (e.g. Titlebars outer page): let nested
        // scrollers / children handle the event — including zero-delta ends.
        if upper - lower <= page {
            return glib::Propagation::Proceed;
        }
        if dy.abs() < f64::EPSILON {
            return glib::Propagation::Stop;
        }
        // Discrete wheel notches report ±1; smooth trackpads report pixel deltas.
        let delta = if dy.abs() <= 3.0 {
            dy * vadj.step_increment().max(48.0)
        } else {
            dy
        };
        let max = (upper - page).max(lower);
        let new_val = (vadj.value() + delta).clamp(lower, max);
        if (new_val - vadj.value()).abs() > f64::EPSILON {
            vadj.set_value(new_val);
        }
        glib::Propagation::Stop
    });
    scroller.add_controller(ctrl);

    // GTK may recreate scroll controllers around map; strip kinetic again.
    scroller.connect_map(|sw| {
        sw.set_kinetic_scrolling(false);
        strip_touchpad_kinetic(sw);
    });

    scroller.connect_edge_overshot(move |sw, _pos| {
        let vadj = sw.vadjustment();
        let max = (vadj.upper() - vadj.page_size()).max(vadj.lower());
        let clamped = vadj.value().clamp(vadj.lower(), max);
        if (clamped - vadj.value()).abs() > f64::EPSILON {
            vadj.set_value(clamped);
        }
        sw.set_kinetic_scrolling(false);
        strip_touchpad_kinetic(sw);
    });
}

/// Keep wheel events on a GtkScale/GtkRange/DropDown from adjusting the control;
/// scroll the nearest *scrollable* enclosing `ScrolledWindow` instead.
pub fn forward_wheel_to_page_scroller(widget: &impl IsA<gtk::Widget>) {
    // Avoid stacking duplicate controllers if called more than once (page map).
    if widget
        .css_classes()
        .iter()
        .any(|c| c.as_str() == "metis-settings-wheel-forwarded")
    {
        return;
    }
    widget.add_css_class("metis-settings-wheel-forwarded");

    let ctrl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    ctrl.set_propagation_phase(gtk::PropagationPhase::Capture);
    ctrl.connect_scroll(move |controller, _, dy| {
        let mut parent = controller.widget().parent();
        while let Some(p) = parent {
            if let Ok(scroller) = p.clone().downcast::<gtk::ScrolledWindow>() {
                let vadj = scroller.vadjustment();
                let page = vadj.page_size();
                let lower = vadj.lower();
                let upper = vadj.upper();
                // Skip non-scrolling wrappers and Never/Never viewports that
                // still eat scroll events via GTK's built-in controller.
                let never_never = scroller.hscrollbar_policy() == gtk::PolicyType::Never
                    && scroller.vscrollbar_policy() == gtk::PolicyType::Never;
                if never_never || upper - lower <= page {
                    parent = p.parent();
                    continue;
                }
                if dy.abs() < f64::EPSILON {
                    return glib::Propagation::Stop;
                }
                let delta = if dy.abs() <= 3.0 {
                    dy * vadj.step_increment().max(48.0)
                } else {
                    dy
                };
                let max = (upper - page).max(lower);
                let new_val = (vadj.value() + delta).clamp(lower, max);
                if (new_val - vadj.value()).abs() > f64::EPSILON {
                    vadj.set_value(new_val);
                }
                return glib::Propagation::Stop;
            }
            parent = p.parent();
        }
        glib::Propagation::Proceed
    });
    widget.add_controller(ctrl);
}

/// Walk a page subtree and forward wheel on scales / spins / closed dropdowns
/// / colour buttons so scrolling the page never gets stuck on a control.
pub fn install_range_wheel_forwards(root: &impl IsA<gtk::Widget>) {
    fn walk(widget: &gtk::Widget) {
        if widget.is::<gtk::Scale>()
            || widget.is::<gtk::SpinButton>()
            || widget.is::<gtk::DropDown>()
            || widget.is::<gtk::ColorDialogButton>()
            || widget.has_css_class("metis-color-swatch")
            || widget.has_css_class("metis-font-picker")
        {
            forward_wheel_to_page_scroller(widget);
        }
        let mut child = widget.first_child();
        while let Some(c) = child {
            walk(&c);
            child = c.next_sibling();
        }
    }
    walk(root.upcast_ref());
}

/// A titled card grouping related controls. Returns the body box to fill.
pub fn section(title: &str) -> (gtk::Box, gtk::Box) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("metis-settings-section");

    let header = gtk::Label::new(Some(title));
    header.set_xalign(0.0);
    header.add_css_class("metis-settings-section-title");
    card.append(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("metis-settings-section-body");
    card.append(&body);
    (card, body)
}

/// Soft helper copy under a section. Constrains wrap width so live language
/// rebuilds cannot lock the Settings window to a huge minimum width.
pub fn hint(text: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_wrap(true);
    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
    l.set_hexpand(true);
    l.set_width_chars(28);
    l.set_max_width_chars(72);
    l.add_css_class("metis-settings-hint");
    l
}

/// Like [`section`] but with a leading symbolic icon in the header.
pub fn section_with_icon(title: &str, icon: &str) -> (gtk::Box, gtk::Box) {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("metis-settings-section");

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    header.add_css_class("metis-settings-section-header");
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(14);
    img.add_css_class("metis-settings-section-icon");
    header.append(&img);
    let label = gtk::Label::new(Some(title));
    label.set_xalign(0.0);
    label.add_css_class("metis-settings-section-title");
    header.append(&label);
    card.append(&header);

    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("metis-settings-section-body");
    card.append(&body);
    (card, body)
}

/// A leading-icon + label + trailing control row.
pub fn row_with_icon(icon: &str, label: &str, control: &impl AsRef<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(16);
    img.add_css_class("metis-settings-row-icon");
    row.append(&img);
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    row.append(&lbl);
    row.append(control.as_ref());
    row
}

/// A label + trailing control row.
pub fn row(label: &str, control: &impl AsRef<gtk::Widget>) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    row.append(&lbl);
    row.append(control.as_ref());
    row
}

/// Settings row with a trailing switch. Clicking the label toggles the switch
/// (GNOME-style) so users are not forced to hit the small thumb target.
pub fn switch_row(label: &str) -> (gtk::Box, gtk::Switch) {
    let sw = gtk::Switch::new();
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.add_css_class("metis-settings-row");
    let lbl = gtk::Label::new(Some(label));
    lbl.set_xalign(0.0);
    lbl.set_hexpand(true);
    lbl.add_css_class("metis-settings-switch-label");
    let sw_toggle = sw.clone();
    let gesture = gtk::GestureClick::new();
    gesture.connect_released(move |_, _, _, _| {
        sw_toggle.set_active(!sw_toggle.is_active());
    });
    lbl.add_controller(gesture);
    row.append(&lbl);
    row.append(&sw);
    (row, sw)
}

/// Prevent held Backspace on an empty entry from bubbling to the Settings sidebar
/// search filter (same class of lag/lockup as the main search field).
pub fn swallow_empty_backspace(entry: &gtk::Entry) {
    let entry_key = entry.clone();
    let key = gtk::EventControllerKey::new();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::BackSpace && entry_key.text().is_empty() {
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    });
    entry.add_controller(key);
}

/// Run `action` on the next main-loop idle turn so GTK can paint the switch
/// state before any file I/O or IPC in the handler.
///
/// **Important:** If you suppress programmatic `set_active` with a flag (e.g.
/// `toggling`), check that flag with [`defer_switch_active_notify_when`] — a
/// check *inside* `action` runs too late (after the idle turn), so a status
/// poll can re-fire enable/disable after the user toggled.
pub fn defer_switch_active_notify<F>(sw: &gtk::Switch, action: F) -> glib::SignalHandlerId
where
    F: Fn(bool) + 'static,
{
    defer_switch_active_notify_when(sw, || true, action)
}

/// Like [`defer_switch_active_notify`], but only schedules `action` when
/// `allow()` is true **at notify time** (before the idle defer).
pub fn defer_switch_active_notify_when<P, F>(
    sw: &gtk::Switch,
    allow: P,
    action: F,
) -> glib::SignalHandlerId
where
    P: Fn() -> bool + 'static,
    F: Fn(bool) + 'static,
{
    let action = std::rc::Rc::new(action);
    let allow = std::rc::Rc::new(allow);
    sw.connect_active_notify(move |switch| {
        if !allow() {
            return;
        }
        let active = switch.is_active();
        let action = action.clone();
        glib::idle_add_local_once(move || action(active));
    })
}

/// Labelled dropdown of installed candidates (plus Auto-detect / Custom), with a
/// revealed path entry + file chooser when Custom is selected. `on_change` receives
/// the chosen value (`None` = auto-detect) whenever the selection or path changes.
pub fn launcher_picker(
    icon: &str,
    label: &str,
    candidates: &[(&str, &str)],
    current: Option<String>,
    on_change: impl Fn(Option<String>) + 'static,
) -> gtk::Box {
    let installed: Vec<(String, String)> = candidates
        .iter()
        .filter(|(bin, _)| metis_config::binary_in_path(bin))
        .map(|(bin, lbl)| (bin.to_string(), lbl.to_string()))
        .collect();

    let mut labels: Vec<String> = Vec::with_capacity(installed.len() + 2);
    labels.push(metis_i18n::tr("Auto-detect"));
    for (_, lbl) in &installed {
        labels.push(lbl.clone());
    }
    labels.push(metis_i18n::tr("Custom…"));
    let custom_index = (labels.len() - 1) as u32;
    let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
    let dd = gtk::DropDown::from_strings(&label_refs);

    let path_placeholder = metis_i18n::tr("Path to executable, e.g. /usr/bin/btop");
    let entry = gtk::Entry::builder()
        .placeholder_text(&path_placeholder)
        .hexpand(true)
        .build();
    let browse = gtk::Button::from_icon_name("document-open-symbolic");
    browse.set_tooltip_text(Some(&metis_i18n::tr("Browse…")));
    let custom_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    custom_box.append(&entry);
    custom_box.append(&browse);
    custom_box.set_visible(false);

    if let Some(cur) = current.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(pos) = installed.iter().position(|(bin, _)| bin == cur) {
            dd.set_selected(1 + pos as u32);
        } else {
            dd.set_selected(custom_index);
            entry.set_text(cur);
            custom_box.set_visible(true);
        }
    }

    let on_change = Rc::new(on_change);
    let installed_bins: Vec<String> = installed.iter().map(|(bin, _)| bin.clone()).collect();

    {
        let entry = entry.clone();
        let custom_box = custom_box.clone();
        let on_change = on_change.clone();
        let installed_bins = installed_bins.clone();
        dd.connect_selected_notify(move |dd| {
            let sel = dd.selected();
            if sel == 0 {
                custom_box.set_visible(false);
                on_change(None);
            } else if sel == custom_index {
                custom_box.set_visible(true);
                on_change(non_empty_path(&entry.text()));
            } else {
                custom_box.set_visible(false);
                on_change(installed_bins.get((sel - 1) as usize).cloned());
            }
        });
    }

    {
        let dd = dd.clone();
        let on_change = on_change.clone();
        entry.connect_changed(move |e| {
            if dd.selected() == custom_index {
                on_change(non_empty_path(&e.text()));
            }
        });
    }

    {
        let entry = entry.clone();
        browse.connect_clicked(move |btn| {
            let dialog = gtk::FileDialog::new();
            dialog.set_title(&metis_i18n::tr("Choose an executable"));
            let parent = btn.root().and_downcast::<gtk::Window>();
            let entry = entry.clone();
            dialog.open(parent.as_ref(), gio::Cancellable::NONE, move |res| {
                if let Ok(file) = res {
                    if let Some(path) = file.path() {
                        entry.set_text(&path.to_string_lossy());
                    }
                }
            });
        });
    }

    let container = gtk::Box::new(gtk::Orientation::Vertical, 8);
    container.append(&row_with_icon(icon, label, &dd));
    container.append(&custom_box);
    container
}

fn non_empty_path(text: &str) -> Option<String> {
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
