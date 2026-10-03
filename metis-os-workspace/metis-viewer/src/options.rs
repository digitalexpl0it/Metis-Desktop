//! Remmina-style Advanced Desktop Settings notebook for Metis Viewer.

use gtk::prelude::*;
use metis_config::{
    ViewerCertPolicy, ViewerColorDepth, ViewerDisplayMode, ViewerHost, ViewerNetwork,
    ViewerPlacement, ViewerRdpOptions,
};
use metis_i18n::tr;

/// GTK controls for per-host RDP options.
pub struct OptionsUi {
    pub root: gtk::Widget,
    color_depth: gtk::DropDown,
    display_mode: gtk::DropDown,
    width_entry: gtk::Entry,
    height_entry: gtk::Entry,
    size_row: gtk::Box,
    placement: gtk::DropDown,
    multi_monitor: gtk::CheckButton,
    span_monitors: gtk::CheckButton,
    clipboard: gtk::CheckButton,
    audio: gtk::CheckButton,
    microphone: gtk::CheckButton,
    printers: gtk::CheckButton,
    smartcard: gtk::CheckButton,
    network: gtk::DropDown,
    wallpaper: gtk::CheckButton,
    font_smoothing: gtk::CheckButton,
    desktop_composition: gtk::CheckButton,
    window_drag: gtk::CheckButton,
    menu_animations: gtk::CheckButton,
    themes: gtk::CheckButton,
    bitmap_cache: gtk::CheckButton,
    auto_reconnect: gtk::CheckButton,
    cert: gtk::DropDown,
}

impl OptionsUi {
    pub fn build() -> Self {
        let notebook = gtk::Notebook::new();
        notebook.add_css_class("metis-viewer-options");
        notebook.set_hexpand(true);
        notebook.set_tab_pos(gtk::PositionType::Top);

        let color_depth = string_dropdown(ViewerColorDepth::all().iter().map(|v| v.label()));
        let display_mode = string_dropdown(ViewerDisplayMode::all().iter().map(|v| v.label()));
        let width_entry = gtk::Entry::new();
        width_entry.set_placeholder_text(Some("1920"));
        width_entry.set_input_purpose(gtk::InputPurpose::Digits);
        width_entry.set_max_width_chars(5);
        let height_entry = gtk::Entry::new();
        height_entry.set_placeholder_text(Some("1080"));
        height_entry.set_input_purpose(gtk::InputPurpose::Digits);
        height_entry.set_max_width_chars(5);
        let size_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        size_row.append(&width_entry);
        let x_lbl = gtk::Label::new(Some("×"));
        size_row.append(&x_lbl);
        size_row.append(&height_entry);
        let placement = string_dropdown(ViewerPlacement::all().iter().map(|v| v.label()));
        let multi_monitor = gtk::CheckButton::with_label(&tr("Use all monitors"));
        let span_monitors = gtk::CheckButton::with_label(&tr("Span desktop across monitors"));

        let display = options_page();
        display.append(&two_col_row(
            labeled_row(&tr("Color depth"), color_depth.clone().upcast()),
            labeled_row(&tr("Display mode"), display_mode.clone().upcast()),
        ));
        display.append(&two_col_row(
            labeled_row(&tr("Window size"), size_row.clone().upcast()),
            labeled_row(&tr("Metis placement"), placement.clone().upcast()),
        ));
        display.append(&two_col_row(
            check_row(&multi_monitor),
            check_row(&span_monitors),
        ));
        let display_hint = gtk::Label::new(Some(&tr(
            "Dedicated desktop moves FreeRDP onto its own workspace. Window mode keeps it on the current desk.",
        )));
        display_hint.set_wrap(true);
        display_hint.set_xalign(0.0);
        display_hint.add_css_class("metis-viewer-hint");
        display.append(&display_hint);
        notebook.append_page(&scroll(display), Some(&tab_label(&tr("Display"))));

        let clipboard = gtk::CheckButton::with_label(&tr("Clipboard"));
        let audio = gtk::CheckButton::with_label(&tr("Play sound on this computer"));
        let microphone = gtk::CheckButton::with_label(&tr("Microphone"));
        let printers = gtk::CheckButton::with_label(&tr("Printers"));
        let smartcard = gtk::CheckButton::with_label(&tr("Smart cards"));
        let local = options_page();
        local.append(&check_row(&clipboard));
        let clip_hint = gtk::Label::new(Some(&tr(
            "Clipboard is off by default — GRD can drop sessions when cliprdr negotiation fails.",
        )));
        clip_hint.set_wrap(true);
        clip_hint.set_xalign(0.0);
        clip_hint.add_css_class("metis-viewer-hint");
        local.append(&clip_hint);
        local.append(&two_col_row(check_row(&audio), check_row(&microphone)));
        local.append(&two_col_row(check_row(&printers), check_row(&smartcard)));
        notebook.append_page(&scroll(local), Some(&tab_label(&tr("Local resources"))));

        let network = string_dropdown(ViewerNetwork::all().iter().map(|v| v.label()));
        let wallpaper = gtk::CheckButton::with_label(&tr("Desktop background"));
        let font_smoothing = gtk::CheckButton::with_label(&tr("Font smoothing"));
        let desktop_composition = gtk::CheckButton::with_label(&tr("Desktop composition"));
        let window_drag = gtk::CheckButton::with_label(&tr("Show window contents while dragging"));
        let menu_animations = gtk::CheckButton::with_label(&tr("Menu and window animation"));
        let themes = gtk::CheckButton::with_label(&tr("Visual styles / themes"));
        let bitmap_cache = gtk::CheckButton::with_label(&tr("Persistent bitmap caching"));
        let auto_reconnect =
            gtk::CheckButton::with_label(&tr("Reconnect if the connection is dropped"));
        let experience = options_page();
        experience.append(&labeled_row(
            &tr("Connection speed"),
            network.clone().upcast(),
        ));
        experience.append(&two_col_row(
            check_row(&wallpaper),
            check_row(&font_smoothing),
        ));
        experience.append(&two_col_row(
            check_row(&desktop_composition),
            check_row(&window_drag),
        ));
        experience.append(&two_col_row(
            check_row(&menu_animations),
            check_row(&themes),
        ));
        experience.append(&two_col_row(
            check_row(&bitmap_cache),
            check_row(&auto_reconnect),
        ));
        notebook.append_page(&scroll(experience), Some(&tab_label(&tr("Experience"))));

        let cert = string_dropdown(ViewerCertPolicy::all().iter().map(|v| v.label()));
        let advanced = options_page();
        advanced.append(&labeled_row(
            &tr("Server authentication"),
            cert.clone().upcast(),
        ));
        let cert_hint = gtk::Label::new(Some(&tr(
            "Ignore is the Metis default for LAN self-signed GRD certificates. Trust on first use remembers the host key; Reject untrusted aborts on unknown certs.",
        )));
        cert_hint.set_wrap(true);
        cert_hint.set_xalign(0.0);
        cert_hint.add_css_class("metis-viewer-hint");
        advanced.append(&cert_hint);
        notebook.append_page(&scroll(advanced), Some(&tab_label(&tr("Advanced"))));

        {
            let size_row = size_row.clone();
            let display_mode = display_mode.clone();
            let sync_size = {
                let display_mode = display_mode.clone();
                let size_row = size_row.clone();
                move || {
                    let idx = display_mode.selected() as usize;
                    let mode = ViewerDisplayMode::all()
                        .get(idx)
                        .copied()
                        .unwrap_or_default();
                    size_row.set_sensitive(mode == ViewerDisplayMode::Windowed);
                }
            };
            sync_size();
            display_mode.connect_notify_local(Some("selected"), move |_, _| sync_size());
        }

        let ui = Self {
            root: notebook.upcast(),
            color_depth,
            display_mode,
            width_entry,
            height_entry,
            size_row,
            placement,
            multi_monitor,
            span_monitors,
            clipboard,
            audio,
            microphone,
            printers,
            smartcard,
            network,
            wallpaper,
            font_smoothing,
            desktop_composition,
            window_drag,
            menu_animations,
            themes,
            bitmap_cache,
            auto_reconnect,
            cert,
        };
        ui.apply(&ViewerRdpOptions::default());
        ui
    }

    pub fn apply(&self, opts: &ViewerRdpOptions) {
        set_dropdown_index(
            &self.color_depth,
            index_of(ViewerColorDepth::all(), opts.color_depth),
        );
        set_dropdown_index(
            &self.display_mode,
            index_of(ViewerDisplayMode::all(), opts.display_mode),
        );
        if opts.width > 0 {
            self.width_entry.set_text(&opts.width.to_string());
        } else {
            self.width_entry.set_text("");
        }
        if opts.height > 0 {
            self.height_entry.set_text(&opts.height.to_string());
        } else {
            self.height_entry.set_text("");
        }
        self.size_row
            .set_sensitive(opts.display_mode == ViewerDisplayMode::Windowed);
        set_dropdown_index(
            &self.placement,
            index_of(ViewerPlacement::all(), opts.placement),
        );
        self.multi_monitor.set_active(opts.multi_monitor);
        self.span_monitors.set_active(opts.span_monitors);
        self.clipboard.set_active(opts.clipboard);
        self.audio.set_active(opts.audio);
        self.microphone.set_active(opts.microphone);
        self.printers.set_active(opts.printers);
        self.smartcard.set_active(opts.smartcard);
        set_dropdown_index(&self.network, index_of(ViewerNetwork::all(), opts.network));
        self.wallpaper.set_active(opts.wallpaper);
        self.font_smoothing.set_active(opts.font_smoothing);
        self.desktop_composition
            .set_active(opts.desktop_composition);
        self.window_drag.set_active(opts.window_drag);
        self.menu_animations.set_active(opts.menu_animations);
        self.themes.set_active(opts.themes);
        self.bitmap_cache.set_active(opts.bitmap_cache);
        self.auto_reconnect.set_active(opts.auto_reconnect);
        set_dropdown_index(&self.cert, index_of(ViewerCertPolicy::all(), opts.cert));
    }

    pub fn apply_host(&self, host: &ViewerHost) {
        self.apply(&host.options);
    }

    pub fn collect(&self) -> ViewerRdpOptions {
        let width = self.width_entry.text().trim().parse().unwrap_or(0);
        let height = self.height_entry.text().trim().parse().unwrap_or(0);
        ViewerRdpOptions {
            color_depth: pick(ViewerColorDepth::all(), self.color_depth.selected()),
            display_mode: pick(ViewerDisplayMode::all(), self.display_mode.selected()),
            width,
            height,
            placement: pick(ViewerPlacement::all(), self.placement.selected()),
            multi_monitor: self.multi_monitor.is_active(),
            span_monitors: self.span_monitors.is_active(),
            clipboard: self.clipboard.is_active(),
            audio: self.audio.is_active(),
            microphone: self.microphone.is_active(),
            printers: self.printers.is_active(),
            smartcard: self.smartcard.is_active(),
            network: pick(ViewerNetwork::all(), self.network.selected()),
            wallpaper: self.wallpaper.is_active(),
            font_smoothing: self.font_smoothing.is_active(),
            desktop_composition: self.desktop_composition.is_active(),
            window_drag: self.window_drag.is_active(),
            menu_animations: self.menu_animations.is_active(),
            themes: self.themes.is_active(),
            bitmap_cache: self.bitmap_cache.is_active(),
            auto_reconnect: self.auto_reconnect.is_active(),
            cert: pick(ViewerCertPolicy::all(), self.cert.selected()),
        }
    }
}

fn pick<T: Copy>(all: &[T], selected: u32) -> T {
    all.get(selected as usize).copied().unwrap_or(all[0])
}

fn index_of<T: PartialEq>(all: &[T], value: T) -> u32 {
    all.iter().position(|v| *v == value).unwrap_or(0) as u32
}

fn set_dropdown_index(dropdown: &gtk::DropDown, index: u32) {
    dropdown.set_selected(index);
}

fn string_dropdown<'a>(labels: impl Iterator<Item = &'a str>) -> gtk::DropDown {
    let model = gtk::StringList::new(&labels.collect::<Vec<_>>());
    let dropdown = gtk::DropDown::new(Some(model), gtk::Expression::NONE);
    dropdown.set_hexpand(true);
    dropdown
}

fn options_page() -> gtk::Box {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    page.add_css_class("metis-viewer-options-page");
    page.set_margin_top(8);
    page.set_margin_bottom(8);
    page.set_margin_start(4);
    page.set_margin_end(4);
    page
}

/// Page body for a notebook tab. No nested ScrolledWindow — the host form
/// already scrolls Advanced Desktop Settings; nested `propagate_natural_height`
/// scrollers remeasure on every keystroke in sibling entries.
fn scroll(child: gtk::Box) -> gtk::Widget {
    child.upcast()
}

fn tab_label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.add_css_class("metis-viewer-options-tab");
    label
}

fn labeled_row(title: &str, widget: gtk::Widget) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
    row.add_css_class("metis-viewer-field");
    row.set_hexpand(true);
    let lbl = gtk::Label::new(Some(title));
    lbl.set_xalign(0.0);
    lbl.add_css_class("metis-viewer-field-label");
    row.append(&lbl);
    row.append(&widget);
    row
}

fn check_row(check: &gtk::CheckButton) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Vertical, 0);
    row.add_css_class("metis-viewer-field");
    row.set_hexpand(true);
    row.append(check);
    row
}

fn two_col_row(left: gtk::Box, right: gtk::Box) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row.add_css_class("metis-viewer-field-row");
    left.remove_css_class("metis-viewer-field");
    right.remove_css_class("metis-viewer-field");
    left.add_css_class("metis-viewer-field-col");
    right.add_css_class("metis-viewer-field-col");
    left.set_hexpand(true);
    right.set_hexpand(true);
    row.append(&left);
    row.append(&right);
    row
}
