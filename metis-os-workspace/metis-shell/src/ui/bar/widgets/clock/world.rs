use std::cell::RefCell;
use std::rc::Rc;

use chrono::{DateTime, Local, Offset, Utc};
use chrono_tz::Tz;
use gtk::prelude::*;

use super::Store;

const MAX_CLOCKS: usize = 3;

pub struct WorldClocksPage {
    pub widget: gtk::Widget,
    inner: Rc<WorldInner>,
}

struct WorldInner {
    store: Store,
    list: gtk::Box,
    picker: gtk::Box,
    picker_rows: gtk::ListBox,
    search: gtk::Entry,
    tz_names: Vec<String>,
    local_label: gtk::Label,
    times: RefCell<Vec<(Tz, gtk::Label)>>,
}

impl WorldClocksPage {
    pub fn new(store: Store) -> Self {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(10)
            .hexpand(true)
            .build();
        root.add_css_class("metis-world-page");
        root.set_width_request(-1);

        // ---- Local (main) clock ----
        let local_card = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(8)
            .build();
        local_card.add_css_class("metis-clock-card");
        local_card.add_css_class("metis-clock-card-main");
        let local_info = gtk::Box::new(gtk::Orientation::Vertical, 2);
        local_info.set_hexpand(true);
        let local_name = gtk::Label::builder()
            .label(metis_i18n::tr("Local Time"))
            .halign(gtk::Align::Start)
            .build();
        local_name.add_css_class("metis-clock-card-name");
        let local_offset = gtk::Label::builder()
            .label(Local::now().format("%A, %B %-d").to_string())
            .halign(gtk::Align::Start)
            .build();
        local_offset.add_css_class("metis-clock-card-offset");
        local_info.append(&local_name);
        local_info.append(&local_offset);
        local_card.append(&local_info);
        let local_label = gtk::Label::new(Some(&Local::now().format("%-I:%M %p").to_string()));
        local_label.add_css_class("metis-clock-card-time");
        local_label.add_css_class("metis-clock-card-time-main");
        local_label.set_valign(gtk::Align::Center);
        local_card.append(&local_label);
        root.append(&local_card);

        // ---- Add row: an "Add clock" button that toggles an inline picker ----
        let add_btn = gtk::Button::from_icon_name("list-add-symbolic");
        add_btn.add_css_class("metis-cal-add-btn");
        add_btn.set_tooltip_text(Some(&metis_i18n::tr("Add a world clock")));
        add_btn.set_halign(gtk::Align::End);
        root.append(&add_btn);

        // Inline picker (no nested popup): a search entry + a scrollable list of
        // every IANA timezone. Lives inside the popover so it can never render
        // behind it the way a GtkDropDown's own popup did.
        let picker = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        picker.add_css_class("metis-tz-picker");
        picker.set_visible(false);

        let search = gtk::Entry::builder()
            .placeholder_text(metis_i18n::tr("Search city / timezone…"))
            .primary_icon_name("system-search-symbolic")
            .build();
        picker.append(&search);

        let tz_names: Vec<String> = chrono_tz::TZ_VARIANTS
            .iter()
            .map(|tz| tz.name().to_string())
            .collect();
        let picker_rows = gtk::ListBox::new();
        picker_rows.set_selection_mode(gtk::SelectionMode::None);
        picker_rows.add_css_class("metis-tz-list");
        for name in &tz_names {
            let row = gtk::ListBoxRow::new();
            let label = gtk::Label::builder()
                .label(name.replace('_', " "))
                .halign(gtk::Align::Start)
                .build();
            label.add_css_class("metis-tz-row");
            row.set_child(Some(&label));
            picker_rows.append(&row);
        }
        let picker_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_height(220)
            .max_content_height(220)
            .overlay_scrolling(false)
            .child(&picker_rows)
            .build();
        picker_scroll.add_css_class("metis-tz-scroll");
        picker.append(&picker_scroll);
        root.append(&picker);

        // ---- Added clocks list ----
        let list = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .build();
        root.append(&list);

        let inner = Rc::new(WorldInner {
            store,
            list,
            picker: picker.clone(),
            picker_rows: picker_rows.clone(),
            search: search.clone(),
            tz_names,
            local_label,
            times: RefCell::new(Vec::new()),
        });

        {
            let inner = inner.clone();
            add_btn.connect_clicked(move |_| {
                if inner.store.borrow().world_clocks.len() >= MAX_CLOCKS {
                    return;
                }
                let show = !inner.picker.get_visible();
                inner.picker.set_visible(show);
                if show {
                    inner.search.set_text("");
                    inner.search.grab_focus();
                }
            });
        }
        {
            let inner = inner.clone();
            picker_rows.set_filter_func(move |row| inner.row_matches(row));
        }
        {
            let inner = inner.clone();
            search.connect_changed(move |_| inner.picker_rows.invalidate_filter());
        }
        {
            let inner = inner.clone();
            picker_rows.connect_row_activated(move |_, row| {
                let idx = row.index();
                if idx >= 0
                    && let Some(name) = inner.tz_names.get(idx as usize)
                {
                    inner.add_zone(&name.clone());
                }
            });
        }

        inner.rebuild();

        Self {
            widget: root.upcast(),
            inner,
        }
    }

    pub fn refresh(&self) {
        let now = Utc::now();
        self.inner
            .local_label
            .set_label(&Local::now().format("%-I:%M %p").to_string());
        for (tz, label) in self.inner.times.borrow().iter() {
            let local: DateTime<Tz> = now.with_timezone(tz);
            label.set_label(&local.format("%-I:%M %p").to_string());
        }
    }
}

impl WorldInner {
    fn row_matches(&self, row: &gtk::ListBoxRow) -> bool {
        let query = self.search.text().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let idx = row.index();
        if idx < 0 {
            return true;
        }
        self.tz_names
            .get(idx as usize)
            .map(|n| n.to_lowercase().replace('_', " ").contains(&query))
            .unwrap_or(false)
    }

    fn add_zone(self: &Rc<Self>, name: &str) {
        {
            let mut cfg = self.store.borrow_mut();
            if cfg.world_clocks.len() >= MAX_CLOCKS {
                return;
            }
            if !cfg.world_clocks.iter().any(|z| z == name) {
                cfg.world_clocks.push(name.to_string());
            }
        }
        self.store.save();
        self.picker.set_visible(false);
        self.rebuild();
    }

    fn remove(self: &Rc<Self>, zone: &str) {
        self.store.borrow_mut().world_clocks.retain(|z| z != zone);
        self.store.save();
        self.rebuild();
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        self.times.borrow_mut().clear();

        let zones: Vec<String> = self
            .store
            .borrow()
            .world_clocks
            .iter()
            .take(MAX_CLOCKS)
            .cloned()
            .collect();

        if zones.is_empty() {
            let empty = gtk::Label::builder()
                .label(metis_i18n::tr("Add up to 3 world clocks"))
                .halign(gtk::Align::Start)
                .build();
            empty.add_css_class("metis-cal-empty");
            self.list.append(&empty);
            return;
        }

        let now = Utc::now();
        for zone in zones {
            let Ok(tz) = zone.parse::<Tz>() else { continue };
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(8)
                .build();
            card.add_css_class("metis-clock-card");

            let info = gtk::Box::new(gtk::Orientation::Vertical, 2);
            info.set_hexpand(true);
            let name = gtk::Label::builder()
                .label(pretty_zone(&tz))
                .halign(gtk::Align::Start)
                .build();
            name.add_css_class("metis-clock-card-name");
            let offset = gtk::Label::builder()
                .label(offset_label(&tz, now))
                .halign(gtk::Align::Start)
                .build();
            offset.add_css_class("metis-clock-card-offset");
            info.append(&name);
            info.append(&offset);
            card.append(&info);

            let time = gtk::Label::new(Some(
                &now.with_timezone(&tz).format("%-I:%M %p").to_string(),
            ));
            time.add_css_class("metis-clock-card-time");
            time.set_valign(gtk::Align::Center);
            card.append(&time);

            let remove = gtk::Button::from_icon_name("window-close-symbolic");
            remove.add_css_class("metis-cal-event-action");
            remove.set_tooltip_text(Some(&metis_i18n::tr("Remove")));
            remove.set_valign(gtk::Align::Center);
            {
                let inner = self.clone();
                let zone = tz.name().to_string();
                remove.connect_clicked(move |_| inner.remove(&zone));
            }
            card.append(&remove);

            self.list.append(&card);
            self.times.borrow_mut().push((tz, time));
        }
    }
}

fn pretty_zone(tz: &Tz) -> String {
    tz.name()
        .rsplit('/')
        .next()
        .unwrap_or(tz.name())
        .replace('_', " ")
}

fn offset_label(tz: &Tz, now: DateTime<Utc>) -> String {
    let tz_secs = now.with_timezone(tz).offset().fix().local_minus_utc();
    let local_secs = Local::now().offset().fix().local_minus_utc();
    let diff = tz_secs - local_secs;
    let sign = if diff < 0 { "-" } else { "+" };
    let abs = diff.abs();
    let h = abs / 3600;
    let m = (abs % 3600) / 60;
    if diff == 0 {
        metis_i18n::tr("Same as local")
    } else if m == 0 {
        metis_i18n::tr("%1 from local").replace("%1", &format!("{sign}{h}h"))
    } else {
        metis_i18n::tr("%1 from local").replace("%1", &format!("{sign}{h}h{m:02}"))
    }
}
