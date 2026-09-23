use std::path::Path;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

use crate::{Cli, theme};

pub fn show(app: &gtk::Application, cli: Cli) {
    theme::install();
    let Some(path) = cli.path else { return };
    let window = gtk::Window::builder()
        .application(app)
        .title("Pinned Screenshot")
        .build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    window.set_namespace(Some("metis-screenshot-pin"));
    window.set_anchor(Edge::Top, true);
    window.set_anchor(Edge::Left, true);
    window.set_margin(Edge::Top, 80);
    window.set_margin(Edge::Left, 80);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 4);
    root.add_css_class("metis-screenshot-pill");
    let image = gtk::Picture::for_file(&gio::File::for_path(&path));
    image.set_can_shrink(true);
    image.set_content_fit(gtk::ContentFit::Contain);
    image.set_size_request(420, 280);
    root.append(&image);
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    for label in ["Copy", "Save", "Close"] {
        let button = gtk::Button::with_label(label);
        match label {
            "Copy" => {
                let path = path.clone();
                button.connect_clicked(move |_| {
                    let _ = std::process::Command::new("wl-copy")
                        .args(["-t", "image/png"])
                        .arg(&path)
                        .status();
                });
            }
            "Save" => {
                let path = path.clone();
                button.connect_clicked(move |_| println!("{}", path.display()));
            }
            _ => {
                let window = window.clone();
                button.connect_clicked(move |_| window.close());
            }
        }
        actions.append(&button);
    }
    root.append(&actions);
    let menu = gtk::Popover::new();
    let menu_actions = gtk::Box::new(gtk::Orientation::Vertical, 4);
    for label in ["Copy", "Save", "Close"] {
        let button = gtk::Button::with_label(label);
        match label {
            "Copy" => {
                let path = path.clone();
                button.connect_clicked(move |_| {
                    let _ = std::process::Command::new("wl-copy")
                        .args(["-t", "image/png"])
                        .arg(&path)
                        .status();
                });
            }
            "Save" => {
                let path = path.clone();
                button.connect_clicked(move |_| println!("{}", path.display()));
            }
            _ => {
                let window = window.clone();
                button.connect_clicked(move |_| window.close());
            }
        }
        menu_actions.append(&button);
    }
    menu.set_child(Some(&menu_actions));
    menu.set_parent(&root);
    let click = gtk::GestureClick::new();
    click.set_button(3);
    {
        let menu = menu.clone();
        click.connect_pressed(move |_, _, _, _| menu.popup());
    }
    root.add_controller(click);
    let drag = gtk::GestureDrag::new();
    {
        let window = window.clone();
        drag.connect_drag_update(move |_, x, y| {
            window.set_margin(Edge::Top, 80 + y as i32);
            window.set_margin(Edge::Left, 80 + x as i32);
        });
    }
    root.add_controller(drag);
    let keys = gtk::EventControllerKey::new();
    {
        let window = window.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
    }
    window.add_controller(keys);
    window.set_child(Some(&root));
    window.present();
}

pub fn spawn(path: &Path) -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|error| format!("resolve screenshot executable: {error}"))?;
    std::process::Command::new(exe)
        .args(["--mode", "pin", "--path"])
        .arg(path)
        .spawn()
        .map_err(|error| format!("start pin window: {error}"))?;
    Ok(())
}
