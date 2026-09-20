//! Settings → System → Reset — factory-reset `~/.config/metis` with backup.

use std::cell::RefCell;
use std::rc::Rc;

use gtk::prelude::*;
use metis_config::{config_dir, reset_metis_config, ResetOptions};
use metis_i18n::tr;

use crate::dialog;
use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("reset");

    let hint = gtk::Label::new(Some(&tr(
        "Restore Metis preferences to factory defaults. This clears files under \
         ~/.config/metis (edge bar, themes, wallpaper, widgets, keybinds, and more). \
         A session restart is recommended afterward so the compositor and shell \
         reload cleanly.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let path_hint = gtk::Label::new(Some(&format!(
        "{} {}",
        tr("Config folder:"),
        config_dir().display()
    )));
    path_hint.set_xalign(0.0);
    path_hint.set_wrap(true);
    path_hint.set_selectable(true);
    path_hint.add_css_class("metis-settings-hint");
    content.append(&path_hint);

    let (opts_card, opts_body) =
        ui::section_with_icon(&tr("Options"), "preferences-system-symbolic");

    let backup = gtk::Switch::new();
    backup.set_active(true);
    backup.set_halign(gtk::Align::End);
    opts_body.append(&ui::row(&tr("Backup config first"), &backup));

    let keep_themes = gtk::Switch::new();
    keep_themes.set_active(true);
    keep_themes.set_halign(gtk::Align::End);
    opts_body.append(&ui::row(
        &tr("Keep custom themes (not dark/light)"),
        &keep_themes,
    ));

    let rerun = gtk::Switch::new();
    rerun.set_active(true);
    rerun.set_halign(gtk::Align::End);
    opts_body.append(&ui::row(&tr("Run first-run setup again"), &rerun));

    content.append(&opts_card);

    let (action_card, action_body) = ui::section_with_icon(&tr("Reset"), "edit-clear-all-symbolic");

    let status = gtk::Label::new(None);
    status.set_xalign(0.0);
    status.set_wrap(true);
    status.add_css_class("metis-settings-hint");
    action_body.append(&status);

    let reset_btn = gtk::Button::with_label(&tr("Reset Metis…"));
    reset_btn.add_css_class("destructive-action");
    reset_btn.set_halign(gtk::Align::Start);
    action_body.append(&reset_btn);
    content.append(&action_card);

    {
        let backup = backup.clone();
        let keep_themes = keep_themes.clone();
        let rerun = rerun.clone();
        let status = status.clone();
        reset_btn.connect_clicked(move |_| {
            let opts = ResetOptions {
                backup: backup.is_active(),
                keep_custom_themes: keep_themes.is_active(),
                rerun_onboarding: rerun.is_active(),
            };
            let status = status.clone();
            let opts_cell = Rc::new(RefCell::new(opts));
            let body = confirm_body(&opts_cell.borrow());
            dialog::confirm_with_extra(
                &tr("Reset Metis?"),
                &body,
                None,
                dialog::ConfirmButtons {
                    cancel: tr("Cancel"),
                    accept: tr("Reset Metis"),
                    accept_destructive: true,
                },
                Rc::new({
                    let opts_cell = opts_cell.clone();
                    let status = status.clone();
                    move || match reset_metis_config(&opts_cell.borrow()) {
                        Ok(result) => {
                            let mut msg =
                                tr("Metis preferences were reset. Log out and sign back in \
                                 (or restart the session) for a full reload.");
                            if let Some(path) = result.backup_path {
                                msg.push('\n');
                                msg.push_str(&tr("Backup saved to:"));
                                msg.push(' ');
                                msg.push_str(&path.display().to_string());
                            }
                            status.set_text(&msg);
                        }
                        Err(err) => {
                            status.set_text(&format!("{} {err}", tr("Reset failed:")));
                        }
                    }
                }),
                Rc::new(|| {}),
            );
        });
    }

    scroller.upcast()
}

fn confirm_body(opts: &ResetOptions) -> String {
    let mut lines = vec![
        tr("This permanently deletes Metis settings under ~/.config/metis.").to_string(),
        String::new(),
        tr("Removed:").to_string(),
        format!(
            "• {}",
            tr("Edge bar, wallpaper, desk layout, desktop widgets")
        ),
        format!(
            "• {}",
            tr("Appearance / stock themes, keybinds, input, power")
        ),
        format!(
            "• {}",
            tr("Weather, calendars, gaming, remote, and other prefs")
        ),
        String::new(),
    ];
    if opts.backup {
        lines.push(format!(
            "• {}",
            tr("A copy will be saved to ~/metis-config-backup-… first")
        ));
    } else {
        lines.push(format!("• {}", tr("No backup will be created")));
    }
    if opts.keep_custom_themes {
        lines.push(format!(
            "• {}",
            tr("Custom theme files (not dark/light) will be kept")
        ));
    }
    if opts.rerun_onboarding {
        lines.push(format!("• {}", tr("First-run setup will show again")));
    } else {
        lines.push(format!("• {}", tr("Onboarding stays marked complete")));
    }
    lines.join("\n")
}
