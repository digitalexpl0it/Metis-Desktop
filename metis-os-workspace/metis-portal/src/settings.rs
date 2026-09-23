use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use ashpd::{
    PortalError,
    backend::settings::{SettingsImpl, SettingsSignalEmitter},
    desktop::settings::{APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY, ColorScheme, Namespace},
    zbus::zvariant::{OwnedValue, Value},
};
use async_trait::async_trait;
use metis_config::{
    SESSION_GTK_DECORATION_LAYOUT, SESSION_WM_BUTTON_LAYOUT, ThemeMode, load_theme_preference,
};

const NS_WM: &str = "org.gnome.desktop.wm.preferences";
const KEY_BUTTON_LAYOUT: &str = "button-layout";
const NS_INTERFACE: &str = "org.gnome.desktop.interface";
const KEY_DECO_LAYOUT: &str = "gtk-decoration-layout";
const KEY_GTK_THEME: &str = "gtk-theme";
/// Portal / GTK decoration layout for client-side decorations.
///
/// Metis only draws server-side chrome for classified terminal/SSD apps; GTK
/// headerbar clients (Cheese, Calculator, …) and browsers keep native CSD.
/// Serving `":"` here hid minimize/maximize on Firefox/Chromium and could
/// leave a partial headerbar visible alongside Metis SSD on misclassified apps.
static SETTINGS_EMITTER: OnceLock<Mutex<Option<Arc<dyn SettingsSignalEmitter>>>> = OnceLock::new();
static THEME_WATCH_STARTED: OnceLock<()> = OnceLock::new();

struct Snapshot {
    color_scheme: ColorScheme,
    gtk_theme: String,
    mode: ThemeMode,
}

impl Snapshot {
    fn from_config() -> Self {
        let mode = load_theme_preference().unwrap_or(ThemeMode::Dark);
        Self {
            color_scheme: color_scheme_for(&mode),
            gtk_theme: gtk_theme_for(&mode),
            mode,
        }
    }
}

fn color_scheme_for(mode: &ThemeMode) -> ColorScheme {
    match mode {
        ThemeMode::Light => ColorScheme::PreferLight,
        ThemeMode::Dark => ColorScheme::PreferDark,
        ThemeMode::System => ColorScheme::NoPreference,
    }
}

fn gtk_theme_for(mode: &ThemeMode) -> String {
    // Portal FileChooser (xdg-desktop-portal-gtk) still follows the gtk-theme
    // name; libadwaita prefers color-scheme. Serve Adwaita-dark in dark mode so
    // Import / Open dialogs are not stuck on light Adwaita.
    match mode {
        ThemeMode::Dark => "Adwaita-dark".into(),
        ThemeMode::Light | ThemeMode::System => "Adwaita".into(),
    }
}

fn owned_string(value: &str) -> OwnedValue {
    Value::from(value).try_into().unwrap_or_else(|_| {
        OwnedValue::try_from(Value::from(String::from(value))).expect("portal string value")
    })
}

/// Spec: `org.freedesktop.appearance` `color-scheme` is type `u` (uint32).
/// ashpd's `OwnedValue::from(ColorScheme)` serializes the integer literal as
/// signed `i`, which breaks Chromium/Electron (`PopVariantOfUint32` fails —
/// GitHub Desktop logs dark_mode_manager_linux.cc "Failed to read color-scheme").
fn owned_color_scheme(scheme: ColorScheme) -> OwnedValue {
    let value: u32 = match scheme {
        ColorScheme::PreferDark => 1,
        ColorScheme::PreferLight => 2,
        ColorScheme::NoPreference => 0,
    };
    Value::from(value)
        .try_into()
        .unwrap_or_else(|_| OwnedValue::from(value))
}

fn namespace_allowed(namespaces: &[String], ns: &str) -> bool {
    namespaces.is_empty() || namespaces.iter().any(|n| n == ns)
}

fn appearance_namespace(snapshot: &Snapshot) -> Namespace {
    let mut map = HashMap::new();
    map.insert(
        COLOR_SCHEME_KEY.to_owned(),
        owned_color_scheme(snapshot.color_scheme),
    );
    map
}

fn interface_namespace(snapshot: &Snapshot) -> Namespace {
    let mut map = HashMap::new();
    map.insert(
        KEY_DECO_LAYOUT.to_owned(),
        owned_string(SESSION_GTK_DECORATION_LAYOUT),
    );
    map.insert(KEY_GTK_THEME.to_owned(), owned_string(&snapshot.gtk_theme));
    map
}

fn wm_namespace() -> Namespace {
    let mut map = HashMap::new();
    map.insert(
        KEY_BUTTON_LAYOUT.to_owned(),
        owned_string(SESSION_WM_BUTTON_LAYOUT),
    );
    map
}

fn read_key(snapshot: &Snapshot, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
    match (namespace, key) {
        (APPEARANCE_NAMESPACE, COLOR_SCHEME_KEY) => Ok(owned_color_scheme(snapshot.color_scheme)),
        (NS_WM, KEY_BUTTON_LAYOUT) => Ok(owned_string(SESSION_WM_BUTTON_LAYOUT)),
        (NS_INTERFACE, KEY_DECO_LAYOUT) => Ok(owned_string(SESSION_GTK_DECORATION_LAYOUT)),
        (NS_INTERFACE, KEY_GTK_THEME) => Ok(owned_string(&snapshot.gtk_theme)),
        _ => Err(PortalError::NotFound(format!(
            "unknown namespace/key: {namespace}/{key}"
        ))),
    }
}

fn emitter_slot() -> &'static Mutex<Option<Arc<dyn SettingsSignalEmitter>>> {
    SETTINGS_EMITTER.get_or_init(|| Mutex::new(None))
}

async fn emit_appearance(snapshot: &Snapshot) {
    let emitter = {
        let guard = match emitter_slot().lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.clone()
    };
    let Some(emitter) = emitter else {
        return;
    };
    if let Err(err) = emitter
        .emit_changed(
            APPEARANCE_NAMESPACE,
            COLOR_SCHEME_KEY,
            // Must be `u`, not ashpd's ColorScheme→`i` conversion (Chromium
            // DarkModeManagerLinux requires uint32).
            Value::from(match snapshot.color_scheme {
                ColorScheme::PreferDark => 1u32,
                ColorScheme::PreferLight => 2u32,
                ColorScheme::NoPreference => 0u32,
            }),
        )
        .await
    {
        tracing::warn!(%err, "portal: emit color-scheme failed");
    }
    if let Err(err) = emitter
        .emit_changed(
            NS_INTERFACE,
            KEY_GTK_THEME,
            Value::from(snapshot.gtk_theme.as_str()),
        )
        .await
    {
        tracing::warn!(%err, "portal: emit gtk-theme failed");
    }
    if let Err(err) = emitter
        .emit_changed(
            NS_WM,
            KEY_BUTTON_LAYOUT,
            Value::from(SESSION_WM_BUTTON_LAYOUT),
        )
        .await
    {
        tracing::warn!(%err, "portal: emit button-layout failed");
    }
    if let Err(err) = emitter
        .emit_changed(
            NS_INTERFACE,
            KEY_DECO_LAYOUT,
            Value::from(SESSION_GTK_DECORATION_LAYOUT),
        )
        .await
    {
        tracing::warn!(%err, "portal: emit gtk-decoration-layout failed");
    }
}

fn ensure_theme_watch() {
    if THEME_WATCH_STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async {
        // Session start: push gsettings so early GTK apps see Metis dark/light.
        metis_config::sync_session_appearance_from_config();
        let mut last = load_theme_preference().unwrap_or(ThemeMode::Dark);
        // Emit once so portal clients that subscribe after connect get a baseline.
        emit_appearance(&Snapshot::from_config()).await;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let mode = load_theme_preference().unwrap_or(ThemeMode::Dark);
            if mode == last {
                continue;
            }
            last = mode;
            let snapshot = Snapshot::from_config();
            metis_config::apply_session_appearance_gsettings(snapshot.mode);
            emit_appearance(&snapshot).await;
            tracing::info!(?mode, "portal: appearance updated from config");
        }
    });
}

#[derive(Default)]
pub struct MetisSettings;

#[async_trait]
impl SettingsImpl for MetisSettings {
    async fn read_all(
        &self,
        namespaces: Vec<String>,
    ) -> Result<HashMap<String, Namespace>, PortalError> {
        let snapshot = Snapshot::from_config();
        let mut out = HashMap::new();
        if namespace_allowed(&namespaces, APPEARANCE_NAMESPACE) {
            out.insert(
                APPEARANCE_NAMESPACE.to_owned(),
                appearance_namespace(&snapshot),
            );
        }
        if namespace_allowed(&namespaces, NS_INTERFACE) {
            out.insert(NS_INTERFACE.to_owned(), interface_namespace(&snapshot));
        }
        if namespace_allowed(&namespaces, NS_WM) {
            out.insert(NS_WM.to_owned(), wm_namespace());
        }
        Ok(out)
    }

    async fn read(&self, namespace: &str, key: &str) -> Result<OwnedValue, PortalError> {
        let snapshot = Snapshot::from_config();
        read_key(&snapshot, namespace, key)
    }

    fn set_signal_emitter(&mut self, signal_emitter: std::sync::Arc<dyn SettingsSignalEmitter>) {
        if let Ok(mut slot) = emitter_slot().lock() {
            *slot = Some(signal_emitter);
        }
        ensure_theme_watch();
    }
}
