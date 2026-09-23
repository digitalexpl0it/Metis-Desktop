//! Win11-style Notification Center — edge layer-shell panel.
//!
//! Side follows the edge bar: opposite a left/right bar, otherwise the right
//! edge. Vertical attach follows the bar too (under a top bar, above a bottom
//! bar, full height beside a side bar).
//!
//! Opened from the clock (bell merged in). Layout top→bottom:
//! 1. Notifications card (header always visible; list collapsible)
//! 2. Events card (header always visible; body collapses when empty)
//! 3. Calendar / tools card pinned to the bottom (icon rail)

mod notif_list;

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration as StdDuration;

use chrono::{DateTime, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use gtk::gdk;
use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use metis_config::BarPosition;

use crate::config::load_clocks_config;
use crate::services::{CalCommand, CalendarEvent, LocalEvent, spawn_calendar_service};
use crate::ui::bar::widgets::clock::Store;
use crate::ui::bar::widgets::clock::alarms::AlarmsPage;
use crate::ui::bar::widgets::clock::calendar::{CalendarPage, CreateRequest, EventView};
use crate::ui::bar::widgets::clock::stopwatch::StopwatchPage;
use crate::ui::bar::widgets::clock::timer::TimerPage;
use crate::ui::bar::widgets::clock::world::WorldClocksPage;
use crate::ui::icons;

use notif_list::NotificationsCard;

const PANEL_WIDTH: i32 = 400;
/// Slide duration (ease-out cubic).
const SLIDE_MS: u32 = 200;
/// Off-screen park: negative margin on the slide edge pushes the surface past
/// the output edge. Positive margins inset inward (wrong direction for hide).
const HIDDEN_SLIDE_MARGIN: i32 = -(PANEL_WIDTH + 8);
/// Approximate height reserved for the bottom calendar/tools card.
const TOOLS_CARD_RESERVE: i32 = 360;

#[derive(Clone, Copy)]
struct NcLayout {
    /// Horizontal edge the panel slides on (`Left` or `Right`).
    slide_edge: Edge,
    top: i32,
    bottom: i32,
    side_class: &'static str,
    attach_class: &'static str,
}

struct Center {
    window: gtk::Window,
    panel: gtk::Box,
    /// User intent: panel should be open (true) or closed (false).
    open: Cell<bool>,
    animating: Cell<bool>,
    /// Bumped to cancel an in-flight slide when the user reverses direction.
    anim_gen: Cell<u64>,
    slide_edge: Cell<Edge>,
    slide_margin: Cell<i32>,
    notif_card: NotificationsCard,
    events_body: gtk::Revealer,
    calendar: CalendarPage,
    world: WorldClocksPage,
    cal_tx: std::sync::mpsc::Sender<CalCommand>,
}

thread_local! {
    static CENTER: RefCell<Option<Rc<Center>>> = const { RefCell::new(None) };
    /// After an outside-click dismiss, ignore a follow-up clock `show()` for a
    /// short window so press→dismiss + click→toggle does not reopen the panel.
    static SUPPRESS_SHOW_UNTIL_MS: Cell<u128> = const { Cell::new(0) };
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

fn bar_cross_inset() -> i32 {
    // Prefer the live bar geometry (set when the edge bar maps); fall back to
    // config so NC still positions correctly before the bar finishes init.
    let live = crate::ui::bar::dashboard_layer_inset();
    if live > 0 {
        live
    } else {
        metis_config::bar::bar_pill_inset(&crate::config::load_bar_config())
    }
}

fn current_bar_position() -> BarPosition {
    crate::config::load_bar_config().position
}

fn nc_layout() -> NcLayout {
    // Always use DontCare + an explicit attach margin from config. Relying on
    // Neutral exclusive failed for bottom bars (layer exclusive map did not
    // shrink), so the panel stayed full-height and painted over the pill.
    let inset = bar_cross_inset();
    match current_bar_position() {
        BarPosition::Right => NcLayout {
            slide_edge: Edge::Left,
            top: 0,
            bottom: 0,
            side_class: "metis-nc-side-left",
            attach_class: "metis-nc-attach-full",
        },
        BarPosition::Left => NcLayout {
            slide_edge: Edge::Right,
            top: 0,
            bottom: 0,
            side_class: "metis-nc-side-right",
            attach_class: "metis-nc-attach-full",
        },
        BarPosition::Bottom => NcLayout {
            slide_edge: Edge::Right,
            top: 0,
            bottom: inset,
            side_class: "metis-nc-side-right",
            attach_class: "metis-nc-attach-bottom",
        },
        BarPosition::Top => NcLayout {
            slide_edge: Edge::Right,
            top: inset,
            bottom: 0,
            side_class: "metis-nc-side-right",
            attach_class: "metis-nc-attach-top",
        },
    }
}

fn apply_layout_classes(panel: &gtk::Box, layout: &NcLayout) {
    for class in [
        "metis-nc-side-left",
        "metis-nc-side-right",
        "metis-nc-attach-top",
        "metis-nc-attach-bottom",
        "metis-nc-attach-full",
    ] {
        panel.remove_css_class(class);
    }
    panel.add_css_class(layout.side_class);
    panel.add_css_class(layout.attach_class);
}

fn apply_window_layout(center: &Center, layout: &NcLayout, slide_margin: i32) {
    let window = &center.window;
    for edge in [Edge::Left, Edge::Right, Edge::Top, Edge::Bottom] {
        window.set_anchor(edge, false);
        window.set_margin(edge, 0);
    }
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Bottom, true);
    window.set_anchor(layout.slide_edge, true);
    window.set_margin(Edge::Top, layout.top);
    window.set_margin(Edge::Bottom, layout.bottom);
    window.set_margin(layout.slide_edge, slide_margin);
    // DontCare: layout against the full output so our explicit top/bottom
    // margins (bar strip) are not stacked on a Neutral exclusive inset.
    window.set_exclusive_zone(-1);
    center.slide_edge.set(layout.slide_edge);
    center.slide_margin.set(slide_margin);
    apply_layout_classes(&center.panel, layout);
}

fn primary_monitor_height() -> i32 {
    if let Some(display) = gdk::Display::default()
        && let Some(obj) = display.monitors().item(0)
        && let Ok(monitor) = obj.downcast::<gdk::Monitor>()
    {
        let h = monitor.geometry().height();
        if h > 0 {
            return h;
        }
    }
    1080
}

/// Max heights for the notifications and events scrollers, scaled to the
/// current monitor so taller displays show more before scrolling.
pub fn scroll_budgets() -> (i32, i32) {
    let layout = nc_layout();
    let vertical = layout.top + layout.bottom;
    let available = (primary_monitor_height() - vertical - TOOLS_CARD_RESERVE - 48).max(220);
    let notif = ((available as f64) * 0.62).round() as i32;
    let events = ((available as f64) * 0.38).round() as i32;
    (notif.clamp(140, 640), events.clamp(100, 360))
}

fn apply_scroll_budgets(center: &Center) {
    let (notif_h, events_h) = scroll_budgets();
    center.notif_card.set_list_max_height(notif_h);
    center.calendar.set_events_list_max_height(events_h);
}

pub fn init() {
    let _ = ensure();
}

pub fn is_active() -> bool {
    CENTER.with(|c| c.borrow().as_ref().is_some_and(|x| x.open.get()))
}

/// True while the notification center panel is open.
pub fn is_open() -> bool {
    is_active()
}

/// Whether the panel is currently (or would be) anchored on the right edge.
pub fn anchors_right() -> bool {
    nc_layout().slide_edge == Edge::Right
}

pub fn on_theme_changed() {
    CENTER.with(|c| {
        if let Some(center) = c.borrow().as_ref() {
            center.window.queue_draw();
        }
    });
}

/// Re-apply side / height attach when the edge bar position or size changes.
pub fn on_bar_config_changed() {
    CENTER.with(|c| {
        let Some(center) = c.borrow().as_ref().cloned() else {
            return;
        };
        let layout = nc_layout();
        let margin = if center.open.get() {
            0
        } else {
            HIDDEN_SLIDE_MARGIN
        };
        // Cancel any in-flight slide; geometry swap would fight the tick.
        center.anim_gen.set(center.anim_gen.get().wrapping_add(1));
        center.animating.set(false);
        apply_window_layout(&center, &layout, margin);
        apply_scroll_budgets(&center);
        if center.open.get() {
            crate::ui::toast::set_panel_open(true);
        }
    });
}

/// Destroy and recreate the panel so chrome picks up a new language catalog.
pub fn reload_for_locale() {
    let was_open = is_active();
    CENTER.with(|c| {
        if let Some(old) = c.borrow_mut().take() {
            old.anim_gen.set(old.anim_gen.get().wrapping_add(1));
            old.animating.set(false);
            old.open.set(false);
            crate::ui::toast::set_panel_open(false);
            old.window.set_visible(false);
            old.window.destroy();
        }
    });
    if was_open {
        show();
    }
}

/// Drop the Notification Center under the screenshot Overlay layer so the
/// picker UI paints and receives hits on top, while the panel stays visible
/// (and capturable) on `Top`.
pub fn set_below_screenshot(below: bool) {
    CENTER.with(|c| {
        let borrow = c.borrow();
        let Some(center) = borrow.as_ref() else {
            return;
        };
        if !center.window.is_visible() {
            return;
        }
        center
            .window
            .set_layer(if below { Layer::Top } else { Layer::Overlay });
        center.window.set_keyboard_mode(if below {
            KeyboardMode::OnDemand
        } else {
            KeyboardMode::Exclusive
        });
    });
}

pub fn toggle() {
    if is_active() {
        dismiss();
    } else {
        show();
    }
}

fn set_slide_margin(center: &Center, margin: i32) {
    center.slide_margin.set(margin);
    center.window.set_margin(center.slide_edge.get(), margin);
}

/// Animate layer-shell slide-edge margin with ease-out cubic. A new call bumps
/// `anim_gen` so a reverse mid-slide cancels the previous tick cleanly.
fn animate_slide_margin(center: Rc<Center>, target: i32, on_done: impl FnOnce(&Center) + 'static) {
    let r#gen = center.anim_gen.get().wrapping_add(1);
    center.anim_gen.set(r#gen);
    center.animating.set(true);
    let start = center.slide_margin.get();
    let delta = target - start;
    if delta == 0 {
        center.animating.set(false);
        on_done(&center);
        return;
    }
    let start_at = glib::monotonic_time();
    let duration_us = (SLIDE_MS as i64) * 1000;
    let mut on_done = Some(on_done);

    glib::timeout_add_local(StdDuration::from_millis(16), move || {
        if center.anim_gen.get() != r#gen {
            return glib::ControlFlow::Break;
        }
        let elapsed = glib::monotonic_time() - start_at;
        let t = (elapsed as f64 / duration_us as f64).clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let margin = start + (delta as f64 * eased).round() as i32;
        set_slide_margin(&center, margin);
        if t >= 1.0 {
            set_slide_margin(&center, target);
            center.animating.set(false);
            if let Some(cb) = on_done.take() {
                cb(&center);
            }
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}

pub fn show() {
    if now_ms() < SUPPRESS_SHOW_UNTIL_MS.with(Cell::get) {
        return;
    }
    crate::ui::bar::notify_bar_interaction();
    crate::ui::bar::close_bar_popovers();
    crate::ui::dashboard::request_close();

    let center = ensure();
    let layout = nc_layout();
    apply_scroll_budgets(&center);
    center.notif_card.refresh();
    let _ = center.cal_tx.send(CalCommand::Refresh);
    let has_events = center.calendar.selected_day_has_events();
    center.events_body.set_reveal_child(has_events);

    // Refresh attach / side in case the bar moved since last open.
    let keep_open = center.open.get()
        && center.window.is_visible()
        && center.slide_margin.get() == 0
        && !center.animating.get()
        && center.slide_edge.get() == layout.slide_edge;
    apply_window_layout(
        &center,
        &layout,
        if keep_open {
            0
        } else if center.window.is_visible() && center.open.get() {
            center.slide_margin.get()
        } else {
            HIDDEN_SLIDE_MARGIN
        },
    );

    if keep_open {
        let _ = center.window.grab_focus();
        return;
    }

    center.open.set(true);
    crate::ui::toast::set_panel_open(true);

    if !center.window.is_visible() {
        set_slide_margin(&center, HIDDEN_SLIDE_MARGIN);
        center.window.set_visible(true);
        center.window.present();
    }

    let focus = center.clone();
    animate_slide_margin(center, 0, move |_| {
        let _ = focus.window.grab_focus();
    });
}

pub fn dismiss() {
    CENTER.with(|c| {
        let Some(center) = c.borrow().as_ref().cloned() else {
            return;
        };
        if !center.open.get() {
            return;
        }
        center.open.set(false);
        crate::ui::toast::set_panel_open(false);
        SUPPRESS_SHOW_UNTIL_MS.with(|cell| cell.set(now_ms().saturating_add(350)));

        if !center.window.is_visible() {
            set_slide_margin(&center, HIDDEN_SLIDE_MARGIN);
            return;
        }

        animate_slide_margin(center, HIDDEN_SLIDE_MARGIN, move |c| {
            // Only park if the user did not reopen during the slide-out.
            if !c.open.get() {
                c.window.set_visible(false);
            }
        });
    });
}

fn ensure() -> Rc<Center> {
    CENTER.with(|cell| {
        if let Some(existing) = cell.borrow().as_ref() {
            return existing.clone();
        }
        let center = build_center();
        *cell.borrow_mut() = Some(center.clone());
        center
    })
}

fn build_center() -> Rc<Center> {
    let bar_cfg = crate::config::load_bar_config();
    let layout = nc_layout();

    let window = gtk::Window::builder()
        .title(metis_i18n::tr("Notification Center"))
        .decorated(false)
        .build();
    window.add_css_class("metis-nc-window");
    window.set_can_focus(true);
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-notification-center"));
    // exclusive_zone / margins applied in apply_window_layout (DontCare + bar inset).
    window.set_default_size(PANEL_WIDTH, 800);
    window.set_size_request(PANEL_WIDTH, -1);

    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    panel.add_css_class("metis-nc-panel");
    panel.set_size_request(PANEL_WIDTH, -1);
    panel.set_hexpand(true);
    panel.set_vexpand(true);
    // No widget margins — panel fills the layer. Internal inset comes from CSS.
    // Top cluster: notifications + events (shrink to content).
    let top = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(10)
        .build();
    top.set_vexpand(false);

    let notif_card = NotificationsCard::new();
    top.append(notif_card.root());

    let store = Store(Rc::new(RefCell::new(load_clocks_config(
        &bar_cfg.clock.timezones,
    ))));

    let calendar = CalendarPage::new();
    let world = WorldClocksPage::new(store.clone());
    let stopwatch = StopwatchPage::new();
    let timer = TimerPage::new();
    let alarms = AlarmsPage::new(store.clone());

    // Events card always visible; body collapses when the selected day is empty.
    let events_card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    events_card.add_css_class("metis-nc-card");
    let events_header = gtk::Label::builder()
        .label(metis_i18n::tr("Events"))
        .halign(gtk::Align::Start)
        .build();
    events_header.add_css_class("metis-nc-card-title");
    events_card.append(&events_header);

    let events_body = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::SlideDown)
        .transition_duration(SLIDE_MS)
        .reveal_child(false)
        .child(&calendar.events_widget)
        .build();
    events_card.append(&events_body);
    top.append(&events_card);

    {
        let events_body = events_body.clone();
        calendar.set_on_selection_change(move |has| {
            events_body.set_reveal_child(has);
        });
    }

    panel.append(&top);

    // Flexible spacer pushes the calendar/tools card to the bottom.
    let spacer = gtk::Box::new(gtk::Orientation::Vertical, 0);
    spacer.set_vexpand(true);
    spacer.set_hexpand(true);
    panel.append(&spacer);

    // Bottom: calendar / tools card (Win11-style).
    let tools_card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .build();
    tools_card.add_css_class("metis-nc-card");
    tools_card.set_valign(gtk::Align::End);
    tools_card.set_vexpand(false);

    let tools_header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    let date_label = gtk::Label::builder()
        .label(Local::now().format("%A %-d %B").to_string())
        .halign(gtk::Align::Start)
        .hexpand(true)
        .build();
    date_label.add_css_class("metis-nc-card-title");
    tools_header.append(&date_label);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(100)
        .vexpand(false)
        .build();
    // Size to the visible page — otherwise the tall timer/alarms pages force a
    // huge empty region under the calendar grid.
    stack.set_hhomogeneous(true);
    stack.set_vhomogeneous(false);
    stack.add_named(&calendar.widget, Some("calendar"));
    stack.add_named(&world.widget, Some("clocks"));
    stack.add_named(&stopwatch.widget, Some("stopwatch"));
    stack.add_named(&timer.widget, Some("timer"));
    stack.add_named(&alarms.widget, Some("alarms"));

    let rail = build_tool_rail(
        &stack,
        &[
            (
                "calendar",
                &metis_i18n::tr("Calendar"),
                "x-office-calendar-symbolic",
            ),
            (
                "clocks",
                &metis_i18n::tr("World clocks"),
                "preferences-system-time-symbolic",
            ),
            (
                "stopwatch",
                &metis_i18n::tr("Stopwatch"),
                "media-playback-start-symbolic",
            ),
            ("timer", &metis_i18n::tr("Timer"), "alarm-symbolic"),
            (
                "alarms",
                &metis_i18n::tr("Alarms"),
                "appointment-soon-symbolic",
            ),
        ],
    );
    tools_header.append(&rail);
    tools_card.append(&tools_header);
    tools_card.append(&stack);
    panel.append(&tools_card);

    let (cal_tx, cal_rx) = spawn_calendar_service();
    {
        let tx = cal_tx.clone();
        calendar.set_on_month_change(move |a, b| {
            let _ = tx.send(CalCommand::SetRange {
                since: day_start(a),
                until: day_end(b),
            });
        });
    }
    {
        let tx = cal_tx.clone();
        calendar.set_on_dismiss(move |ev| {
            let _ = tx.send(CalCommand::Dismiss(ev.uid.clone()));
        });
    }
    {
        let tx = cal_tx.clone();
        calendar.set_on_delete(move |ev| {
            let _ = tx.send(CalCommand::Delete(ev.uid.clone()));
        });
    }
    {
        let tx = cal_tx.clone();
        calendar.set_on_create(move |req| {
            let _ = tx.send(CalCommand::AddLocal(local_event_from(req)));
        });
    }
    {
        let tx = cal_tx.clone();
        calendar.set_on_refresh(move || {
            let _ = tx.send(CalCommand::Refresh);
        });
    }
    {
        let (a, b) = calendar.visible_range();
        let _ = cal_tx.send(CalCommand::SetRange {
            since: day_start(a),
            until: day_end(b),
        });
    }

    let cal_tx_for_center = cal_tx.clone();

    window.set_child(Some(&panel));

    let key = gtk::EventControllerKey::new();
    key.set_propagation_phase(gtk::PropagationPhase::Capture);
    key.connect_key_pressed(|_, key, _, _| {
        if key == gdk::Key::Escape {
            dismiss();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);

    let date_label_tick = date_label.clone();
    glib::timeout_add_local(StdDuration::from_secs(1), move || {
        date_label_tick.set_label(&Local::now().format("%A %-d %B").to_string());
        CENTER.with(|c| {
            if let Some(center) = c.borrow().as_ref()
                && center.open.get()
            {
                center.world.refresh();
            }
        });
        glib::ControlFlow::Continue
    });

    if std::env::var("METIS_DEMO_NOTIFICATIONS").is_ok() {
        notif_list::seed_demo_notifications();
    }

    let center = Rc::new(Center {
        window,
        panel: panel.clone(),
        open: Cell::new(false),
        animating: Cell::new(false),
        anim_gen: Cell::new(0),
        slide_edge: Cell::new(layout.slide_edge),
        slide_margin: Cell::new(HIDDEN_SLIDE_MARGIN),
        notif_card,
        events_body: events_body.clone(),
        calendar,
        world,
        cal_tx: cal_tx_for_center,
    });
    apply_window_layout(&center, &layout, HIDDEN_SLIDE_MARGIN);
    apply_scroll_budgets(&center);

    let center_rx = center.clone();
    glib::timeout_add_local(StdDuration::from_millis(500), move || {
        let mut latest = None;
        while let Ok(events) = cal_rx.try_recv() {
            latest = Some(events);
        }
        if let Some(events) = latest {
            let views: Vec<EventView> = events.iter().map(event_to_view).collect();
            center_rx.calendar.set_events(views);
            center_rx
                .events_body
                .set_reveal_child(center_rx.calendar.selected_day_has_events());
        }
        glib::ControlFlow::Continue
    });

    center
}

fn build_tool_rail(stack: &gtk::Stack, tabs: &[(&str, &str, &str)]) -> gtk::Box {
    let rail = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(2)
        .halign(gtk::Align::End)
        .build();
    rail.add_css_class("metis-nc-tool-rail");

    let mut group: Option<gtk::ToggleButton> = None;
    for (i, (name, tip, icon)) in tabs.iter().enumerate() {
        let btn = gtk::ToggleButton::new();
        btn.set_child(Some(&icons::image(icon)));
        btn.set_tooltip_text(Some(tip));
        btn.add_css_class("metis-nc-tool-btn");
        if let Some(ref leader) = group {
            btn.set_group(Some(leader));
        } else {
            group = Some(btn.clone());
        }
        if i == 0 {
            btn.set_active(true);
        }
        {
            let stack = stack.clone();
            let name = (*name).to_string();
            btn.connect_toggled(move |b| {
                if b.is_active() {
                    stack.set_visible_child_name(&name);
                }
            });
        }
        rail.append(&btn);
    }
    rail
}

fn day_start(date: NaiveDate) -> DateTime<Local> {
    date.and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .unwrap_or_else(Local::now)
}

fn day_end(date: NaiveDate) -> DateTime<Local> {
    date.and_hms_opt(23, 59, 59)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .unwrap_or_else(Local::now)
}

fn event_to_view(event: &CalendarEvent) -> EventView {
    let start_date = event.start.date_naive();
    let end_date = if event.all_day {
        (event.end - Duration::days(1)).date_naive().max(start_date)
    } else {
        event.end.date_naive()
    };
    EventView {
        uid: event.id.clone(),
        date: start_date,
        end_date,
        start: if event.all_day {
            None
        } else {
            Some(event.start.time())
        },
        all_day: event.all_day,
        title: event.summary.clone(),
        location: event.location.clone(),
        color: event.color.clone(),
        can_delete: event.can_delete,
    }
}

fn local_event_from(req: CreateRequest) -> LocalEvent {
    let start = if req.all_day {
        day_start(req.date)
    } else {
        let time = req
            .start
            .unwrap_or_else(|| NaiveTime::from_hms_opt(9, 0, 0).unwrap_or_default());
        Local
            .from_local_datetime(&req.date.and_time(time))
            .single()
            .unwrap_or_else(Local::now)
    };
    let end = if req.all_day {
        start + Duration::hours(24)
    } else {
        start + Duration::hours(1)
    };
    LocalEvent {
        id: new_event_id(),
        summary: req.title,
        start,
        end,
        all_day: req.all_day,
        location: None,
    }
}

fn new_event_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("evt-{nanos}")
}
