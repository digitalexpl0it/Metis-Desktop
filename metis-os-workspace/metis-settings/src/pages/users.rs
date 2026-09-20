//! Settings → System → Users: profile, password, and local account admin.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk::gio;
use gtk::prelude::*;
use metis_i18n::tr;
use metis_remote::AccountInfo;
use zeroize::Zeroize;

use crate::{bg, dialog, runtime, ui};

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("users");
    let accounts: Rc<RefCell<Vec<AccountInfo>>> = Rc::new(RefCell::new(Vec::new()));
    let list_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list_box.add_css_class("metis-settings-list");
    list_box.add_css_class("metis-settings-user-list");

    // ---- Your profile -----------------------------------------------------
    let (profile_card, profile_body) =
        ui::section_with_icon(&tr("Your profile"), "avatar-default-symbolic");

    let (avatar_frame, avatar) = circular_avatar(64, "metis-settings-avatar");
    refresh_avatar(&avatar);

    let change_pic = gtk::Button::with_label(&tr("Change picture…"));
    change_pic.add_css_class("flat");
    {
        let avatar = avatar.clone();
        change_pic.connect_clicked(move |btn| {
            let Some(parent) = btn.root().and_downcast::<gtk::Window>() else {
                return;
            };
            let dialog = gtk::FileDialog::builder()
                .title(tr("Choose profile picture"))
                .modal(true)
                .build();
            let filter = gtk::FileFilter::new();
            filter.set_name(Some(&tr("Images")));
            filter.add_mime_type("image/png");
            filter.add_mime_type("image/jpeg");
            filter.add_mime_type("image/webp");
            let filters = gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            dialog.set_filters(Some(&filters));
            let avatar = avatar.clone();
            dialog.open(Some(&parent), gio::Cancellable::NONE, move |res| {
                let Ok(file) = res else { return };
                let Some(src) = file.path() else { return };
                if let Err(err) = install_face_image(&src) {
                    tracing::warn!(%err, "failed to set profile picture");
                    return;
                }
                let mut menu = metis_config::load_menu_config();
                menu.avatar_path = Some(face_path().display().to_string());
                // Ensure the menu header is shown so the new picture is visible.
                menu.show_user_header = true;
                let _ = metis_config::save_menu_config(&menu);
                refresh_avatar(&avatar);
                // Menu header is built at bar install time — remount so it reloads
                // ~/.face / menu.json.
                runtime::send("reload-bar");
            });
        });
    }

    let avatar_row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    avatar_row.add_css_class("metis-settings-row");
    avatar_row.append(&avatar_frame);
    let avatar_col = gtk::Box::new(gtk::Orientation::Vertical, 6);
    avatar_col.set_valign(gtk::Align::Center);
    avatar_col.append(&change_pic);
    avatar_row.append(&avatar_col);
    profile_body.append(&avatar_row);

    let name_entry = gtk::Entry::new();
    name_entry.set_hexpand(true);
    ui::swallow_empty_backspace(&name_entry);
    let me = metis_config::load_menu_config().resolved_display_name();
    name_entry.set_text(&me);
    profile_body.append(&ui::row_with_icon(
        "avatar-default-symbolic",
        &tr("Display name"),
        &name_entry,
    ));

    let save_name = gtk::Button::with_label(&tr("Save name"));
    save_name.add_css_class("suggested-action");
    {
        let name_entry = name_entry.clone();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        save_name.connect_clicked(move |_| {
            let name = name_entry.text().to_string();
            let user = std::env::var("USER").unwrap_or_default();
            if user.is_empty() {
                return;
            }
            let name_c = name.clone();
            let accounts = accounts.clone();
            let list_box = list_box.clone();
            bg::run_bg(move || metis_remote::set_display_name(&user, &name_c), {
                let name_c = name.clone();
                let name_entry = name_entry.clone();
                move |result: Result<(), String>| match result {
                    Ok(()) => {
                        let mut menu = metis_config::load_menu_config();
                        menu.user_display_name = Some(name_c);
                        menu.show_user_header = true;
                        let _ = metis_config::save_menu_config(&menu);
                        refresh_account_list(&accounts, &list_box);
                        runtime::send("reload-bar");
                    }
                    Err(err) => {
                        tracing::warn!(%err, "failed to set display name");
                        name_entry.set_placeholder_text(Some(err.as_str()));
                    }
                }
            });
        });
    }

    let change_pw = gtk::Button::with_label(&tr("Change password…"));
    change_pw.add_css_class("flat");
    {
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        change_pw.connect_clicked(move |_| {
            let user = std::env::var("USER").unwrap_or_default();
            if !user.is_empty() {
                open_password_sheet(&user, accounts.clone(), list_box.clone());
            }
        });
    }

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    actions.add_css_class("metis-settings-actions");
    actions.append(&change_pw);
    actions.append(&save_name);
    profile_body.append(&actions);
    content.append(&profile_card);

    // ---- Other users ------------------------------------------------------
    let (users_card, users_body) =
        ui::section_with_icon(&tr("Other users"), "system-users-symbolic");
    users_body.append(&list_box);

    let add_btn = gtk::Button::with_label(&tr("Add User…"));
    add_btn.add_css_class("suggested-action");
    {
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        add_btn.connect_clicked(move |_| {
            open_add_user_sheet(accounts.clone(), list_box.clone());
        });
    }
    let add_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    add_row.set_halign(gtk::Align::End);
    add_row.set_margin_top(8);
    add_row.append(&add_btn);
    users_body.append(&add_row);

    let hint = ui::hint(&tr(
        "Administrator access adds the account to the sudo group. Creating, \
         removing, or changing passwords may show a PolicyKit password dialog.",
    ));
    users_body.append(&hint);
    content.append(&users_card);

    refresh_account_list(&accounts, &list_box);
    scroller.upcast()
}

fn face_path() -> PathBuf {
    std::env::var_os("HOME")
        .map(|h| PathBuf::from(h).join(".face"))
        .unwrap_or_else(|| PathBuf::from(".face"))
}

fn circular_avatar(size: i32, css_class: &str) -> (gtk::Box, gtk::Picture) {
    let frame = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    frame.add_css_class(css_class);
    frame.set_size_request(size, size);
    frame.set_hexpand(false);
    frame.set_vexpand(false);
    frame.set_halign(gtk::Align::Center);
    frame.set_valign(gtk::Align::Center);
    // Required for border-radius to actually clip the photo.
    frame.set_overflow(gtk::Overflow::Hidden);

    let pic = gtk::Picture::new();
    pic.set_content_fit(gtk::ContentFit::Cover);
    pic.set_can_shrink(true);
    pic.set_size_request(size, size);
    pic.set_hexpand(true);
    pic.set_vexpand(true);
    frame.append(&pic);
    (frame, pic)
}

fn set_avatar_picture(pic: &gtk::Picture, path: Option<&std::path::Path>) {
    if let Some(path) = path.filter(|p| p.is_file()) {
        pic.set_file(Some(&gio::File::for_path(path)));
        return;
    }
    pic.set_file(Option::<&gio::File>::None);
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let theme = gtk::IconTheme::for_display(&display);
    let icon = theme.lookup_icon(
        "avatar-default-symbolic",
        &[],
        64,
        1,
        gtk::TextDirection::None,
        gtk::IconLookupFlags::FORCE_SYMBOLIC,
    );
    pic.set_paintable(Some(&icon));
}

fn refresh_avatar(pic: &gtk::Picture) {
    let path = metis_config::load_menu_config().resolved_avatar_path();
    set_avatar_picture(pic, path.as_deref());
}

fn install_face_image(src: &std::path::Path) -> std::io::Result<()> {
    let dest = face_path();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(src, &dest)?;
    // Keep the file user-readable (and avoid a root-owned leftover if Settings
    // was ever run elevated).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

fn refresh_account_list(accounts: &Rc<RefCell<Vec<AccountInfo>>>, list_box: &gtk::Box) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }
    match metis_remote::list_accounts() {
        Ok(list) => {
            *accounts.borrow_mut() = list;
        }
        Err(err) => {
            tracing::warn!(%err, "failed to list accounts");
            let err_lbl = gtk::Label::new(Some(&tr("Could not list local users.")));
            err_lbl.add_css_class("metis-settings-hint");
            err_lbl.set_xalign(0.0);
            list_box.append(&err_lbl);
            return;
        }
    }
    let rows = accounts.borrow().clone();
    if rows.is_empty() {
        let empty = gtk::Label::new(Some(&tr("No local users found.")));
        empty.add_css_class("metis-settings-hint");
        empty.set_xalign(0.0);
        list_box.append(&empty);
        return;
    }
    for (i, acct) in rows.iter().enumerate() {
        let row = build_user_row(acct, accounts.clone(), list_box.clone());
        if i > 0 {
            list_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        }
        list_box.append(&row);
    }
}

fn load_account_avatar(pic: &gtk::Picture, acct: &AccountInfo) {
    let path = if acct.is_current {
        metis_config::load_menu_config().resolved_avatar_path()
    } else {
        metis_config::resolve_user_avatar_path(&acct.username, &acct.home)
    };
    set_avatar_picture(pic, path.as_deref());
}

fn build_user_row(
    acct: &AccountInfo,
    accounts: Rc<RefCell<Vec<AccountInfo>>>,
    list_box: gtk::Box,
) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 14);
    row.add_css_class("metis-settings-user-row");
    row.set_hexpand(true);

    let (avatar_frame, avatar) = circular_avatar(44, "metis-settings-user-avatar");
    load_account_avatar(&avatar, acct);
    row.append(&avatar_frame);

    let text = gtk::Box::new(gtk::Orientation::Vertical, 3);
    text.set_hexpand(true);
    text.set_valign(gtk::Align::Center);

    let name = gtk::Label::new(Some(&acct.display_name));
    name.set_xalign(0.0);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.add_css_class("metis-settings-row-title");
    text.append(&name);

    let mut user_line = acct.username.clone();
    if acct.is_current {
        user_line.push_str(" · ");
        user_line.push_str(&tr("you"));
    }
    let user_lbl = gtk::Label::new(Some(&user_line));
    user_lbl.set_xalign(0.0);
    user_lbl.add_css_class("metis-settings-user-username");
    text.append(&user_lbl);

    let role = if acct.is_admin {
        tr("Administrator")
    } else {
        tr("Standard user")
    };
    let role_lbl = gtk::Label::new(Some(&role));
    role_lbl.set_xalign(0.0);
    role_lbl.add_css_class("metis-settings-user-role");
    if acct.is_admin {
        role_lbl.add_css_class("metis-settings-user-role-admin");
    }
    text.append(&role_lbl);
    row.append(&text);

    let controls = gtk::Box::new(gtk::Orientation::Vertical, 8);
    controls.set_valign(gtk::Align::Center);
    controls.set_halign(gtk::Align::End);

    let admin_line = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    admin_line.set_halign(gtk::Align::End);
    let admin_lbl = gtk::Label::new(Some(&tr("Administrator")));
    admin_lbl.add_css_class("metis-settings-user-admin-label");
    let admin_sw = gtk::Switch::new();
    admin_sw.set_valign(gtk::Align::Center);
    admin_sw.set_active(acct.is_admin);
    if acct.is_current {
        // Don't let the session user demote themselves from this page.
        admin_sw.set_sensitive(false);
        admin_lbl.set_sensitive(false);
    }
    {
        let user = acct.username.clone();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        ui::defer_switch_active_notify(&admin_sw, move |active| {
            let user = user.clone();
            let accounts = accounts.clone();
            let list_box = list_box.clone();
            bg::run_bg(
                move || metis_remote::set_admin(&user, active),
                move |result| {
                    if let Err(err) = result {
                        tracing::warn!(%err, "failed to set admin");
                    }
                    refresh_account_list(&accounts, &list_box);
                },
            );
        });
    }
    admin_line.append(&admin_lbl);
    admin_line.append(&admin_sw);
    controls.append(&admin_line);

    let btns = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    btns.set_halign(gtk::Align::End);
    let pw = gtk::Button::with_label(&tr("Change password"));
    pw.add_css_class("flat");
    pw.add_css_class("metis-settings-user-action");
    {
        let user = acct.username.clone();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        pw.connect_clicked(move |_| {
            open_password_sheet(&user, accounts.clone(), list_box.clone());
        });
    }
    btns.append(&pw);

    if !acct.is_current {
        let remove = gtk::Button::with_label(&tr("Remove"));
        remove.add_css_class("destructive-action");
        remove.add_css_class("flat");
        remove.add_css_class("metis-settings-user-action");
        let user = acct.username.clone();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        remove.connect_clicked(move |_| {
            let user = user.clone();
            let accounts = accounts.clone();
            let list_box = list_box.clone();
            let msg = format!(
                "{} “{}”? {}",
                tr("Delete user"),
                user,
                tr("Home directory will be removed.")
            );
            dialog::confirm(
                &tr("Remove user"),
                &msg,
                dialog::ConfirmButtons {
                    cancel: tr("Cancel"),
                    accept: tr("Remove"),
                    accept_destructive: true,
                },
                Rc::new(move || {
                    let user = user.clone();
                    let accounts = accounts.clone();
                    let list_box = list_box.clone();
                    bg::run_bg(
                        move || metis_remote::remove_user(&user),
                        move |result| {
                            if let Err(err) = result {
                                tracing::warn!(%err, "failed to remove user");
                            }
                            refresh_account_list(&accounts, &list_box);
                        },
                    );
                }),
                Rc::new(|| {}),
            );
        });
        btns.append(&remove);
    }
    controls.append(&btns);
    row.append(&controls);
    row
}
fn open_password_sheet(user: &str, accounts: Rc<RefCell<Vec<AccountInfo>>>, list_box: gtk::Box) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let p1 = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .placeholder_text(tr("New password"))
        .hexpand(true)
        .build();
    let p2 = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .placeholder_text(tr("Confirm password"))
        .hexpand(true)
        .build();
    body.append(&p1);
    body.append(&p2);
    let err = gtk::Label::new(None);
    err.add_css_class("metis-settings-hint");
    err.set_xalign(0.0);
    body.append(&err);
    let apply = gtk::Button::with_label(&tr("Set password"));
    apply.add_css_class("suggested-action");
    apply.set_halign(gtk::Align::End);
    body.append(&apply);

    let user = user.to_string();
    apply.connect_clicked(move |_| {
        let mut a = p1.text().to_string();
        let mut b = p2.text().to_string();
        if a.is_empty() {
            err.set_label(&tr("Password must not be empty."));
            a.zeroize();
            b.zeroize();
            return;
        }
        if a != b {
            err.set_label(&tr("Passwords do not match."));
            a.zeroize();
            b.zeroize();
            return;
        }
        b.zeroize();
        let user = user.clone();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        let err = err.clone();
        bg::run_bg(
            move || {
                let result = metis_remote::set_account_password(&user, &a);
                let mut a = a;
                a.zeroize();
                result
            },
            move |result| match result {
                Ok(()) => {
                    dialog::dismiss(false);
                    refresh_account_list(&accounts, &list_box);
                }
                Err(e) => {
                    tracing::warn!(%e, "failed to set password");
                    err.set_label(&e);
                }
            },
        );
    });

    let _ = dialog::present(&tr("Change password"), &body, Rc::new(|| {}));
}

fn open_add_user_sheet(accounts: Rc<RefCell<Vec<AccountInfo>>>, list_box: gtk::Box) {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let username = gtk::Entry::builder()
        .placeholder_text(tr("Username"))
        .hexpand(true)
        .build();
    ui::swallow_empty_backspace(&username);
    let full_name = gtk::Entry::builder()
        .placeholder_text(tr("Full name"))
        .hexpand(true)
        .build();
    ui::swallow_empty_backspace(&full_name);
    let p1 = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .placeholder_text(tr("Password"))
        .hexpand(true)
        .build();
    let p2 = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .placeholder_text(tr("Confirm password"))
        .hexpand(true)
        .build();
    let (admin_row, admin_sw) = ui::switch_row(&tr("Allow administrator (sudo) access"));
    body.append(&username);
    body.append(&full_name);
    body.append(&p1);
    body.append(&p2);
    body.append(&admin_row);
    let err = gtk::Label::new(None);
    err.add_css_class("metis-settings-hint");
    err.set_xalign(0.0);
    body.append(&err);
    let create = gtk::Button::with_label(&tr("Create user"));
    create.add_css_class("suggested-action");
    create.set_halign(gtk::Align::End);
    body.append(&create);

    let create_btn = create.clone();
    create_btn.connect_clicked(move |_| {
        let user = username.text().to_string();
        let name = full_name.text().to_string();
        let mut a = p1.text().to_string();
        let mut b = p2.text().to_string();
        let admin = admin_sw.is_active();
        if user.is_empty() {
            err.set_label(&tr("Username is required."));
            a.zeroize();
            b.zeroize();
            return;
        }
        if a.is_empty() || a != b {
            err.set_label(&tr("Enter matching non-empty passwords."));
            a.zeroize();
            b.zeroize();
            return;
        }
        b.zeroize();
        let accounts = accounts.clone();
        let list_box = list_box.clone();
        create.set_sensitive(false);
        err.set_label(&tr("Waiting for administrator approval…"));
        let err = err.clone();
        let create = create.clone();
        bg::run_bg(
            move || {
                let result = metis_remote::add_user(&user, &name, admin, &a);
                let mut a = a;
                a.zeroize();
                result
            },
            move |result: Result<(), String>| {
                create.set_sensitive(true);
                match result {
                    Ok(()) => {
                        dialog::dismiss(false);
                        refresh_account_list(&accounts, &list_box);
                    }
                    Err(e) => {
                        tracing::warn!(%e, "failed to add user");
                        err.set_label(&e);
                    }
                }
            },
        );
    });

    let _ = dialog::present(&tr("Add User"), &body, Rc::new(|| {}));
}
