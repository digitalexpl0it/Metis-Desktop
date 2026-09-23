//! Settings → System → Screenshot — binds to `~/.config/metis/screenshot.json`.

use std::cell::RefCell;
use std::rc::Rc;

use gio::prelude::*;
use gtk::prelude::*;
use metis_config::{
    AfterCaptureAction, ScreenshotConfig, ScreenshotMode, expand_save_dir, load_screenshot_config,
    save_screenshot_config,
};
use metis_i18n::tr;

use crate::ui;

pub fn build() -> gtk::Widget {
    let (scroller, content) = ui::page_for("screenshot");
    let cfg = Rc::new(RefCell::new(load_screenshot_config()));

    let hint = gtk::Label::new(Some(&tr(
        "Defaults for the Metis screenshot picker (PrtSc). Interactive captures can \
         open the Metis editor for annotate and OCR. Shift+PrtSc stays fast and never \
         auto-opens the editor. Metis packages include the OCR engine.",
    )));
    hint.set_xalign(0.0);
    hint.set_wrap(true);
    hint.add_css_class("metis-settings-hint");
    content.append(&hint);

    let (capture_card, capture_body) =
        ui::section_with_icon(&tr("Capture"), "camera-photo-symbolic");

    let mode = gtk::DropDown::from_strings(&[&tr("Selection"), &tr("Full screen"), &tr("Window")]);
    mode.set_selected(mode_to_index(cfg.borrow().default_mode));
    capture_body.append(&ui::row(&tr("Default mode"), &mode));

    let pointer = gtk::Switch::new();
    pointer.set_active(cfg.borrow().draw_cursor);
    pointer.set_halign(gtk::Align::End);
    capture_body.append(&ui::row(&tr("Include pointer"), &pointer));

    let delay = gtk::SpinButton::with_range(0.0, 30.0, 1.0);
    delay.set_digits(0);
    delay.set_value(cfg.borrow().delay_seconds as f64);
    capture_body.append(&ui::row(&tr("Delay (seconds)"), &delay));

    let prefer = gtk::Switch::new();
    prefer.set_active(cfg.borrow().prefer_pointer_output);
    prefer.set_halign(gtk::Align::End);
    capture_body.append(&ui::row(&tr("Capture monitor under pointer"), &prefer));

    content.append(&capture_card);

    let (after_card, after_body) =
        ui::section_with_icon(&tr("After capture"), "document-save-symbolic");

    let after = gtk::DropDown::from_strings(&[
        &tr("Edit in Metis"),
        &tr("Copy"),
        &tr("Save"),
        &tr("Copy and save"),
        &tr("Open externally"),
    ]);
    after.set_selected(action_to_index(cfg.borrow().interactive_action()));
    after_body.append(&ui::row(&tr("Interactive (PrtSc)"), &after));

    let instant = gtk::DropDown::from_strings(&[&tr("Copy"), &tr("Save"), &tr("Copy and save")]);
    instant.set_selected(match cfg.borrow().instant_action() {
        AfterCaptureAction::Save => 1,
        AfterCaptureAction::CopyAndSave => 2,
        _ => 0,
    });
    after_body.append(&ui::row(&tr("Instant full (Shift+PrtSc)"), &instant));

    let dir_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let dir_entry = gtk::Entry::new();
    dir_entry.set_text(&cfg.borrow().save_dir);
    dir_entry.set_hexpand(true);
    let browse = gtk::Button::with_label(&tr("Browse…"));
    dir_row.append(&dir_entry);
    dir_row.append(&browse);
    after_body.append(&ui::row(&tr("Save folder"), &dir_row));

    content.append(&after_card);

    let persist = {
        let cfg = cfg.clone();
        let mode = mode.clone();
        let pointer = pointer.clone();
        let delay = delay.clone();
        let prefer = prefer.clone();
        let after = after.clone();
        let instant = instant.clone();
        let dir_entry = dir_entry.clone();
        move || {
            let interactive = index_to_action(after.selected());
            let instant_action = match instant.selected() {
                1 => AfterCaptureAction::Save,
                2 => AfterCaptureAction::CopyAndSave,
                _ => AfterCaptureAction::Copy,
            };
            let next = ScreenshotConfig {
                default_mode: index_to_mode(mode.selected()),
                draw_cursor: pointer.is_active(),
                delay_seconds: delay.value().max(0.0).round() as u32,
                after_capture: interactive,
                interactive_after_capture: None,
                instant_after_capture: instant_action,
                prefer_pointer_output: prefer.is_active(),
                save_dir: dir_entry.text().to_string(),
            };
            *cfg.borrow_mut() = next.clone();
            if let Err(err) = save_screenshot_config(&next) {
                tracing::warn!(%err, "failed to save screenshot.json");
            }
        }
    };

    {
        let persist = persist.clone();
        mode.connect_selected_notify(move |_| persist());
    }
    {
        let persist = persist.clone();
        pointer.connect_active_notify(move |_| persist());
    }
    {
        let persist = persist.clone();
        delay.connect_value_changed(move |_| persist());
    }
    {
        let persist = persist.clone();
        prefer.connect_active_notify(move |_| persist());
    }
    {
        let persist = persist.clone();
        after.connect_selected_notify(move |_| persist());
    }
    {
        let persist = persist.clone();
        instant.connect_selected_notify(move |_| persist());
    }
    {
        let persist = persist.clone();
        dir_entry.connect_changed(move |_| persist());
    }
    {
        let dir_entry = dir_entry.clone();
        let persist = persist.clone();
        browse.connect_clicked(move |_| {
            let dialog = gtk::FileDialog::builder()
                .title(tr("Screenshot save folder"))
                .modal(true)
                .build();
            let initial = expand_save_dir(&dir_entry.text());
            if initial.is_dir() {
                dialog.set_initial_folder(Some(&gio::File::for_path(&initial)));
            }
            let dir_entry = dir_entry.clone();
            let persist = persist.clone();
            dialog.select_folder(
                None::<&gtk::Window>,
                None::<&gio::Cancellable>,
                move |result| {
                    if let Ok(folder) = result
                        && let Some(path) = folder.path()
                    {
                        dir_entry.set_text(&path.display().to_string());
                        persist();
                    }
                },
            );
        });
    }

    scroller.upcast()
}

fn mode_to_index(mode: ScreenshotMode) -> u32 {
    match mode {
        ScreenshotMode::Selection => 0,
        ScreenshotMode::Screen => 1,
        ScreenshotMode::Window => 2,
    }
}

fn index_to_mode(index: u32) -> ScreenshotMode {
    match index {
        1 => ScreenshotMode::Screen,
        2 => ScreenshotMode::Window,
        _ => ScreenshotMode::Selection,
    }
}

fn action_to_index(action: AfterCaptureAction) -> u32 {
    match action {
        AfterCaptureAction::Edit => 0,
        AfterCaptureAction::Copy => 1,
        AfterCaptureAction::Save => 2,
        AfterCaptureAction::CopyAndSave => 3,
        AfterCaptureAction::Open => 4,
    }
}

fn index_to_action(index: u32) -> AfterCaptureAction {
    match index {
        1 => AfterCaptureAction::Copy,
        2 => AfterCaptureAction::Save,
        3 => AfterCaptureAction::CopyAndSave,
        4 => AfterCaptureAction::Open,
        _ => AfterCaptureAction::Edit,
    }
}
