use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::prelude::*;

use crate::config::{ALARM_SOUNDS, Alarm};

use super::Store;

pub struct AlarmsPage {
    pub widget: gtk::Widget,
}

struct Inner {
    store: Store,
    list: gtk::Box,
    // Add-form state.
    revealer: gtk::Revealer,
    hour12: Cell<u32>, // 1..=12
    minute: Cell<u32>, // 0..=59
    pm: Cell<bool>,
    hour_lbl: gtk::Label,
    minute_lbl: gtk::Label,
    ampm_btn: gtk::Button,
    day_btns: RefCell<Vec<(u8, gtk::ToggleButton)>>,
    name_entry: gtk::Entry,
    sound_sel: Cell<usize>,
}

impl AlarmsPage {
    pub fn new(store: Store) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .hexpand(true)
            .build();
        root.add_css_class("metis-alarm-page");
        root.set_width_request(-1);

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        let title = gtk::Label::builder()
            .label(metis_i18n::tr("Alarms"))
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        title.add_css_class("metis-bar-section-title");
        let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
        add_btn.add_css_class("metis-cal-add-btn");
        add_btn.set_tooltip_text(Some(&metis_i18n::tr("New alarm")));
        header.append(&title);
        header.append(&add_btn);
        root.append(&header);

        // ---- Add form (revealer) ----
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideDown)
            .reveal_child(false)
            .build();
        let form = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .build();
        form.add_css_class("metis-alarm-form");

        // Time steppers + AM/PM.
        let time_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::Center)
            .build();
        let (hour_col, hour_lbl) = stepper("12");
        let (minute_col, minute_lbl) = stepper("00");
        let ampm_btn = gtk::Button::with_label(&metis_i18n::tr("AM"));
        ampm_btn.add_css_class("metis-alarm-ampm");
        ampm_btn.set_valign(gtk::Align::Center);
        time_row.append(&hour_col);
        time_row.append(&colon());
        time_row.append(&minute_col);
        time_row.append(&ampm_btn);
        form.append(&time_row);

        // Repeat days.
        let repeat = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        repeat.add_css_class("metis-alarm-section");
        repeat.append(&caption(&metis_i18n::tr("Repeat")));
        let days_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .halign(gtk::Align::Center)
            .build();
        let mut day_btns = Vec::new();
        for (idx, label) in [
            metis_i18n::tr("S"),
            metis_i18n::tr("M"),
            metis_i18n::tr("T"),
            metis_i18n::tr("W"),
            metis_i18n::tr("T"),
            metis_i18n::tr("F"),
            metis_i18n::tr("S"),
        ]
        .iter()
        .enumerate()
        {
            let b = gtk::ToggleButton::with_label(label);
            b.add_css_class("metis-alarm-day");
            days_row.append(&b);
            day_btns.push((idx as u8, b));
        }
        repeat.append(&days_row);
        form.append(&repeat);

        // Name.
        let name_entry = gtk::Entry::builder()
            .placeholder_text(metis_i18n::tr("Name"))
            .build();
        form.append(&name_entry);

        // Sound: a segmented row of linked toggles (no nested popup like a
        // GtkDropDown, which renders behind the popover on our compositor).
        let sound_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .build();
        sound_box.append(&caption(&metis_i18n::tr("Sound")));
        let sound_seg = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(0)
            .build();
        sound_seg.add_css_class("metis-alarm-sound-seg");
        sound_seg.add_css_class("linked");
        let mut sound_btns: Vec<gtk::ToggleButton> = Vec::new();
        let mut sound_group: Option<gtk::ToggleButton> = None;
        for (i, s) in ALARM_SOUNDS.iter().enumerate() {
            let b = gtk::ToggleButton::with_label(&metis_i18n::tr(s.label));
            b.add_css_class("metis-alarm-sound-btn");
            b.set_hexpand(true);
            if let Some(ref leader) = sound_group {
                b.set_group(Some(leader));
            } else {
                sound_group = Some(b.clone());
            }
            if i == 0 {
                b.set_active(true);
            }
            sound_seg.append(&b);
            sound_btns.push(b);
        }
        sound_box.append(&sound_seg);
        form.append(&sound_box);

        // Actions.
        let actions = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .halign(gtk::Align::End)
            .build();
        let cancel_btn = gtk::Button::with_label(&metis_i18n::tr("Cancel"));
        cancel_btn.add_css_class("metis-clock-btn");
        let save_btn = gtk::Button::with_label(&metis_i18n::tr("Add"));
        save_btn.add_css_class("metis-cal-add-btn");
        actions.append(&cancel_btn);
        actions.append(&save_btn);
        form.append(&actions);

        revealer.set_child(Some(&form));
        root.append(&revealer);

        // ---- Alarm list ----
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        root.append(&list);

        let inner = Rc::new(Inner {
            store,
            list,
            revealer: revealer.clone(),
            hour12: Cell::new(12),
            minute: Cell::new(0),
            pm: Cell::new(false),
            hour_lbl,
            minute_lbl,
            ampm_btn: ampm_btn.clone(),
            day_btns: RefCell::new(day_btns),
            name_entry: name_entry.clone(),
            sound_sel: Cell::new(0),
        });

        for (i, b) in sound_btns.iter().enumerate() {
            let inner = inner.clone();
            b.connect_toggled(move |b| {
                if b.is_active() {
                    inner.sound_sel.set(i);
                }
            });
        }

        wire_stepper(&inner, &hour_col, true);
        wire_stepper(&inner, &minute_col, false);
        {
            let inner = inner.clone();
            ampm_btn.connect_clicked(move |_| {
                inner.pm.set(!inner.pm.get());
                let label = if inner.pm.get() {
                    metis_i18n::tr("PM")
                } else {
                    metis_i18n::tr("AM")
                };
                inner.ampm_btn.set_label(&label);
            });
        }
        {
            let inner = inner.clone();
            add_btn.connect_clicked(move |_| {
                let show = !inner.revealer.reveals_child();
                inner.revealer.set_reveal_child(show);
            });
        }
        {
            let revealer = revealer.clone();
            cancel_btn.connect_clicked(move |_| revealer.set_reveal_child(false));
        }
        {
            let inner = inner.clone();
            save_btn.connect_clicked(move |_| inner.save());
        }

        inner.rebuild();

        Self {
            widget: root.upcast(),
        }
    }
}

impl Inner {
    fn save(self: &Rc<Self>) {
        let hour12 = self.hour12.get();
        let hour24 = match (hour12 % 12, self.pm.get()) {
            (h, false) => h,     // 12 AM -> 0
            (h, true) => h + 12, // 12 PM -> 12
        };
        let days: Vec<u8> = self
            .day_btns
            .borrow()
            .iter()
            .filter(|(_, b)| b.is_active())
            .map(|(d, _)| *d)
            .collect();
        let sound = ALARM_SOUNDS
            .get(self.sound_sel.get())
            .map(|s| s.id.to_string());
        let alarm = Alarm {
            id: new_id(),
            hour: hour24 as u8,
            minute: self.minute.get() as u8,
            label: self.name_entry.text().trim().to_string(),
            enabled: true,
            days,
            sound,
        };
        self.store.borrow_mut().alarms.push(alarm);
        self.store.save();
        // Reset the form.
        self.name_entry.set_text("");
        for (_, b) in self.day_btns.borrow().iter() {
            b.set_active(false);
        }
        self.revealer.set_reveal_child(false);
        self.rebuild();
    }

    fn bump_hour(&self, up: bool) {
        let v = self.hour12.get();
        let next = if up {
            if v >= 12 { 1 } else { v + 1 }
        } else if v <= 1 {
            12
        } else {
            v - 1
        };
        self.hour12.set(next);
        self.hour_lbl.set_label(&format!("{next:02}"));
    }

    fn bump_minute(&self, up: bool) {
        let v = self.minute.get();
        let next = if up { (v + 1) % 60 } else { (v + 59) % 60 };
        self.minute.set(next);
        self.minute_lbl.set_label(&format!("{next:02}"));
    }

    fn remove(self: &Rc<Self>, id: &str) {
        self.store.borrow_mut().alarms.retain(|a| a.id != id);
        self.store.save();
        self.rebuild();
    }

    fn set_enabled(&self, id: &str, enabled: bool) {
        if let Some(a) = self
            .store
            .borrow_mut()
            .alarms
            .iter_mut()
            .find(|a| a.id == id)
        {
            a.enabled = enabled;
        }
        self.store.save();
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let alarms = self.store.borrow().alarms.clone();
        if alarms.is_empty() {
            let empty = gtk::Label::builder()
                .label(metis_i18n::tr("No alarms"))
                .build();
            empty.add_css_class("metis-cal-empty");
            self.list.append(&empty);
            return;
        }
        for alarm in alarms {
            self.list.append(&self.build_row(&alarm));
        }
    }

    fn build_row(self: &Rc<Self>, alarm: &Alarm) -> gtk::Widget {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(10)
            .build();
        row.add_css_class("metis-clock-alarm");

        let info = gtk::Box::new(gtk::Orientation::Vertical, 1);
        info.set_hexpand(true);
        let time = gtk::Label::builder()
            .label(fmt_time(alarm.hour, alarm.minute))
            .halign(gtk::Align::Start)
            .build();
        time.add_css_class("metis-clock-alarm-time");
        info.append(&time);
        let sub = subtitle(alarm);
        if !sub.is_empty() {
            let sub_lbl = gtk::Label::builder()
                .label(&sub)
                .halign(gtk::Align::Start)
                .build();
            sub_lbl.add_css_class("metis-cal-event-sub");
            info.append(&sub_lbl);
        }
        row.append(&info);

        let toggle = gtk::Switch::new();
        toggle.set_active(alarm.enabled);
        toggle.set_valign(gtk::Align::Center);
        {
            let inner = self.clone();
            let id = alarm.id.clone();
            toggle.connect_state_set(move |_, state| {
                inner.set_enabled(&id, state);
                glib::Propagation::Proceed
            });
        }
        row.append(&toggle);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.add_css_class("metis-cal-event-action");
        remove.set_valign(gtk::Align::Center);
        {
            let inner = self.clone();
            let id = alarm.id.clone();
            remove.connect_clicked(move |_| inner.remove(&id));
        }
        row.append(&remove);

        row.upcast()
    }
}

fn subtitle(alarm: &Alarm) -> String {
    if alarm.days.is_empty() {
        if alarm.label.is_empty() {
            metis_i18n::tr("Every day")
        } else {
            metis_i18n::tr("Every day  ·  %1").replace("%1", &alarm.label)
        }
    } else {
        let names = [
            metis_i18n::tr("Sun"),
            metis_i18n::tr("Mon"),
            metis_i18n::tr("Tue"),
            metis_i18n::tr("Wed"),
            metis_i18n::tr("Thu"),
            metis_i18n::tr("Fri"),
            metis_i18n::tr("Sat"),
        ];
        let mut sorted = alarm.days.clone();
        sorted.sort_unstable();
        let days = sorted
            .iter()
            .filter_map(|d| names.get(*d as usize).map(String::as_str))
            .collect::<Vec<_>>()
            .join(", ");
        if alarm.label.is_empty() {
            days
        } else {
            format!("{days}  ·  {}", alarm.label)
        }
    }
}

fn fmt_time(hour: u8, minute: u8) -> String {
    let (h12, pm) = match hour {
        0 => (12, false),
        1..=11 => (hour, false),
        12 => (12, true),
        _ => (hour - 12, true),
    };
    let suffix = if pm {
        metis_i18n::tr("PM")
    } else {
        metis_i18n::tr("AM")
    };
    format!("{h12:02}:{minute:02} {suffix}")
}

fn wire_stepper(inner: &Rc<Inner>, col: &gtk::Box, is_hour: bool) {
    let up = col.first_child().and_downcast::<gtk::Button>();
    let down = col.last_child().and_downcast::<gtk::Button>();
    if let Some(up) = up {
        let inner = inner.clone();
        up.connect_clicked(move |_| {
            if is_hour {
                inner.bump_hour(true)
            } else {
                inner.bump_minute(true)
            }
        });
    }
    if let Some(down) = down {
        let inner = inner.clone();
        down.connect_clicked(move |_| {
            if is_hour {
                inner.bump_hour(false)
            } else {
                inner.bump_minute(false)
            }
        });
    }
}

fn stepper(initial: &str) -> (gtk::Box, gtk::Label) {
    let col = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .build();
    col.add_css_class("metis-timer-stepper");
    let up = gtk::Button::from_icon_name("list-add-symbolic");
    up.add_css_class("metis-timer-step-btn");
    let value = gtk::Label::new(Some(initial));
    value.add_css_class("metis-timer-step-value");
    let down = gtk::Button::from_icon_name("list-remove-symbolic");
    down.add_css_class("metis-timer-step-btn");
    col.append(&up);
    col.append(&value);
    col.append(&down);
    (col, value)
}

fn colon() -> gtk::Label {
    let l = gtk::Label::new(Some(":"));
    l.add_css_class("metis-timer-colon");
    l.set_valign(gtk::Align::Center);
    l
}

fn caption(text: &str) -> gtk::Label {
    let l = gtk::Label::builder()
        .label(text)
        .halign(gtk::Align::Start)
        .build();
    l.add_css_class("metis-alarm-caption");
    l
}

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("alarm-{nanos}")
}
