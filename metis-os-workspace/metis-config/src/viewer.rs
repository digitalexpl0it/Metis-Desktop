//! RDP viewer recent / saved hosts — `~/.config/metis/viewer.json`.
//!
//! Passwords are never stored here. Per-host FreeRDP session options live on
//! each [`ViewerHost`] (Remmina-style Advanced Desktop Settings).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

const MAX_RECENT: usize = 12;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerHost {
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub username: String,
    /// Optional display name shown on saved-host cards.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    /// Remmina-style session options (Display / Local / Experience / Advanced).
    #[serde(default, skip_serializing_if = "ViewerRdpOptions::is_default")]
    pub options: ViewerRdpOptions,
}

fn default_port() -> u16 {
    3389
}

fn default_true() -> bool {
    true
}

/// How Metis places the FreeRDP client window after connect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewerPlacement {
    /// Auto-move onto a dedicated remote desktop (default).
    #[default]
    Workspace,
    /// Leave as a normal tiled/floating window on the current desktop.
    Window,
}

impl ViewerPlacement {
    pub fn label(self) -> &'static str {
        match self {
            Self::Workspace => "Dedicated desktop",
            Self::Window => "Window on current desktop",
        }
    }

    pub fn all() -> &'static [ViewerPlacement] {
        &[Self::Workspace, Self::Window]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewerColorDepth {
    #[default]
    Auto,
    Bpp15,
    Bpp16,
    Bpp24,
    Bpp32,
}

impl ViewerColorDepth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Bpp15 => "High Color (15 bit)",
            Self::Bpp16 => "High Color (16 bit)",
            Self::Bpp24 => "True Color (24 bit)",
            Self::Bpp32 => "Highest Quality (32 bit)",
        }
    }

    pub fn all() -> &'static [ViewerColorDepth] {
        &[
            Self::Auto,
            Self::Bpp15,
            Self::Bpp16,
            Self::Bpp24,
            Self::Bpp32,
        ]
    }

    pub fn freerdp_bpp(self) -> Option<u16> {
        match self {
            Self::Auto => None,
            Self::Bpp15 => Some(15),
            Self::Bpp16 => Some(16),
            Self::Bpp24 => Some(24),
            Self::Bpp32 => Some(32),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewerDisplayMode {
    /// FreeRDP `/dynamic-resolution` (default).
    #[default]
    Dynamic,
    Fullscreen,
    Windowed,
}

impl ViewerDisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dynamic => "Dynamic resolution",
            Self::Fullscreen => "Fullscreen",
            Self::Windowed => "Windowed",
        }
    }

    pub fn all() -> &'static [ViewerDisplayMode] {
        &[Self::Dynamic, Self::Fullscreen, Self::Windowed]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewerNetwork {
    #[default]
    Auto,
    Lan,
    Broadband,
    BroadbandLow,
    Wan,
    Modem,
}

impl ViewerNetwork {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Lan => "LAN",
            Self::Broadband => "Broadband (high)",
            Self::BroadbandLow => "Broadband (low)",
            Self::Wan => "WAN",
            Self::Modem => "Modem",
        }
    }

    pub fn all() -> &'static [ViewerNetwork] {
        &[
            Self::Auto,
            Self::Lan,
            Self::Broadband,
            Self::BroadbandLow,
            Self::Wan,
            Self::Modem,
        ]
    }

    pub fn freerdp_value(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Lan => "lan",
            Self::Broadband => "broadband",
            Self::BroadbandLow => "broadband-low",
            Self::Wan => "wan",
            Self::Modem => "modem",
        }
    }
}

/// Certificate handling for FreeRDP `/cert:…`.
///
/// Default is `Ignore` for GRD / LAN self-signed hosts (no interactive prompt).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ViewerCertPolicy {
    #[default]
    Ignore,
    /// Trust on first use.
    Tofu,
    Deny,
}

impl ViewerCertPolicy {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ignore => "Ignore (LAN / self-signed)",
            Self::Tofu => "Trust on first use",
            Self::Deny => "Reject untrusted",
        }
    }

    pub fn all() -> &'static [ViewerCertPolicy] {
        &[Self::Ignore, Self::Tofu, Self::Deny]
    }

    pub fn freerdp_value(self) -> &'static str {
        match self {
            Self::Ignore => "ignore",
            Self::Tofu => "tofu",
            Self::Deny => "deny",
        }
    }
}

/// Per-host FreeRDP session options (no secrets).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewerRdpOptions {
    #[serde(default)]
    pub color_depth: ViewerColorDepth,
    #[serde(default)]
    pub display_mode: ViewerDisplayMode,
    /// Windowed width; ignored unless [`ViewerDisplayMode::Windowed`].
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub width: u32,
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub height: u32,
    #[serde(default)]
    pub placement: ViewerPlacement,
    #[serde(default)]
    pub multi_monitor: bool,
    #[serde(default)]
    pub span_monitors: bool,

    /// Off by default — GRD + FreeRDP cliprdr can abort the session.
    #[serde(default)]
    pub clipboard: bool,
    #[serde(default = "default_true")]
    pub audio: bool,
    #[serde(default)]
    pub microphone: bool,
    #[serde(default)]
    pub printers: bool,
    #[serde(default)]
    pub smartcard: bool,

    #[serde(default)]
    pub network: ViewerNetwork,
    #[serde(default)]
    pub wallpaper: bool,
    #[serde(default)]
    pub font_smoothing: bool,
    #[serde(default)]
    pub desktop_composition: bool,
    #[serde(default)]
    pub window_drag: bool,
    #[serde(default)]
    pub menu_animations: bool,
    #[serde(default)]
    pub themes: bool,
    #[serde(default = "default_true")]
    pub bitmap_cache: bool,
    #[serde(default = "default_true")]
    pub auto_reconnect: bool,

    #[serde(default)]
    pub cert: ViewerCertPolicy,
}

fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}

impl Default for ViewerRdpOptions {
    fn default() -> Self {
        Self {
            color_depth: ViewerColorDepth::Auto,
            display_mode: ViewerDisplayMode::Dynamic,
            width: 0,
            height: 0,
            placement: ViewerPlacement::Workspace,
            multi_monitor: false,
            span_monitors: false,
            clipboard: false,
            audio: true,
            microphone: false,
            printers: false,
            smartcard: false,
            network: ViewerNetwork::Auto,
            wallpaper: false,
            font_smoothing: false,
            desktop_composition: false,
            window_drag: false,
            menu_animations: false,
            themes: false,
            bitmap_cache: true,
            auto_reconnect: true,
            cert: ViewerCertPolicy::Ignore,
        }
    }
}

impl ViewerRdpOptions {
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// Build FreeRDP client argv flags (excluding `/v:` `/u:` `/p:`).
    pub fn freerdp_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        args.push(format!("/cert:{}", self.cert.freerdp_value()));
        args.push(format!("/network:{}", self.network.freerdp_value()));

        match self.display_mode {
            ViewerDisplayMode::Dynamic => args.push("/dynamic-resolution".into()),
            ViewerDisplayMode::Fullscreen => args.push("/f".into()),
            ViewerDisplayMode::Windowed => {
                let w = self.width.max(800);
                let h = self.height.max(600);
                args.push(format!("/size:{w}x{h}"));
            }
        }
        if let Some(bpp) = self.color_depth.freerdp_bpp() {
            args.push(format!("/bpp:{bpp}"));
        }
        if self.multi_monitor {
            args.push("/multimon".into());
        }
        if self.span_monitors {
            args.push("/span".into());
        }

        args.push(toggle("+clipboard", "-clipboard", self.clipboard));
        if self.audio {
            args.push("/audio-mode:0".into());
            args.push("+sound".into());
        } else {
            args.push("/audio-mode:2".into());
            args.push("-sound".into());
        }
        args.push(toggle("+microphone", "-microphone", self.microphone));
        args.push(toggle("+printers", "-printers", self.printers));
        args.push(toggle("+smartcard", "-smartcard", self.smartcard));

        args.push(toggle("+wallpaper", "-wallpaper", self.wallpaper));
        args.push(toggle("+fonts", "-fonts", self.font_smoothing));
        args.push(toggle("+aero", "-aero", self.desktop_composition));
        args.push(toggle("+window-drag", "-window-drag", self.window_drag));
        args.push(toggle("+menu-anims", "-menu-anims", self.menu_animations));
        args.push(toggle("+themes", "-themes", self.themes));
        args.push(toggle("+bitmap-cache", "-bitmap-cache", self.bitmap_cache));
        args.push(toggle(
            "+auto-reconnect",
            "-auto-reconnect",
            self.auto_reconnect,
        ));
        args
    }
}

fn toggle(on: &str, off: &str, enabled: bool) -> String {
    if enabled { on } else { off }.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ViewerConfig {
    #[serde(default)]
    pub recent: Vec<ViewerHost>,
}

pub fn viewer_config_path() -> PathBuf {
    super::config_dir().join("viewer.json")
}

pub fn load_viewer_config() -> ViewerConfig {
    let path = viewer_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str(&text) {
            return cfg;
        }
        tracing::warn!("viewer.json parse failed — using defaults");
    }
    ViewerConfig::default()
}

pub fn save_viewer_config(cfg: &ViewerConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(cfg).map_err(std::io::Error::other)?;
    std::fs::write(viewer_config_path(), json)
}

fn same_endpoint(a: &ViewerHost, b: &ViewerHost) -> bool {
    a.host == b.host && a.port == b.port && a.username == b.username
}

/// Push `entry` to the front of recent hosts (dedupe by host+port+user).
/// Preserves an existing label when the new entry's label is empty; keeps prior
/// options when the new entry still has defaults and a prior custom set exists.
pub fn remember_host(entry: ViewerHost) -> std::io::Result<()> {
    let mut cfg = load_viewer_config();
    let prior = cfg
        .recent
        .iter()
        .find(|h| same_endpoint(h, &entry))
        .cloned();
    cfg.recent.retain(|h| !same_endpoint(h, &entry));
    let mut stored = entry;
    if stored.label.is_empty()
        && let Some(p) = &prior
    {
        stored.label = p.label.clone();
    }
    if stored.options.is_default()
        && let Some(p) = &prior
        && !p.options.is_default()
    {
        stored.options = p.options.clone();
    }
    cfg.recent.insert(0, stored);
    if cfg.recent.len() > MAX_RECENT {
        cfg.recent.truncate(MAX_RECENT);
    }
    save_viewer_config(&cfg)
}

/// Remove a recent host matching host+port+user.
pub fn remove_recent(entry: &ViewerHost) -> std::io::Result<()> {
    let mut cfg = load_viewer_config();
    let before = cfg.recent.len();
    cfg.recent.retain(|h| !same_endpoint(h, entry));
    if cfg.recent.len() == before {
        return Ok(());
    }
    save_viewer_config(&cfg)
}

fn viewer_pending_placement_path() -> PathBuf {
    runtime_metis_dir().join("viewer-pending-placement")
}

fn runtime_metis_dir() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/empty/metis-no-xdg-runtime-dir"));
    base.join("metis")
}

/// Stamp the placement preference for the next FreeRDP client window.
pub fn set_viewer_pending_placement(placement: ViewerPlacement) {
    let dir = runtime_metis_dir();
    let _ = std::fs::create_dir_all(&dir);
    let body = match placement {
        ViewerPlacement::Workspace => "workspace",
        ViewerPlacement::Window => "window",
    };
    let path = viewer_pending_placement_path();
    let tmp = path.with_extension("tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Consume the pending FreeRDP placement stamp (defaults to dedicated workspace).
pub fn take_viewer_pending_placement() -> ViewerPlacement {
    let path = viewer_pending_placement_path();
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let _ = std::fs::remove_file(&path);
    match text.trim() {
        "window" => ViewerPlacement::Window,
        _ => ViewerPlacement::Workspace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn remember_and_remove_recent_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("metis-viewer-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("metis")).unwrap();
        // SAFETY: serialized test; restored before unlock.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", &dir) };

        let options = ViewerRdpOptions {
            clipboard: true,
            cert: ViewerCertPolicy::Tofu,
            ..Default::default()
        };
        let entry = ViewerHost {
            host: "192.168.1.10".into(),
            port: 3389,
            username: "alice".into(),
            label: "Home PC".into(),
            options: options.clone(),
        };
        remember_host(entry.clone()).unwrap();
        let cfg = load_viewer_config();
        assert_eq!(cfg.recent.len(), 1);
        assert_eq!(cfg.recent[0], entry);

        remember_host(ViewerHost {
            host: "192.168.1.10".into(),
            port: 3389,
            username: "alice".into(),
            label: String::new(),
            options: ViewerRdpOptions::default(),
        })
        .unwrap();
        let cfg = load_viewer_config();
        assert_eq!(cfg.recent[0].label, "Home PC");
        assert!(cfg.recent[0].options.clipboard);
        assert_eq!(cfg.recent[0].options.cert, ViewerCertPolicy::Tofu);

        remove_recent(&entry).unwrap();
        let cfg = load_viewer_config();
        assert!(cfg.recent.is_empty());

        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn freerdp_args_include_cert_and_toggles() {
        let opts = ViewerRdpOptions {
            clipboard: true,
            wallpaper: true,
            display_mode: ViewerDisplayMode::Windowed,
            width: 1280,
            height: 720,
            cert: ViewerCertPolicy::Deny,
            ..Default::default()
        };
        let args = opts.freerdp_args();
        assert!(args.iter().any(|a| a == "/cert:deny"));
        assert!(args.iter().any(|a| a == "+clipboard"));
        assert!(args.iter().any(|a| a == "+wallpaper"));
        assert!(args.iter().any(|a| a == "/size:1280x720"));
        assert!(!args.iter().any(|a| a == "/dynamic-resolution"));
    }

    #[test]
    fn legacy_viewer_json_without_options_loads() {
        let json = r#"{"recent":[{"host":"h","port":3389,"username":"u","label":""}]}"#;
        let cfg: ViewerConfig = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.recent.len(), 1);
        assert_eq!(cfg.recent[0].options, ViewerRdpOptions::default());
    }

    #[test]
    fn pending_placement_stamp_roundtrip() {
        let _guard = ENV_LOCK.lock().unwrap();
        let dir =
            std::env::temp_dir().join(format!("metis-viewer-placement-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // SAFETY: serialized test; restored before unlock.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", &dir) };

        set_viewer_pending_placement(ViewerPlacement::Window);
        assert_eq!(take_viewer_pending_placement(), ViewerPlacement::Window);
        // Consumed — next take defaults to dedicated workspace.
        assert_eq!(take_viewer_pending_placement(), ViewerPlacement::Workspace);

        set_viewer_pending_placement(ViewerPlacement::Workspace);
        assert_eq!(take_viewer_pending_placement(), ViewerPlacement::Workspace);

        unsafe { std::env::remove_var("XDG_RUNTIME_DIR") };
        let _ = std::fs::remove_dir_all(&dir);
    }
}
