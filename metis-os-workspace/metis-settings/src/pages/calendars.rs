//! Calendars: add/remove CalDAV, Thunderbird, Microsoft 365, and local accounts.
//! Writes `calendars.json` and stores CalDAV passwords / MS refresh tokens in the
//! Secret Service (via `metis-secrets`); nudges the shell with `reload-calendars`.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;

use metis_config::{AccountKind, CalendarAccount, CalendarsConfig};

use crate::dialog;
use crate::{msauth, runtime, ui};
use metis_i18n::tr;

struct Inner {
    config: RefCell<CalendarsConfig>,
    list: gtk::Box,
}

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("calendars");

    let (list_card, list_body) = ui::section(&tr("Accounts"));
    let list = gtk::Box::new(gtk::Orientation::Vertical, 8);
    list.add_css_class("metis-settings-inset");
    list_body.append(&list);
    content.append(&list_card);

    let inner = Rc::new(Inner {
        config: RefCell::new(metis_config::load_calendars_config()),
        list,
    });

    let add_btn = gtk::Button::with_label(&tr("Add account…"));
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk::Align::Start);
    add_btn.set_margin_top(4);
    {
        let inner = inner.clone();
        add_btn.connect_clicked(move |_| show_add_account_sheet(inner.clone()));
    }
    list_body.append(&add_btn);

    inner.rebuild();

    scroller.upcast()
}

/// Top-slide: pick a provider, fill fields, add the account.
fn show_add_account_sheet(inner: Rc<Inner>) {
    if dialog::is_open() {
        return;
    }

    let wrap = gtk::Box::new(gtk::Orientation::Vertical, 10);
    wrap.add_css_class("metis-settings-top-sheet-body");

    let intro = gtk::Label::new(Some(&tr(
        "Choose what to connect, then fill in the details for that provider.",
    )));
    intro.set_xalign(0.0);
    intro.set_wrap(true);
    intro.add_css_class("metis-settings-top-dialog-body");
    wrap.append(&intro);

    let kind_labels = [
        tr("CalDAV"),
        tr("Thunderbird"),
        tr("Microsoft 365"),
        tr("Local"),
    ];
    let kind_refs: Vec<&str> = kind_labels.iter().map(|s| s.as_str()).collect();
    let kind_dd = gtk::DropDown::from_strings(&kind_refs);
    wrap.append(&ui::row(&tr("Connect to"), &kind_dd));

    let name = entry(&tr("Display name"));
    let url = entry(&tr("CalDAV URL (server, calendar, or principal)"));
    let username = entry(&tr("Username"));
    let tenant = entry(&tr("MS tenant (e.g. common or your tenant id)"));
    let client_id = entry(&tr("MS application (client) id"));
    let color = entry(&tr("Color (e.g. #22d3ee) — optional"));
    for e in [&name, &url, &username, &tenant, &client_id, &color] {
        wrap.append(e);
    }

    let err = gtk::Label::new(None);
    err.set_xalign(0.0);
    err.set_wrap(true);
    err.add_css_class("metis-settings-error");
    err.set_visible(false);
    wrap.append(&err);

    let update_visibility = {
        let url = url.clone();
        let username = username.clone();
        let tenant = tenant.clone();
        let client_id = client_id.clone();
        move |idx: u32| {
            let is_caldav = idx == 0;
            let is_ms = idx == 2;
            url.set_visible(is_caldav);
            username.set_visible(is_caldav);
            tenant.set_visible(is_ms);
            client_id.set_visible(is_ms);
        }
    };
    update_visibility(kind_dd.selected());
    {
        let update_visibility = update_visibility.clone();
        kind_dd.connect_selected_notify(move |dd| update_visibility(dd.selected()));
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let add_btn = gtk::Button::with_label(&tr("Add account"));
    add_btn.add_css_class("suggested-action");
    actions.append(&add_btn);
    wrap.append(&actions);

    {
        let inner = inner.clone();
        let err = err.clone();
        let name = name.clone();
        let url = url.clone();
        let username = username.clone();
        let tenant = tenant.clone();
        let client_id = client_id.clone();
        let color = color.clone();
        let kind_dd = kind_dd.clone();
        add_btn.connect_clicked(move |_| {
            let kind = match kind_dd.selected() {
                0 => AccountKind::Caldav,
                1 => AccountKind::Thunderbird,
                2 => AccountKind::Ms365,
                _ => AccountKind::Local,
            };
            if matches!(kind, AccountKind::Caldav) && opt(&url).is_none() {
                err.set_text(&tr("Enter a CalDAV URL."));
                err.set_visible(true);
                return;
            }
            if matches!(kind, AccountKind::Ms365) && opt(&client_id).is_none() {
                err.set_text(&tr("Enter the Microsoft application (client) id."));
                err.set_visible(true);
                return;
            }
            err.set_visible(false);

            let display = name.text().trim().to_string();
            let display = if display.is_empty() {
                match kind {
                    AccountKind::Caldav => tr("CalDAV"),
                    AccountKind::Thunderbird => tr("Thunderbird"),
                    AccountKind::Ms365 => tr("Microsoft 365"),
                    AccountKind::Local => tr("Local"),
                }
            } else {
                display
            };
            let account = CalendarAccount {
                id: new_account_id(&display),
                kind,
                name: display,
                url: opt(&url),
                username: opt(&username),
                tenant: opt(&tenant),
                client_id: opt(&client_id),
                color: opt(&color),
                enabled: true,
                read_only: false,
            };
            inner.add_account(account);
            dialog::dismiss_silent();
        });
    }

    let _ = dialog::present(&tr("Add account"), &wrap, Rc::new(|| {}));
    let focus = name.clone();
    glib::idle_add_local_once(move || {
        focus.grab_focus();
    });
}

impl Inner {
    fn persist_and_reload(&self) {
        if let Err(err) = metis_config::save_calendars_config(&self.config.borrow()) {
            tracing::warn!(%err, "failed to save calendars.json");
        }
        runtime::send("reload-calendars");
    }

    fn add_account(self: &Rc<Self>, account: CalendarAccount) {
        self.config.borrow_mut().accounts.push(account);
        self.persist_and_reload();
        self.rebuild();
    }

    fn remove_account(self: &Rc<Self>, id: &str) {
        let id_owned = id.to_string();
        self.config.borrow_mut().accounts.retain(|a| a.id != id);
        self.persist_and_reload();
        self.rebuild();
        // Best-effort keyring cleanup (CalDAV password + M365 refresh token).
        run_async(async move {
            let _ = metis_secrets::delete(&id_owned, metis_secrets::CALDAV_PASSWORD).await;
            let _ = metis_secrets::delete(&id_owned, metis_secrets::MS_REFRESH_TOKEN).await;
        });
    }

    fn set_enabled(self: &Rc<Self>, id: &str, enabled: bool) {
        if let Some(a) = self
            .config
            .borrow_mut()
            .accounts
            .iter_mut()
            .find(|a| a.id == id)
        {
            a.enabled = enabled;
        }
        self.persist_and_reload();
    }

    fn rebuild(self: &Rc<Self>) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        let accounts = self.config.borrow().accounts.clone();
        if accounts.is_empty() {
            let empty = gtk::Label::new(Some(&tr("No calendar accounts.")));
            empty.set_xalign(0.0);
            empty.add_css_class("metis-settings-hint");
            self.list.append(&empty);
            return;
        }
        for account in accounts {
            self.list.append(&self.build_row(&account));
        }
    }

    fn build_row(self: &Rc<Self>, account: &CalendarAccount) -> gtk::Widget {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 6);
        row.add_css_class("metis-settings-list");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let label = gtk::Label::new(Some(&format!("{}  ·  {:?}", account.name, account.kind)));
        label.set_xalign(0.0);
        label.set_hexpand(true);
        header.append(&label);

        let toggle = gtk::Switch::new();
        toggle.set_active(account.enabled);
        toggle.set_valign(gtk::Align::Center);
        {
            let inner = self.clone();
            let id = account.id.clone();
            toggle.connect_state_set(move |_, state| {
                inner.set_enabled(&id, state);
                glib::Propagation::Proceed
            });
        }
        header.append(&toggle);

        let remove = gtk::Button::from_icon_name("user-trash-symbolic");
        remove.set_valign(gtk::Align::Center);
        {
            let inner = self.clone();
            let id = account.id.clone();
            remove.connect_clicked(move |_| inner.remove_account(&id));
        }
        header.append(&remove);
        row.append(&header);

        match account.kind {
            AccountKind::Caldav => row.append(&caldav_password_row(account)),
            AccountKind::Ms365 => row.append(&ms_login_row(account)),
            _ => {}
        }

        row.upcast()
    }
}

fn caldav_password_row(account: &CalendarAccount) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 2);
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let entry = gtk::PasswordEntry::builder()
        .hexpand(true)
        .show_peek_icon(true)
        .build();
    let save = gtk::Button::with_label(&tr("Save password"));
    let status = gtk::Label::new(None);
    status.add_css_class("metis-settings-hint");
    status.set_xalign(0.0);
    row.append(&entry);
    row.append(&save);

    {
        let id = account.id.clone();
        let entry = entry.clone();
        let status = status.clone();
        save.connect_clicked(move |_| {
            let pw = entry.text().to_string();
            if pw.is_empty() {
                return;
            }
            let id = id.clone();
            let shared = Arc::new(Mutex::new(String::new()));
            run_async({
                let shared = shared.clone();
                async move {
                    let msg = match metis_secrets::store(&id, metis_secrets::CALDAV_PASSWORD, &pw)
                        .await
                    {
                        Ok(()) => "Password saved".to_string(),
                        Err(e) => format!("Failed: {e}"),
                    };
                    if let Ok(mut g) = shared.lock() {
                        *g = msg;
                    }
                }
            });
            // Never `tr("")` — gettext maps empty msgid to the PO file header.
            entry.set_text("");
            poll_status(&status, shared, || runtime::send("reload-calendars"));
        });
    }

    outer.append(&row);
    outer.append(&status);
    outer.upcast()
}

fn ms_login_row(account: &CalendarAccount) -> gtk::Widget {
    let outer = gtk::Box::new(gtk::Orientation::Vertical, 4);
    let sign_in = gtk::Button::with_label(&tr("Sign in with Microsoft"));
    sign_in.set_halign(gtk::Align::Start);
    let status = gtk::Label::new(None);
    status.add_css_class("metis-settings-hint");
    status.set_wrap(true);
    status.set_xalign(0.0);
    outer.append(&sign_in);
    outer.append(&status);

    let tenant = account.tenant.clone().unwrap_or_else(|| "common".into());
    let client_id = account.client_id.clone().unwrap_or_default();
    let id = account.id.clone();
    {
        let status = status.clone();
        sign_in.connect_clicked(move |btn| {
            if client_id.is_empty() {
                status.set_label(&tr("Set the application (client) id first."));
                return;
            }
            btn.set_sensitive(false);
            let shared = Arc::new(Mutex::new("Starting sign-in…".to_string()));
            run_async({
                let shared = shared.clone();
                let tenant = tenant.clone();
                let client_id = client_id.clone();
                let id = id.clone();
                async move {
                    let msg = match msauth::start_device_login(&tenant, &client_id).await {
                        Ok(code) => {
                            if let Ok(mut g) = shared.lock() {
                                *g = code.message.clone();
                            }
                            match msauth::complete_device_login(&id, &tenant, &client_id, &code)
                                .await
                            {
                                Ok(()) => "Signed in.".to_string(),
                                Err(e) => format!("Sign-in failed: {e}"),
                            }
                        }
                        Err(e) => format!("Could not start sign-in: {e}"),
                    };
                    if let Ok(mut g) = shared.lock() {
                        *g = msg;
                    }
                }
            });
            let btn = btn.clone();
            poll_status(&status, shared, move || {
                btn.set_sensitive(true);
                runtime::send("reload-calendars");
            });
        });
    }

    outer.upcast()
}

fn entry(placeholder: &str) -> gtk::Entry {
    let e = gtk::Entry::builder().placeholder_text(placeholder).build();
    ui::swallow_empty_backspace(&e);
    e
}

fn opt(entry: &gtk::Entry) -> Option<String> {
    let t = entry.text().trim().to_string();
    (!t.is_empty()).then_some(t)
}

fn new_account_id(name: &str) -> String {
    let slug: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{}-{n}", slug.trim_matches('-'))
}

/// Run a future to completion on a throwaway tokio runtime off the GTK thread.
fn run_async<F>(fut: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    std::thread::spawn(move || {
        if let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            rt.block_on(fut);
        }
    });
}

/// Mirror a worker thread's status string into a label until it stabilizes, then
/// fire `on_done` once.
fn poll_status<F: Fn() + 'static>(label: &gtk::Label, shared: Arc<Mutex<String>>, on_done: F) {
    let label = label.clone();
    let last = Rc::new(RefCell::new(String::new()));
    let stable_ticks = Rc::new(std::cell::Cell::new(0u32));
    let done = Rc::new(std::cell::Cell::new(false));
    glib::timeout_add_local(std::time::Duration::from_millis(600), move || {
        let current = shared.lock().map(|g| g.clone()).unwrap_or_default();
        if !current.is_empty() && current != *last.borrow() {
            label.set_label(&current);
            *last.borrow_mut() = current.clone();
            stable_ticks.set(0);
        } else {
            stable_ticks.set(stable_ticks.get() + 1);
        }
        let terminal = current.starts_with("Signed in")
            || current.starts_with("Password saved")
            || current.starts_with("Failed")
            || current.contains("failed");
        if terminal && stable_ticks.get() >= 1 && !done.get() {
            done.set(true);
            on_done();
            return glib::ControlFlow::Break;
        }
        glib::ControlFlow::Continue
    });
}
