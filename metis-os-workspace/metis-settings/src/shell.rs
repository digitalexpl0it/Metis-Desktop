//! Settings v2 chrome — Home + mini sidebar + category view (GtkStack, no Overlay).
//!
//! Overlays were eating all pointer events on GTK 4.22 even when “hidden”, so
//! category navigation uses a Stack slide instead of a dimmed overlay sheet.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use metis_i18n::tr;

use crate::dialog;
use crate::gtk_cb::OptFnStrRef;
use crate::home;
use crate::motion;
use crate::nav::{self, NavHue, CATEGORIES};
use crate::pages;
use crate::runtime;
use crate::theme;
use crate::ui;
use crate::{i18n_gtk, PageLaunch, APP_ICON_BYTES};

const MINI_SIDEBAR_WIDTH: i32 = 64;

thread_local! {
    static SETTINGS_WINDOW: RefCell<Option<gtk::ApplicationWindow>> = const { RefCell::new(None) };
    static UI_READY: Cell<bool> = const { Cell::new(false) };
    static OPEN_PAGE: OptFnStrRef = const { RefCell::new(None) };
    static SHOW_HOME: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
}

/// Open Settings once, or reuse the existing window (navigate + unminimize).
pub fn open(app: &gtk::Application, launch: PageLaunch, force_rebuild: bool) {
    let window_alive = SETTINGS_WINDOW.with(|slot| slot.borrow().is_some());
    if force_rebuild || !UI_READY.get() || !window_alive {
        build(app, launch);
        UI_READY.set(true);
    } else {
        apply_launch(&launch);
        present();
    }
}

fn apply_launch(launch: &PageLaunch) {
    if let Some(page) = launch.page.as_deref() {
        nav::request_page(page);
        if page == "network" {
            if let Some(tab) = launch.tab.as_deref() {
                pages::network::request_tab(tab);
            }
        }
    } else {
        SHOW_HOME.with(|slot| {
            if let Some(show) = slot.borrow().as_ref() {
                show();
            }
        });
    }
}

pub fn present() {
    SETTINGS_WINDOW.with(|slot| {
        if let Some(window) = slot.borrow().as_ref() {
            window.unminimize();
            window.present();
        }
    });
    runtime::activate_settings_window();
}

/// Tear down Settings completely. Unique-instance + perpetual page timers
/// (network/bt/power polls) keep `GtkApplication::quit` from exiting the
/// process, so we hard-exit after clearing state.
fn shutdown_application(app: &gtk::Application, _closing: Option<&gtk::ApplicationWindow>) {
    thread_local! {
        static DONE: Cell<bool> = const { Cell::new(false) };
    }
    if DONE.with(|d| d.replace(true)) {
        return;
    }

    SETTINGS_WINDOW.with(|slot| {
        *slot.borrow_mut() = None;
    });
    UI_READY.set(false);
    OPEN_PAGE.with(|s| *s.borrow_mut() = None);
    SHOW_HOME.with(|s| *s.borrow_mut() = None);
    dialog::dismiss_silent();

    // Drop orphan password / VPN / widget dialogs so they do not linger briefly.
    for win in app.windows() {
        win.set_visible(false);
    }
    app.quit();
    // Immediate exit — do not wait for the main loop; timers would keep it alive.
    std::process::exit(0);
}

fn build(app: &gtk::Application, launch: PageLaunch) {
    i18n_gtk::apply_gtk_direction();
    theme::install();

    let under_metis = std::env::var_os("METIS_SESSION").is_some();
    let window = SETTINGS_WINDOW.with(|slot| {
        if let Some(existing) = slot.borrow().as_ref() {
            existing.set_title(Some(&tr("Settings")));
            return existing.clone();
        }
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title(tr("Settings"))
            .default_width(1100)
            .default_height(720)
            .decorated(!under_metis)
            .build();
        window.add_css_class("metis-settings-window");
        window.connect_map(apply_window_icon);
        window.connect_close_request({
            let app = app.clone();
            move |win| {
                shutdown_application(&app, Some(win));
                glib::Propagation::Stop
            }
        });
        window.connect_destroy({
            let app = app.clone();
            move |_| {
                // Compositor / SSD close sometimes destroys without close-request.
                if UI_READY.get() || SETTINGS_WINDOW.with(|s| s.borrow().is_some()) {
                    shutdown_application(&app, None);
                }
            }
        });
        apply_window_icon(&window);
        *slot.borrow_mut() = Some(window.clone());
        window
    });

    let page_stack = build_page_stack(&window, &launch);

    let sheet_header_title = gtk::Label::new(None);
    sheet_header_title.set_xalign(0.0);
    sheet_header_title.set_hexpand(true);
    sheet_header_title.add_css_class("metis-settings-sheet-title");

    let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
    close_btn.add_css_class("metis-settings-sheet-close");
    close_btn.set_tooltip_text(Some(&tr("Back to Home")));

    let sheet_header = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    sheet_header.add_css_class("metis-settings-sheet-header");
    sheet_header.set_margin_top(14);
    sheet_header.set_margin_bottom(8);
    sheet_header.set_margin_start(18);
    sheet_header.set_margin_end(14);
    sheet_header.append(&sheet_header_title);
    sheet_header.append(&close_btn);

    let page_nav = gtk::ListBox::new();
    page_nav.add_css_class("metis-settings-sheet-pages");
    page_nav.set_selection_mode(gtk::SelectionMode::Single);
    let page_nav_scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .hexpand(false)
        .width_request(168)
        .overlay_scrolling(false)
        .child(&page_nav)
        .build();
    page_nav_scroll.add_css_class("metis-settings-sheet-pages-scroll");
    page_nav_scroll.set_kinetic_scrolling(false);
    ui::wire_vertical_scroll(&page_nav_scroll);

    let sheet_body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    sheet_body.set_hexpand(true);
    sheet_body.set_vexpand(true);
    sheet_body.append(&page_nav_scroll);
    sheet_body.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    sheet_body.append(&page_stack);

    let sheet_card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    sheet_card.add_css_class("metis-settings-category-sheet");
    sheet_card.set_hexpand(true);
    sheet_card.set_vexpand(true);
    sheet_card.append(&sheet_header);
    sheet_card.append(&sheet_body);

    let active_category: Rc<RefCell<Option<&'static str>>> = Rc::new(RefCell::new(None));
    let selecting_pages = Rc::new(Cell::new(false));

    let home_btn = mini_button("go-home-symbolic", &tr("Home"), None);
    home_btn.add_css_class("metis-settings-mini-home");

    let mini_cat_btns: Vec<(&'static str, gtk::Button)> = CATEGORIES
        .iter()
        .map(|cat| (cat.id, mini_button(cat.icon, &tr(cat.title), Some(cat.hue))))
        .collect();

    let sync_mini = {
        let home_btn = home_btn.clone();
        let mini_cat_btns = mini_cat_btns.clone();
        let active_category = active_category.clone();
        Rc::new(move || {
            let active = *active_category.borrow();
            if active.is_none() {
                home_btn.add_css_class("metis-settings-mini-active");
            } else {
                home_btn.remove_css_class("metis-settings-mini-active");
            }
            for (id, btn) in &mini_cat_btns {
                if Some(*id) == active {
                    btn.add_css_class("metis-settings-mini-active");
                } else {
                    btn.remove_css_class("metis-settings-mini-active");
                }
            }
        })
    };

    // Main content: Home ↔ category (slide), no Overlay in the pick path.
    let content_stack = gtk::Stack::new();
    content_stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    content_stack.set_transition_duration(motion::ms(220));
    content_stack.set_hhomogeneous(true);
    content_stack.set_vhomogeneous(true);
    content_stack.set_hexpand(true);
    content_stack.set_vexpand(true);

    let show_home: Rc<dyn Fn()> = {
        let content_stack = content_stack.clone();
        let active_category = active_category.clone();
        let sync_mini = sync_mini.clone();
        Rc::new(move || {
            *active_category.borrow_mut() = None;
            content_stack.set_visible_child_name("home");
            sync_mini();
        })
    };

    let open_page: Rc<dyn Fn(&str)> = {
        let page_stack = page_stack.clone();
        let page_nav = page_nav.clone();
        let page_nav_scroll = page_nav_scroll.clone();
        let sheet_header_title = sheet_header_title.clone();
        let content_stack = content_stack.clone();
        let active_category = active_category.clone();
        let selecting_pages = selecting_pages.clone();
        let sync_mini = sync_mini.clone();
        Rc::new(move |page_id: &str| {
            let Some(cat) = nav::category_for_page(page_id) else {
                return;
            };
            let pages = nav::pages_in_category(cat.id);
            if pages.is_empty() {
                return;
            }

            let switching_cat = *active_category.borrow() != Some(cat.id);
            *active_category.borrow_mut() = Some(cat.id);
            sheet_header_title.set_label(&tr(cat.title));

            if switching_cat {
                while let Some(child) = page_nav.first_child() {
                    page_nav.remove(&child);
                }
                for item in &pages {
                    let Some(id) = item.page_id else { continue };
                    let row = sheet_page_row(item);
                    row.set_widget_name(id);
                    page_nav.append(&row);
                }
                page_nav_scroll.set_visible(pages.len() > 1);
            }

            selecting_pages.set(true);
            if page_stack.visible_child_name().as_deref() != Some(page_id) {
                page_stack.set_visible_child_name(page_id);
            }
            let mut child = page_nav.first_child();
            while let Some(widget) = child {
                let next = widget.next_sibling();
                if let Ok(row) = widget.downcast::<gtk::ListBoxRow>() {
                    if row.widget_name() == page_id {
                        page_nav.select_row(Some(&row));
                        break;
                    }
                }
                child = next;
            }
            selecting_pages.set(false);

            content_stack.set_visible_child_name("category");
            sync_mini();
        })
    };

    let open_category: Rc<dyn Fn(&str)> = {
        let open_page = open_page.clone();
        Rc::new(move |cat_id: &str| {
            if let Some(first) = nav::pages_in_category(cat_id)
                .first()
                .and_then(|p| p.page_id)
            {
                open_page(first);
            }
        })
    };

    {
        let open_page = open_page.clone();
        let selecting_pages = selecting_pages.clone();
        page_nav.connect_row_selected(move |_list, row| {
            if selecting_pages.get() {
                return;
            }
            let Some(row) = row else { return };
            let id = row.widget_name();
            if !id.is_empty() {
                open_page(&id);
            }
        });
    }

    {
        let show_home = show_home.clone();
        close_btn.connect_clicked(move |_| show_home());
    }
    {
        let show_home = show_home.clone();
        home_btn.connect_clicked(move |_| show_home());
    }
    for (id, btn) in &mini_cat_btns {
        let open_category = open_category.clone();
        let cat_id = *id;
        btn.connect_clicked(move |_| open_category(cat_id));
    }

    {
        let show_home = show_home.clone();
        let content_stack = content_stack.clone();
        let key = gtk::EventControllerKey::new();
        key.set_propagation_phase(gtk::PropagationPhase::Capture);
        key.connect_key_pressed(move |_, keyval, _, _| {
            if keyval != gtk::gdk::Key::Escape {
                return glib::Propagation::Proceed;
            }
            // Top-sheet (color/font/confirm) first — then category → home.
            if dialog::is_open() {
                dialog::dismiss(true);
                return glib::Propagation::Stop;
            }
            if content_stack.visible_child_name().as_deref() == Some("category") {
                show_home();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(key);
    }

    nav::set_page_request_handler(open_page.clone());
    OPEN_PAGE.with(|s| *s.borrow_mut() = Some(open_page.clone()));
    SHOW_HOME.with(|s| *s.borrow_mut() = Some(show_home.clone()));

    let (home_root, _search, _filter) = home::build(open_category.clone(), open_page.clone());
    content_stack.add_named(&home_root, Some("home"));
    content_stack.add_named(&sheet_card, Some("category"));
    content_stack.set_visible_child_name("home");

    let mini = gtk::Box::new(gtk::Orientation::Vertical, 6);
    mini.add_css_class("metis-settings-mini-sidebar");
    mini.set_size_request(MINI_SIDEBAR_WIDTH, -1);
    mini.set_hexpand(false);
    mini.set_vexpand(true);
    mini.set_margin_top(12);
    mini.set_margin_bottom(12);
    mini.append(&home_btn);
    let mini_sep = gtk::Separator::new(gtk::Orientation::Horizontal);
    mini_sep.set_margin_start(14);
    mini_sep.set_margin_end(14);
    mini_sep.set_margin_top(4);
    mini_sep.set_margin_bottom(4);
    mini.append(&mini_sep);
    for (_, btn) in &mini_cat_btns {
        mini.append(btn);
    }

    let layout = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    layout.add_css_class("metis-settings-root");
    layout.set_hexpand(true);
    layout.set_vexpand(true);
    layout.append(&mini);
    layout.append(&gtk::Separator::new(gtk::Orientation::Vertical));
    layout.append(&content_stack);

    // Overlay: chrome stays put; top-slide sheets float above a scrim.
    let overlay = gtk::Overlay::new();
    overlay.set_hexpand(true);
    overlay.set_vexpand(true);
    overlay.set_child(Some(&layout));
    dialog::install(&overlay);

    let (prev_w, prev_h) = (window.default_size().0, window.default_size().1);
    let mapped_w = window.width();
    let mapped_h = window.height();
    window.set_child(Some(&overlay));
    let keep_w = if mapped_w > 1 {
        mapped_w
    } else if prev_w > 0 {
        prev_w
    } else {
        1100
    };
    let keep_h = if mapped_h > 1 {
        mapped_h
    } else if prev_h > 0 {
        prev_h
    } else {
        720
    };
    window.set_default_size(keep_w, keep_h);

    if let Some(page) = launch.page.as_deref() {
        open_page(page);
        if page == "network" {
            if let Some(tab) = launch.tab.as_deref() {
                pages::network::request_tab(tab);
            }
        }
    } else {
        show_home();
    }

    present();

    {
        let app = app.clone();
        i18n_gtk::register_ui_rebuild(Rc::new(move |page_id| {
            open(
                &app,
                PageLaunch {
                    page: Some(page_id),
                    tab: None,
                },
                true,
            );
        }));
    }
}

fn build_page_stack(window: &gtk::ApplicationWindow, launch: &PageLaunch) -> gtk::Stack {
    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_hhomogeneous(false);
    stack.set_vhomogeneous(false);
    stack.set_transition_duration(motion::ms(180));
    stack.set_hexpand(true);
    stack.set_vexpand(true);

    stack.add_titled(
        &pages::appearance::build(),
        Some("appearance"),
        "Appearance",
    );
    stack.add_titled(
        &pages::background::build(),
        Some("background"),
        "Background",
    );
    stack.add_titled(&pages::edgebar::build(), Some("edgebar"), "Edge bar");
    stack.add_titled(
        &pages::desktop_widgets::build(),
        Some("desktop_widgets"),
        "Desktop widgets",
    );
    stack.add_titled(&pages::windows::build(), Some("windows"), "Windows");
    stack.add_titled(
        &pages::titlebars::build(),
        Some("titlebars"),
        "App titlebars",
    );
    stack.add_titled(&pages::menu::build(), Some("menu"), "Metis Menu");
    stack.add_titled(&pages::weather::build(), Some("weather"), "Weather");
    stack.add_titled(
        &pages::network::build(if launch.page.as_deref() == Some("network") {
            launch.tab.as_deref()
        } else {
            None
        }),
        Some("network"),
        "Network",
    );
    stack.add_titled(&pages::calendars::build(), Some("calendars"), "Calendars");
    stack.add_titled(&pages::mouse::build(), Some("mouse"), "Mouse");
    stack.add_titled(&pages::touchpad::build(), Some("touchpad"), "Touchpad");
    stack.add_titled(&pages::keyboard::build(), Some("keyboard"), "Keyboard");
    stack.add_titled(&pages::shortcuts::build(), Some("shortcuts"), "Shortcuts");
    stack.add_titled(&pages::bluetooth::build(), Some("bluetooth"), "Bluetooth");
    stack.add_titled(&pages::printers::build(), Some("printers"), "Printers");
    stack.add_titled(
        &pages::screenshot::build(),
        Some("screenshot"),
        "Screenshot",
    );
    stack.add_titled(
        &pages::control_center::build(),
        Some("control_center"),
        "Control Center",
    );
    stack.add_titled(&pages::sound::build(), Some("sound"), "Sound");
    stack.add_titled(&pages::power::build(), Some("power"), "Power");
    stack.add_titled(&pages::locale::build(), Some("locale"), "Language & region");
    stack.add_titled(&pages::users::build(), Some("users"), "Users");
    stack.add_titled(&pages::date_time::build(), Some("date_time"), "Date & Time");
    stack.add_titled(&pages::startup::build(), Some("startup"), "Startup");
    stack.add_titled(
        &pages::remote::build(window.upcast_ref()),
        Some("remote"),
        "Remote access",
    );
    stack.add_titled(&pages::gaming::build(), Some("gaming"), "Gaming");
    stack.add_titled(&pages::reset::build(), Some("reset"), "Reset");
    stack.add_titled(&pages::about::build(), Some("about"), "About");
    stack.add_titled(
        &pages::display::build(window.upcast_ref()),
        Some("display"),
        "Display",
    );
    stack
}

fn mini_button(icon: &str, tooltip: &str, hue: Option<NavHue>) -> gtk::Button {
    let btn = gtk::Button::new();
    btn.add_css_class("metis-settings-mini-btn");
    btn.set_tooltip_text(Some(tooltip));
    btn.set_halign(gtk::Align::Center);
    btn.set_focus_on_click(false);

    let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    badge.add_css_class("metis-settings-mini-badge");
    if let Some(hue) = hue {
        badge.add_css_class(hue.css_class());
    } else {
        badge.add_css_class(NavHue::Gray.css_class());
    }
    let img = gtk::Image::from_icon_name(icon);
    img.set_pixel_size(18);
    img.add_css_class("metis-settings-mini-icon");
    badge.append(&img);
    btn.set_child(Some(&badge));
    btn
}

fn sheet_page_row(item: &nav::NavItem) -> gtk::ListBoxRow {
    let row_box = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    row_box.set_margin_top(8);
    row_box.set_margin_bottom(8);
    row_box.set_margin_start(10);
    row_box.set_margin_end(10);

    if let Some(icon) = item.icon {
        let badge = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        badge.add_css_class("metis-settings-nav-icon-wrap");
        if let Some(hue) = item.hue {
            badge.add_css_class(hue.css_class());
        }
        let img = gtk::Image::from_icon_name(icon);
        img.set_pixel_size(14);
        badge.append(&img);
        row_box.append(&badge);
    }

    let label = gtk::Label::new(Some(&tr(item.title)));
    label.set_xalign(0.0);
    label.set_hexpand(true);
    label.add_css_class("metis-settings-sheet-page-label");
    row_box.append(&label);

    let row = gtk::ListBoxRow::new();
    row.add_css_class("metis-settings-sheet-page-row");
    row.set_child(Some(&row_box));
    row
}

fn load_app_icon() -> Option<gtk::gdk::Texture> {
    let bytes = glib::Bytes::from_static(APP_ICON_BYTES);
    match gtk::gdk::Texture::from_bytes(&bytes) {
        Ok(texture) => Some(texture),
        Err(err) => {
            tracing::warn!(%err, "failed to decode embedded settings icon");
            None
        }
    }
}

fn apply_window_icon(window: &gtk::ApplicationWindow) {
    if let Some(texture) = load_app_icon() {
        if let Some(surface) = window.surface() {
            if let Some(toplevel) = surface.downcast_ref::<gtk::gdk::Toplevel>() {
                toplevel.set_icon_list(&[texture]);
                return;
            }
        }
    }
    window.set_icon_name(Some("metis-settings"));
}
