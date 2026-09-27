//! Interactive capture consent for xdg Screenshot / ScreenCast.
//!
//! Sandboxed clients expect a user prompt before the portal returns a PipeWire
//! node or `file://` screenshot. CI / automation can set
//! `METIS_PORTAL_AUTO_APPROVE=1` to skip the dialog.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gtk::prelude::*;
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};

/// Kind of portal capture being requested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureKind {
    Screenshot,
    Screencast,
}

fn session_allowlist() -> &'static Mutex<HashSet<String>> {
    static LIST: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    LIST.get_or_init(|| Mutex::new(HashSet::new()))
}

fn allowlist_key(kind: CaptureKind, app_label: &str) -> String {
    format!("{kind:?}:{app_label}")
}

fn auto_approve_env() -> bool {
    matches!(
        std::env::var("METIS_PORTAL_AUTO_APPROVE").as_deref(),
        Ok("1") | Ok("true") | Ok("TRUE") | Ok("yes") | Ok("YES")
    )
}

/// Human-readable app label for the consent dialog.
pub fn display_app_label(app_id: Option<&ashpd::MaybeAppID>) -> String {
    match app_id {
        Some(id) => {
            let s = id.to_string();
            if s.is_empty() {
                "An application".into()
            } else {
                s
            }
        }
        None => "An application".into(),
    }
}

/// Ask the user whether `app_label` may capture. Returns `true` when allowed.
pub async fn request_capture_consent(kind: CaptureKind, app_label: &str) -> bool {
    if auto_approve_env() {
        tracing::debug!(?kind, app = %app_label, "capture consent auto-approved (env)");
        return true;
    }

    let key = allowlist_key(kind, app_label);
    if session_allowlist()
        .lock()
        .map(|s| s.contains(&key))
        .unwrap_or(false)
    {
        tracing::debug!(?kind, app = %app_label, "capture consent remembered this session");
        return true;
    }

    let app_label_owned = app_label.to_string();
    let allowed = tokio::task::spawn_blocking(move || run_consent_dialog(kind, &app_label_owned))
        .await
        .unwrap_or(false);

    if allowed && let Ok(mut list) = session_allowlist().lock() {
        list.insert(allowlist_key(kind, app_label));
    }
    allowed
}

fn run_consent_dialog(kind: CaptureKind, app_label: &str) -> bool {
    let (tx, rx) = std::sync::mpsc::sync_channel::<bool>(1);
    let app_label = app_label.to_string();
    let title: &'static str = match kind {
        CaptureKind::Screenshot => "Screenshot request",
        CaptureKind::Screencast => "Screen share request",
    };
    let body = match kind {
        CaptureKind::Screenshot => format!("{app_label} wants to take a screenshot."),
        CaptureKind::Screencast => format!("{app_label} wants to share your screen."),
    };

    let thread = std::thread::Builder::new()
        .name("metis-portal-consent".into())
        .spawn(move || {
            if gtk::gdk::Display::default().is_none()
                && let Err(err) = gtk::init()
            {
                tracing::error!(%err, "consent dialog: gtk init failed");
                let _ = tx.try_send(false);
                return;
            }

            install_consent_css();

            let app = gtk::Application::builder()
                .application_id("io.metis.portal.consent")
                .flags(gtk::gio::ApplicationFlags::NON_UNIQUE)
                .build();

            let tx_activate = tx.clone();
            app.connect_activate(move |app| {
                present_dialog(app, title, &body, tx_activate.clone());
            });

            let argv: [&str; 0] = [];
            let _ = app.run_with_args(&argv);
            // Deny if the dialog closed without an explicit choice.
            let _ = tx.try_send(false);
        });

    match thread {
        Ok(handle) => {
            let result = rx.recv_timeout(Duration::from_secs(120)).unwrap_or(false);
            let _ = handle.join();
            result
        }
        Err(err) => {
            tracing::error!(%err, "consent dialog: failed to spawn UI thread");
            false
        }
    }
}

fn present_dialog(
    app: &gtk::Application,
    title: &'static str,
    body: &str,
    tx: std::sync::mpsc::SyncSender<bool>,
) {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title(title)
        .resizable(false)
        .decorated(false)
        .default_width(420)
        .build();
    window.add_css_class("metis-portal-consent-window");

    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    window.set_namespace(Some("metis-portal-consent"));
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Top, 56);

    let root = gtk::Box::new(gtk::Orientation::Vertical, 14);
    root.add_css_class("metis-portal-consent");
    root.set_width_request(420);

    let heading = gtk::Label::new(Some(title));
    heading.set_xalign(0.0);
    heading.add_css_class("metis-portal-consent-title");
    root.append(&heading);

    let msg = gtk::Label::new(Some(body));
    msg.set_xalign(0.0);
    msg.set_wrap(true);
    msg.add_css_class("metis-portal-consent-body");
    root.append(&msg);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let deny = gtk::Button::with_label("Deny");
    deny.add_css_class("metis-portal-consent-deny");
    let allow = gtk::Button::with_label("Allow");
    allow.add_css_class("metis-portal-consent-allow");
    actions.append(&deny);
    actions.append(&allow);
    root.append(&actions);

    window.set_child(Some(&root));

    let decide = {
        let app = app.clone();
        let window = window.clone();
        move |allowed: bool| {
            let _ = tx.try_send(allowed);
            window.close();
            app.quit();
        }
    };
    {
        let decide = decide.clone();
        deny.connect_clicked(move |_| decide(false));
    }
    {
        let decide = decide.clone();
        allow.connect_clicked(move |_| decide(true));
    }
    window.connect_close_request({
        let decide = decide.clone();
        move |_| {
            decide(false);
            glib::Propagation::Proceed
        }
    });

    window.present();
}

fn theme_token_name() -> &'static str {
    match metis_config::load_theme_preference().unwrap_or(metis_config::ThemeMode::Dark) {
        metis_config::ThemeMode::Light => "light",
        metis_config::ThemeMode::Dark => "dark",
        metis_config::ThemeMode::System => {
            if gtk::Settings::default()
                .map(|s| s.is_gtk_application_prefer_dark_theme())
                .unwrap_or(true)
            {
                "dark"
            } else {
                "light"
            }
        }
    }
}

fn install_consent_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let name = theme_token_name();
    let tokens = metis_config::load_theme_tokens(name);
    if let Some(settings) = gtk::Settings::default() {
        settings.set_gtk_application_prefer_dark_theme(name == "dark");
    }

    let surface = &tokens.surface_raised;
    let text = &tokens.text;
    let muted = &tokens.text_muted;
    let border = &tokens.border;
    let accent = tokens.accent_primary();
    let css = format!(
        r#"
        window.metis-portal-consent-window {{
            background-color: transparent;
        }}
        .metis-portal-consent {{
            background-color: {surface};
            color: {text};
            border: 1px solid {border};
            border-radius: 16px;
            padding: 18px 20px;
        }}
        .metis-portal-consent-title {{
            font-size: 16px;
            font-weight: 700;
            color: {text};
        }}
        .metis-portal-consent-body {{
            font-size: 13px;
            color: {muted};
        }}
        button.metis-portal-consent-allow {{
            background-color: {accent};
            color: #ffffff;
            border-radius: 8px;
            padding: 8px 16px;
            border: none;
        }}
        button.metis-portal-consent-deny {{
            background-color: transparent;
            color: {text};
            border: 1px solid {border};
            border-radius: 8px;
            padding: 8px 16px;
        }}
        "#
    );
    let provider = gtk::CssProvider::new();
    provider.load_from_string(&css);
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_label_fallback() {
        assert_eq!(display_app_label(None), "An application");
    }

    #[test]
    fn allowlist_key_stable() {
        assert_eq!(
            allowlist_key(CaptureKind::Screenshot, "app.foo"),
            "Screenshot:app.foo"
        );
    }
}
