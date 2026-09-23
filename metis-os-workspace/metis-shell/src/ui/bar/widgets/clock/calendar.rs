use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use chrono::{Datelike, Days, Local, Months, NaiveDate, NaiveTime};
use gtk::prelude::*;

/// Display model the calendar renders. Decoupled from the backend `Event` so the
/// providers can be wired in later without touching the UI.
#[derive(Clone)]
pub struct EventView {
    pub uid: String,
    pub date: NaiveDate,
    pub end_date: NaiveDate,
    pub start: Option<NaiveTime>,
    pub all_day: bool,
    pub title: String,
    pub location: Option<String>,
    pub color: Option<String>,
    pub can_delete: bool,
}

type EventAction = Rc<dyn Fn(&EventView)>;

/// A new event the user wants to create on the local calendar.
#[derive(Clone)]
pub struct CreateRequest {
    pub date: NaiveDate,
    pub title: String,
    pub all_day: bool,
    pub start: Option<NaiveTime>,
}

struct Callbacks {
    on_month_change: Option<Box<dyn Fn(NaiveDate, NaiveDate)>>,
    on_create: Option<Box<dyn Fn(CreateRequest)>>,
    on_dismiss: Option<EventAction>,
    on_delete: Option<EventAction>,
    on_refresh: Option<Box<dyn Fn()>>,
    on_selection_change: Option<Box<dyn Fn(bool)>>,
}

struct Inner {
    shown: RefCell<NaiveDate>,
    selected: RefCell<NaiveDate>,
    grid: gtk::Grid,
    title: gtk::Label,
    header_weekday: gtk::Label,
    header_date: gtk::Label,
    events_title: gtk::Label,
    events_box: gtk::Box,
    events: RefCell<Vec<EventView>>,
    event_days: RefCell<HashSet<NaiveDate>>,
    cb: RefCell<Callbacks>,
}

pub struct CalendarPage {
    /// Month grid + navigation (calendar tools page body).
    pub widget: gtk::Widget,
    /// Selected-day events list + add form (events card body).
    pub events_widget: gtk::Widget,
    events_scroll: gtk::ScrolledWindow,
    inner: Rc<Inner>,
}

impl CalendarPage {
    pub fn new() -> Self {
        let today = Local::now().date_naive();
        let first_of_month = today.with_day(1).unwrap_or(today);

        // ---- Calendar column: header + month grid ----
        let left = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        left.add_css_class("metis-cal-columns");
        left.set_width_request(-1);
        left.set_hexpand(true);

        let header_weekday = gtk::Label::builder().halign(gtk::Align::Start).build();
        header_weekday.add_css_class("metis-cal-head-weekday");
        let header_date = gtk::Label::builder().halign(gtk::Align::Start).build();
        header_date.add_css_class("metis-cal-head-date");
        left.append(&header_weekday);
        left.append(&header_date);

        let nav = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .build();
        nav.set_margin_top(6);
        let prev_year = nav_button("\u{00AB}");
        let prev_month = nav_button("\u{2039}");
        let title = gtk::Label::builder().hexpand(true).build();
        title.add_css_class("metis-cal-title");
        let next_month = nav_button("\u{203A}");
        let next_year = nav_button("\u{00BB}");
        let today_btn = gtk::Button::builder()
            .label(metis_i18n::tr("Today"))
            .build();
        today_btn.add_css_class("metis-cal-today-btn");
        nav.append(&prev_year);
        nav.append(&prev_month);
        nav.append(&title);
        nav.append(&next_month);
        nav.append(&next_year);
        nav.append(&today_btn);
        left.append(&nav);

        let grid = gtk::Grid::builder()
            .row_spacing(2)
            .column_spacing(2)
            .column_homogeneous(true)
            .build();
        grid.add_css_class("metis-cal-grid");
        left.append(&grid);

        // ---- Events column: events for the selected day ----
        let right = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        right.add_css_class("metis-cal-events-pane");
        right.set_width_request(240);
        right.set_hexpand(true);

        let events_header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        let events_title = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        events_title.add_css_class("metis-bar-section-title");
        let refresh_btn = gtk::Button::from_icon_name("view-refresh-symbolic");
        refresh_btn.add_css_class("metis-cal-event-action");
        refresh_btn.set_tooltip_text(Some(&metis_i18n::tr("Refresh")));
        events_header.append(&events_title);
        events_header.append(&refresh_btn);
        right.append(&events_header);

        let events_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        let events_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .overlay_scrolling(false)
            .min_content_height(80)
            .max_content_height(180)
            .child(&events_box)
            .build();
        events_scroll.add_css_class("metis-nc-scrolled");
        events_scroll.add_css_class("metis-cal-events-scroll");
        right.append(&events_scroll);

        let add_btn = gtk::Button::builder()
            .label(metis_i18n::tr("+ Add event"))
            .build();
        add_btn.add_css_class("metis-cal-add-btn");
        add_btn.set_halign(gtk::Align::Start);
        right.append(&add_btn);

        // Inline add-event form (hidden until "+ Add event").
        let form_revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .build();
        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        form.add_css_class("metis-cal-form");
        let title_entry = gtk::Entry::builder()
            .placeholder_text(metis_i18n::tr("Event title"))
            .build();
        form.append(&title_entry);
        let time_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        let all_day_check = gtk::CheckButton::with_label(&metis_i18n::tr("All day"));
        let hour_spin = gtk::SpinButton::with_range(0.0, 23.0, 1.0);
        hour_spin.set_value(9.0);
        let min_spin = gtk::SpinButton::with_range(0.0, 59.0, 1.0);
        time_row.append(&all_day_check);
        time_row.append(&hour_spin);
        time_row.append(&gtk::Label::new(Some(":")));
        time_row.append(&min_spin);
        form.append(&time_row);
        let form_actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::End)
            .build();
        let save_btn = gtk::Button::with_label(&metis_i18n::tr("Save"));
        save_btn.add_css_class("metis-cal-add-btn");
        let cancel_btn = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
        cancel_btn.add_css_class("metis-clock-btn");
        form_actions.append(&cancel_btn);
        form_actions.append(&save_btn);
        form.append(&form_actions);
        form_revealer.set_child(Some(&form));
        right.append(&form_revealer);

        {
            let all_day_check = all_day_check.clone();
            let hour_spin = hour_spin.clone();
            let min_spin = min_spin.clone();
            all_day_check.connect_toggled(move |c| {
                let timed = !c.is_active();
                hour_spin.set_sensitive(timed);
                min_spin.set_sensitive(timed);
            });
        }

        // Panel layout: calendar grid and events are separate widgets so the
        // Notification Center can place them in different cards.
        left.set_hexpand(true);
        right.set_hexpand(true);
        right.set_width_request(-1);

        let inner = Rc::new(Inner {
            shown: RefCell::new(first_of_month),
            selected: RefCell::new(today),
            grid,
            title,
            header_weekday,
            header_date,
            events_title,
            events_box,
            events: RefCell::new(Vec::new()),
            event_days: RefCell::new(HashSet::new()),
            cb: RefCell::new(Callbacks {
                on_month_change: None,
                on_create: None,
                on_dismiss: None,
                on_delete: None,
                on_refresh: None,
                on_selection_change: None,
            }),
        });

        wire_nav(&inner, &prev_year, -12);
        wire_nav(&inner, &prev_month, -1);
        wire_nav(&inner, &next_month, 1);
        wire_nav(&inner, &next_year, 12);
        {
            let inner = inner.clone();
            today_btn.connect_clicked(move |_| {
                let today = Local::now().date_naive();
                *inner.shown.borrow_mut() = today.with_day(1).unwrap_or(today);
                *inner.selected.borrow_mut() = today;
                inner.rebuild();
                inner.notify_month_change();
            });
        }
        {
            let revealer = form_revealer.clone();
            let title_entry = title_entry.clone();
            add_btn.connect_clicked(move |_| {
                let show = !revealer.reveals_child();
                revealer.set_reveal_child(show);
                if show {
                    title_entry.grab_focus();
                }
            });
        }
        {
            let revealer = form_revealer.clone();
            cancel_btn.connect_clicked(move |_| revealer.set_reveal_child(false));
        }
        {
            let inner = inner.clone();
            refresh_btn.connect_clicked(move |_| {
                if let Some(cb) = inner.cb.borrow().on_refresh.as_ref() {
                    cb();
                }
            });
        }
        {
            let inner = inner.clone();
            let revealer = form_revealer.clone();
            let title_entry = title_entry.clone();
            let all_day_check = all_day_check.clone();
            let hour_spin = hour_spin.clone();
            let min_spin = min_spin.clone();
            save_btn.connect_clicked(move |_| {
                let title = title_entry.text().trim().to_string();
                if title.is_empty() {
                    title_entry.grab_focus();
                    return;
                }
                let all_day = all_day_check.is_active();
                let start = if all_day {
                    None
                } else {
                    NaiveTime::from_hms_opt(hour_spin.value() as u32, min_spin.value() as u32, 0)
                };
                let req = CreateRequest {
                    date: *inner.selected.borrow(),
                    title,
                    all_day,
                    start,
                };
                if let Some(cb) = inner.cb.borrow().on_create.as_ref() {
                    cb(req);
                }
                title_entry.set_text("");
                revealer.set_reveal_child(false);
            });
        }

        inner.rebuild();

        Self {
            widget: left.upcast(),
            events_widget: right.upcast(),
            events_scroll,
            inner,
        }
    }

    /// Cap the events list height so long days scroll instead of blowing the panel.
    pub fn set_events_list_max_height(&self, max_h: i32) {
        self.events_scroll.set_max_content_height(max_h.max(80));
    }
    /// Combined two-column widget for legacy popover layouts.
    #[allow(dead_code)] // legacy calendar popover layout API
    pub fn combined_widget(&self) -> gtk::Widget {
        let columns = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(18)
            .build();
        columns.add_css_class("metis-cal-columns");
        columns.append(&self.widget);
        let sep = gtk::Separator::new(gtk::Orientation::Vertical);
        columns.append(&sep);
        columns.append(&self.events_widget);
        columns.upcast()
    }

    /// True when the selected day has at least one event.
    pub fn selected_day_has_events(&self) -> bool {
        let sel = *self.inner.selected.borrow();
        self.inner
            .events
            .borrow()
            .iter()
            .any(|e| sel >= e.date && sel <= e.end_date)
    }

    pub fn set_on_selection_change(&self, f: impl Fn(bool) + 'static) {
        self.inner.cb.borrow_mut().on_selection_change = Some(Box::new(f));
    }

    /// Called once after construction so the parent can request the first range.
    pub fn set_on_month_change(&self, f: impl Fn(NaiveDate, NaiveDate) + 'static) {
        self.inner.cb.borrow_mut().on_month_change = Some(Box::new(f));
    }

    pub fn set_on_create(&self, f: impl Fn(CreateRequest) + 'static) {
        self.inner.cb.borrow_mut().on_create = Some(Box::new(f));
    }

    pub fn set_on_refresh(&self, f: impl Fn() + 'static) {
        self.inner.cb.borrow_mut().on_refresh = Some(Box::new(f));
    }

    pub fn set_on_dismiss(&self, f: impl Fn(&EventView) + 'static) {
        self.inner.cb.borrow_mut().on_dismiss = Some(Rc::new(f));
    }

    pub fn set_on_delete(&self, f: impl Fn(&EventView) + 'static) {
        self.inner.cb.borrow_mut().on_delete = Some(Rc::new(f));
    }

    /// Currently displayed [first_day, last_day] inclusive, padded by a week so
    /// adjacent-month cells with events also light up.
    pub fn visible_range(&self) -> (NaiveDate, NaiveDate) {
        self.inner.visible_range()
    }

    /// Replace the known events; refreshes dots and the selected-day list.
    pub fn set_events(&self, events: Vec<EventView>) {
        let mut days = HashSet::new();
        for ev in &events {
            let mut d = ev.date;
            while d <= ev.end_date {
                days.insert(d);
                let Some(next) = d.checked_add_days(Days::new(1)) else {
                    break;
                };
                d = next;
            }
        }
        *self.inner.event_days.borrow_mut() = days;
        *self.inner.events.borrow_mut() = events;
        self.inner.rebuild();
    }
}

fn nav_button(label: &str) -> gtk::Button {
    let b = gtk::Button::builder().label(label).build();
    b.add_css_class("metis-cal-nav");
    b
}

fn wire_nav(inner: &Rc<Inner>, button: &gtk::Button, delta_months: i32) {
    let inner = inner.clone();
    button.connect_clicked(move |_| {
        let current = *inner.shown.borrow();
        let next = if delta_months >= 0 {
            current.checked_add_months(Months::new(delta_months as u32))
        } else {
            current.checked_sub_months(Months::new((-delta_months) as u32))
        };
        if let Some(next) = next {
            *inner.shown.borrow_mut() = next.with_day(1).unwrap_or(next);
            inner.rebuild();
            inner.notify_month_change();
        }
    });
}

impl Inner {
    fn week_start_offset(&self) -> u32 {
        match metis_config::load_datetime_config()
            .first_day_of_week
            .sunday_based_index()
        {
            Some(0) => 0, // Sunday
            Some(1) => 1, // Monday
            _ => 0,       // Locale default → Sunday-based for now
        }
    }

    fn visible_range(&self) -> (NaiveDate, NaiveDate) {
        let anchor = *self.shown.borrow();
        let start = self.week_start_offset();
        let sunday_based = anchor.weekday().num_days_from_sunday();
        let first_col = (sunday_based + 7 - start) % 7;
        let grid_start = anchor
            .checked_sub_days(Days::new(first_col as u64))
            .unwrap_or(anchor);
        let grid_end = grid_start.checked_add_days(Days::new(41)).unwrap_or(anchor);
        (grid_start, grid_end)
    }

    fn notify_month_change(&self) {
        if let Some(cb) = self.cb.borrow().on_month_change.as_ref() {
            let (a, b) = self.visible_range();
            cb(a, b);
        }
    }

    fn rebuild(self: &Rc<Self>) {
        self.rebuild_header();
        self.rebuild_grid();
        self.rebuild_events();
    }

    fn rebuild_header(&self) {
        let sel = *self.selected.borrow();
        self.header_weekday.set_label(&sel.format("%A").to_string());
        self.header_date
            .set_label(&sel.format("%B %-d %Y").to_string());
        let anchor = *self.shown.borrow();
        self.title.set_label(&anchor.format("%B %Y").to_string());
    }

    fn rebuild_grid(self: &Rc<Self>) {
        while let Some(child) = self.grid.first_child() {
            self.grid.remove(&child);
        }

        let labels = [
            metis_i18n::tr("Su"),
            metis_i18n::tr("Mo"),
            metis_i18n::tr("Tu"),
            metis_i18n::tr("We"),
            metis_i18n::tr("Th"),
            metis_i18n::tr("Fr"),
            metis_i18n::tr("Sa"),
        ];
        let start = self.week_start_offset() as usize;
        for i in 0..7 {
            let wd = &labels[(i + start) % 7];
            let label = gtk::Label::new(Some(wd));
            label.add_css_class("metis-cal-weekday");
            self.grid.attach(&label, i as i32, 0, 1, 1);
        }

        let anchor = *self.shown.borrow();
        let today = Local::now().date_naive();
        let selected = *self.selected.borrow();
        let event_days = self.event_days.borrow();
        let (grid_start, _) = self.visible_range();

        for cell in 0..42 {
            let date = match grid_start.checked_add_days(Days::new(cell as u64)) {
                Some(d) => d,
                None => continue,
            };
            let col = cell % 7;
            let row = (cell / 7) + 1;

            let cell_box = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(1)
                .build();
            let num = gtk::Label::new(Some(&date.day().to_string()));
            num.add_css_class("metis-cal-daynum");
            let dot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            dot.set_halign(gtk::Align::Center);
            dot.add_css_class("metis-cal-dot");
            dot.set_visible(event_days.contains(&date));
            cell_box.append(&num);
            cell_box.append(&dot);

            let btn = gtk::Button::builder().child(&cell_box).build();
            btn.add_css_class("metis-cal-day");
            if date.month() != anchor.month() {
                btn.add_css_class("metis-cal-adjacent");
            }
            if date == today {
                btn.add_css_class("metis-cal-today");
            }
            if date == selected {
                btn.add_css_class("metis-cal-selected");
            }
            {
                let inner = self.clone();
                btn.connect_clicked(move |_| inner.select(date));
            }

            self.grid.attach(&btn, col, row, 1, 1);
        }
    }

    fn rebuild_events(&self) {
        let sel = *self.selected.borrow();
        let today = Local::now().date_naive();
        let title = if sel == today {
            metis_i18n::tr("Today")
        } else {
            sel.format("%a, %b %-d").to_string()
        };
        self.events_title.set_label(&title);

        while let Some(child) = self.events_box.first_child() {
            self.events_box.remove(&child);
        }

        let events = self.events.borrow();
        let mut day_events: Vec<&EventView> = events
            .iter()
            .filter(|e| sel >= e.date && sel <= e.end_date)
            .collect();
        day_events.sort_by(|a, b| {
            b.all_day
                .cmp(&a.all_day)
                .then(a.start.cmp(&b.start))
                .then(a.title.cmp(&b.title))
        });

        if day_events.is_empty() {
            let empty = gtk::Label::builder()
                .label(metis_i18n::tr("No events"))
                .halign(gtk::Align::Start)
                .build();
            empty.add_css_class("metis-cal-empty");
            self.events_box.append(&empty);
            self.notify_selection_change(false);
            return;
        }

        for ev in day_events {
            self.events_box.append(&self.build_event_row(ev));
        }
        self.notify_selection_change(true);
    }

    fn notify_selection_change(&self, has_events: bool) {
        if let Some(cb) = self.cb.borrow().on_selection_change.as_ref() {
            cb(has_events);
        }
    }

    #[allow(deprecated)]
    fn build_event_row(&self, ev: &EventView) -> gtk::Widget {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        row.add_css_class("metis-cal-event");

        let bar = gtk::Box::new(gtk::Orientation::Vertical, 0);
        bar.add_css_class("metis-cal-event-color");
        bar.set_width_request(3);
        if let Some(color) = &ev.color
            && let Ok(provider) = inline_color_css(color)
        {
            bar.style_context()
                .add_provider(&provider, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION);
        }
        row.append(&bar);

        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        let title = gtk::Label::builder()
            .label(&ev.title)
            .halign(gtk::Align::Start)
            .wrap(true)
            .build();
        title.add_css_class("metis-cal-event-title");
        text.append(&title);

        let when = if ev.all_day {
            metis_i18n::tr("All day")
        } else if let Some(t) = ev.start {
            t.format("%-I:%M %p").to_string()
        } else {
            String::new()
        };
        let sub = match (&ev.location, when.is_empty()) {
            (Some(loc), false) => format!("{when}  ·  {loc}"),
            (Some(loc), true) => loc.clone(),
            (None, _) => when,
        };
        if !sub.is_empty() {
            let sub_label = gtk::Label::builder()
                .label(&sub)
                .halign(gtk::Align::Start)
                .build();
            sub_label.add_css_class("metis-cal-event-sub");
            text.append(&sub_label);
        }
        row.append(&text);

        let dismiss = gtk::Button::from_icon_name("window-close-symbolic");
        dismiss.add_css_class("metis-cal-event-action");
        dismiss.set_tooltip_text(Some(&metis_i18n::tr("Dismiss")));
        dismiss.set_valign(gtk::Align::Center);
        {
            let ev = ev.clone();
            let cb = self.cb.borrow().on_dismiss.clone();
            dismiss.connect_clicked(move |_| {
                if let Some(cb) = &cb {
                    cb(&ev);
                }
            });
        }
        row.append(&dismiss);

        if ev.can_delete {
            let delete = gtk::Button::from_icon_name("user-trash-symbolic");
            delete.add_css_class("metis-cal-event-action");
            delete.set_tooltip_text(Some(&metis_i18n::tr("Delete")));
            delete.set_valign(gtk::Align::Center);
            let ev = ev.clone();
            let cb = self.cb.borrow().on_delete.clone();
            delete.connect_clicked(move |_| {
                if let Some(cb) = &cb {
                    cb(&ev);
                }
            });
            row.append(&delete);
        }

        // Selecting a day is handled by the grid; rows are informational here.
        row.upcast()
    }

    fn select(self: &Rc<Self>, date: NaiveDate) {
        *self.selected.borrow_mut() = date;
        let anchor = *self.shown.borrow();
        if date.month() != anchor.month() || date.year() != anchor.year() {
            *self.shown.borrow_mut() = date.with_day(1).unwrap_or(date);
            self.rebuild();
            self.notify_month_change();
        } else {
            self.rebuild();
        }
    }
}

/// Build a one-off CSS provider that paints a widget's background a given color.
#[allow(deprecated)]
fn inline_color_css(color: &str) -> Result<gtk::CssProvider, ()> {
    let safe: String = color
        .chars()
        .filter(|c| {
            c.is_ascii_alphanumeric()
                || *c == '#'
                || *c == '('
                || *c == ')'
                || *c == ','
                || *c == '.'
                || *c == ' '
        })
        .collect();
    if safe.is_empty() {
        return Err(());
    }
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&format!(
        ".metis-cal-event-color {{ background-color: {safe}; }}"
    ));
    Ok(provider)
}
