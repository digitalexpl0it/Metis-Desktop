//! Shared Metis configuration: pure serde + filesystem types consumed by both the
//! shell (`metis-shell`) and the settings app (`metis-settings`). No GTK here so
//! the settings binary can link it without pulling in the shell.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod bar;
pub mod calendars;
pub mod clocks;
pub mod css;
pub mod dashboard;
pub mod datetime;
pub mod decorations;
pub mod desktop_widgets;
pub mod game_rules;
pub mod gaming;
pub mod gaming_paths;
pub mod gpu_offload;
pub mod graphics;
pub mod input;
pub mod keybinds;
pub mod kitty;
pub mod launch_tweaks;
pub mod locale;
pub mod lock;
pub mod menu;
pub mod outputs;
pub mod power;
pub mod remote;
pub mod reset;
pub mod sanitize;
pub mod screenshot;
pub mod startup;
pub mod theme;
pub mod updates;
pub mod viewer;
pub mod wallpaper;
pub mod weather;
pub mod widget_ext;
pub mod xwayland_policy;

use serde::{Deserialize, Serialize};

// `ThemeMode` is re-exported below via `pub use theme::{...}`, which also brings it
// into this module's scope for the path helpers.

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default)]
    pub onboarding_complete: bool,
    /// Current wizard step (0-based) while [`Self::onboarding_complete`] is false.
    /// Restored after a mid-wizard session restart; cleared on finish/skip.
    #[serde(default)]
    pub onboarding_step: u32,
    #[serde(default)]
    pub gaming_setup_complete: bool,
    #[serde(default = "default_show_briefing")]
    pub show_briefing_on_login: bool,
    /// Session UI graphics profile (Auto / Compatibility / Normal). Independent of
    /// Gaming's PRIME `graphics_mode`.
    #[serde(default)]
    pub graphics_profile: graphics::GraphicsProfile,
    /// When true, XWayland also listens on the abstract Unix socket
    /// (`@/tmp/.X11-unix/...`). Default false for slightly tighter local exposure;
    /// one shared X11 server remains either way unless `xwayland_mode` is isolated.
    #[serde(default = "default_false")]
    pub xwayland_abstract_socket: bool,
    /// X11 isolation mode (Phase 15 §E / 18 D). `shared` (default) = one XWayland
    /// for all X11 clients. `isolated` = soft second bucket for gaming-class
    /// launches (lazy-spawned). Not a sandbox — same-UID can still open either
    /// display.
    #[serde(default)]
    pub xwayland_mode: XwaylandMode,
    /// Which launches use the gaming XWayland bucket when isolated (Phase 18 D).
    #[serde(default)]
    pub xwayland_policy: xwayland_policy::XwaylandPolicy,
}

/// How many XWayland servers Metis runs (Phase 15 §E).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum XwaylandMode {
    #[default]
    Shared,
    Isolated,
}

fn default_theme() -> String {
    "dark".into()
}

fn default_show_briefing() -> bool {
    true
}

fn default_false() -> bool {
    false
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            onboarding_complete: false,
            onboarding_step: 0,
            gaming_setup_complete: false,
            show_briefing_on_login: default_show_briefing(),
            graphics_profile: graphics::GraphicsProfile::default(),
            xwayland_abstract_socket: default_false(),
            xwayland_mode: XwaylandMode::default(),
            xwayland_policy: xwayland_policy::XwaylandPolicy::default(),
        }
    }
}

fn sanitize_app_config(mut cfg: AppConfig) -> AppConfig {
    cfg.xwayland_policy = cfg.xwayland_policy.sanitize();
    cfg
}

pub fn config_dir() -> std::path::PathBuf {
    // On Linux, ProjectDirs uses only the `application` component for the path,
    // so `application = "metis"` yields ~/.config/metis (the documented location).
    directories::ProjectDirs::from("com", "metis", "metis")
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| std::path::PathBuf::from(h).join(".config/metis"))
                .unwrap_or_else(|_| std::path::PathBuf::from(".config/metis"))
        })
}

pub fn ensure_config_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(config_dir().join("themes"))?;
    Ok(())
}

pub fn app_config_path() -> std::path::PathBuf {
    config_dir().join("config.json")
}

pub fn desk_config_path() -> std::path::PathBuf {
    config_dir().join("desk.json")
}

pub fn briefing_config_path() -> std::path::PathBuf {
    config_dir().join("briefing.json")
}

pub fn theme_file_path(mode: &ThemeMode) -> std::path::PathBuf {
    theme_file_path_for_name(match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => "system",
    })
}

pub fn theme_file_path_for_name(name: &str) -> std::path::PathBuf {
    config_dir().join("themes").join(format!("{name}.json"))
}

pub fn load_app_config() -> AppConfig {
    let path = app_config_path();
    if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str(&text) {
                return sanitize_app_config(cfg);
            }
        }
    }
    AppConfig::default()
}

pub fn save_app_config(config: &AppConfig) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let path = app_config_path();
    let config = sanitize_app_config(config.clone());
    let json = serde_json::to_string_pretty(&config).map_err(std::io::Error::other)?;
    // Atomic replace so a partial write cannot leave a corrupt file.
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            std::io::Error::new(
                e.kind(),
                format!(
                    "permission denied writing {} — is the file owned by root? \
                     Run: sudo chown -R \"$USER:$USER\" ~/.config/metis",
                    path.display()
                ),
            )
        } else {
            e
        }
    })?;
    std::fs::rename(&tmp, &path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            std::io::Error::new(
                e.kind(),
                format!(
                    "permission denied replacing {} — is it owned by root? \
                     Run: sudo chown -R \"$USER:$USER\" ~/.config/metis",
                    path.display()
                ),
            )
        } else {
            e
        }
    })
}

pub fn load_theme_preference() -> Option<ThemeMode> {
    let cfg = load_app_config();
    match cfg.theme.as_str() {
        "light" => Some(ThemeMode::Light),
        "system" => Some(ThemeMode::System),
        _ => Some(ThemeMode::Dark),
    }
}

pub fn save_theme_preference(mode: ThemeMode) -> std::io::Result<()> {
    let mut cfg = load_app_config();
    cfg.theme = match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => "system",
    }
    .into();
    let result = save_app_config(&cfg);
    // Always stamp the chosen mode so apps (Metis Viewer) can follow Appearance
    // even when config.json is momentarily unwritable.
    let _ = write_appearance_mode_stamp(mode);
    result
}

/// Sidecar written on every Appearance Light/Dark click (`appearance.mode`).
pub fn write_appearance_mode_stamp(mode: ThemeMode) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let label = match mode {
        ThemeMode::Light => "light",
        ThemeMode::Dark => "dark",
        ThemeMode::System => "system",
    };
    let path = config_dir().join("appearance.mode");
    let tmp = path.with_extension("mode.tmp");
    std::fs::write(&tmp, format!("{label}\n"))?;
    std::fs::rename(tmp, path)
}

/// Read [`write_appearance_mode_stamp`], if present.
pub fn load_appearance_mode_stamp() -> Option<ThemeMode> {
    let path = config_dir().join("appearance.mode");
    let text = std::fs::read_to_string(path).ok()?;
    match text.trim() {
        "light" => Some(ThemeMode::Light),
        "dark" => Some(ThemeMode::Dark),
        "system" => Some(ThemeMode::System),
        _ => None,
    }
}

/// Preference for UI theming: stamp (latest Appearance click) then `config.json`.
pub fn load_theme_preference_for_ui() -> ThemeMode {
    load_appearance_mode_stamp()
        .or_else(load_theme_preference)
        .unwrap_or(ThemeMode::Dark)
}

pub fn load_graphics_profile() -> graphics::GraphicsProfile {
    load_app_config().graphics_profile
}

pub fn save_graphics_profile(profile: graphics::GraphicsProfile) -> std::io::Result<()> {
    let mut cfg = load_app_config();
    cfg.graphics_profile = profile;
    save_app_config(&cfg)
}

/// `color-scheme` / `gtk-theme` values for `org.gnome.desktop.interface`.
///
/// Libadwaita follows `color-scheme`. Older GTK / `xdg-desktop-portal-gtk`
/// (FileChooser, etc.) still key off the `gtk-theme` name — keep `Adwaita-dark`
/// for dark mode so portal dialogs match Metis instead of opening light Adwaita.
pub fn appearance_gsettings_values(mode: ThemeMode) -> (&'static str, &'static str) {
    match mode {
        ThemeMode::Dark => ("prefer-dark", "Adwaita-dark"),
        ThemeMode::Light => ("prefer-light", "Adwaita"),
        ThemeMode::System => ("default", "Adwaita"),
    }
}

/// `GTK_THEME` for spawned clients (`Adwaita:dark` / `Adwaita`). Helps GTK4 apps
/// that do not yet read the Settings portal.
pub fn appearance_gtk_theme_env(mode: ThemeMode) -> Option<&'static str> {
    match mode {
        ThemeMode::Dark => Some("Adwaita:dark"),
        ThemeMode::Light => Some("Adwaita"),
        ThemeMode::System => None,
    }
}

/// GNOME WM preference for CSD traffic lights (`org.gnome.desktop.wm.preferences`).
///
/// Fresh Ubuntu images (and some greeter leftovers) ship `appmenu:close`, which
/// hides minimize/maximize on Firefox, Chromium, and GTK headerbars. Metis owns
/// the session, so we normalize to the full Ubuntu/GNOME layout.
pub const SESSION_WM_BUTTON_LAYOUT: &str = "appmenu:minimize,maximize,close";

/// GTK decoration layout string (`org.gnome.desktop.interface gtk-decoration-layout`
/// when the schema key exists) and the Settings portal value for the same idea.
pub const SESSION_GTK_DECORATION_LAYOUT: &str = "icon:minimize,maximize,close";

/// Best-effort sync so non-Metis GTK / browser CSD follows Metis light/dark and
/// shows minimize + maximize + close (not close-only).
pub fn apply_session_appearance_gsettings(mode: ThemeMode) {
    let (scheme, gtk_theme) = appearance_gsettings_values(mode);
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "color-scheme", scheme])
        .status();
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "gtk-theme", gtk_theme])
        .status();
    let _ = std::process::Command::new("gsettings")
        .args([
            "set",
            "org.gnome.desktop.wm.preferences",
            "button-layout",
            SESSION_WM_BUTTON_LAYOUT,
        ])
        .status();
    // Present on full GNOME schema installs; missing on some minimal images.
    let _ = std::process::Command::new("gsettings")
        .args([
            "set",
            "org.gnome.desktop.interface",
            "gtk-decoration-layout",
            SESSION_GTK_DECORATION_LAYOUT,
        ])
        .status();
}

/// Apply gsettings from the persisted theme preference (session start / portal).
pub fn sync_session_appearance_from_config() {
    let mode = load_theme_preference().unwrap_or(ThemeMode::Dark);
    apply_session_appearance_gsettings(mode);
}

pub fn mark_onboarding_complete() -> std::io::Result<()> {
    let mut cfg = load_app_config();
    cfg.onboarding_complete = true;
    cfg.onboarding_step = 0;
    save_app_config(&cfg)
}

/// Persist the in-progress wizard step (no-op once onboarding is complete).
pub fn save_onboarding_step(step: u32) -> std::io::Result<()> {
    let mut cfg = load_app_config();
    if cfg.onboarding_complete {
        return Ok(());
    }
    if cfg.onboarding_step == step {
        return Ok(());
    }
    cfg.onboarding_step = step;
    save_app_config(&cfg)
}

/// Open (or re-open) the wizard from step 0 — used by Settings "Run setup again".
pub fn reset_onboarding_progress() -> std::io::Result<()> {
    let mut cfg = load_app_config();
    cfg.onboarding_complete = false;
    cfg.onboarding_step = 0;
    save_app_config(&cfg)
}

/// Clamp a saved step into `0..step_count` for resume.
pub fn clamped_onboarding_step(step_count: usize) -> usize {
    if step_count == 0 {
        return 0;
    }
    let step = load_app_config().onboarding_step as usize;
    step.min(step_count - 1)
}

pub fn mark_gaming_setup_complete() -> std::io::Result<()> {
    let mut cfg = load_app_config();
    cfg.gaming_setup_complete = true;
    save_app_config(&cfg)
}

/// Persist a theme token set to `themes/<name>.json` (used by the settings app's
/// Appearance page). The shell's file watcher re-applies it live.
pub fn save_theme_tokens(name: &str, tokens: &theme::ThemeTokens) -> std::io::Result<()> {
    ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(tokens).map_err(std::io::Error::other)?;
    std::fs::write(theme_file_path_for_name(name), json)
}

/// Load a theme token set from `themes/<name>.json`, falling back to the embedded
/// default for that name (dark/light) when missing or unparsable.
pub fn load_theme_tokens(name: &str) -> theme::ThemeTokens {
    let path = theme_file_path_for_name(name);
    if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(tokens) = serde_json::from_str(&text) {
                return tokens;
            }
        }
    }
    match name {
        "light" => theme::ThemeTokens::light_default(),
        _ => theme::ThemeTokens::dark_default(),
    }
}

pub use bar::{
    bar_config_path, load_bar_config, sanitize_bar_config, save_bar_config,
    save_default_bar_config, BarBorder, BarConfig, BarDisplays, BarFill, BarFillMode,
    BarGradientDirection, BarPosition, BarWidgetId, BorderMode, ClockConfig, DefaultLayout,
    TitlebarPillBorder, TrayIconMode, WindowBorder, WorkspaceMode,
};
pub use calendars::{
    calendars_config_path, default_local_dir, load_calendars_config, save_calendars_config,
    AccountKind, CalendarAccount, CalendarsConfig,
};
pub use clocks::{
    alarm_sound_canberra_id, clocks_config_path, load_clocks_config, save_clocks_config, Alarm,
    AlarmSound, ClocksConfig, ALARM_SOUNDS,
};
pub use css::build_stylesheet;
pub use dashboard::{
    dashboard_config_path, load_dashboard_config, process_monitor_needs_terminal,
    save_dashboard_config, save_default_dashboard_config, DashboardConfig, DashboardWidgetId,
    KNOWN_PROCESS_MONITORS,
};
pub use datetime::{
    datetime_config_path, load_datetime_config, save_datetime_config, DateTimeConfig,
    FirstDayOfWeek,
};
pub use decorations::{
    decorations_config_path, load_decorations_config, save_decorations_config,
    save_default_decorations_config, DecorationsConfig, DecorationsOverride,
};
pub use desktop_widgets::{
    desktop_widgets_config_path, load_desktop_widgets_config, save_desktop_widgets_config,
    DesktopWidgetChrome, DesktopWidgetChromeOverride, DesktopWidgetInstance, DesktopWidgetKind,
    DesktopWidgetView, DesktopWidgetsConfig, EqualizerBarShape, EqualizerColorMode,
    EqualizerVizStyle, ResolvedDesktopWidgetChrome,
};
pub use game_rules::{
    game_rules_config_path, load_game_rules_config, save_game_rules_config, GameRulesConfig,
    WindowRule, WindowRuleOutcome,
};
pub use gaming::{
    command_is_web_browser, command_prefers_dgpu, gaming_config_path, gaming_flatpak_state_path,
    load_gaming_config, load_gaming_flatpak_state, on_battery, prefer_dgpu_for_launch,
    save_default_gaming_config, save_gaming_config, save_gaming_flatpak_state, GameScopeProfile,
    GamingConfig, GamingFlatpakState, GraphicsMode,
};
pub use gaming_paths::{
    flatpak_env_arg, sanitize_offload_env, sanitize_offload_env_pair, shell_export_line,
    validate_steam_library_path, OFFLOAD_ENV_KEY_ALLOWLIST,
};
pub use gpu_offload::{
    detect_hybrid_gpu, display_gpu_pci, offload_env_vars, GpuOffloadKind, HybridGpuInfo,
};
pub use graphics::{
    effective_graphics_compatibility, effective_graphics_profile_label, is_virtual_machine,
    session_graphics_compatibility, GraphicsProfile,
};
pub use input::{
    input_config_path, load_input_config, save_input_config, AccelProfile, CapsLockBehavior,
    ComposeKey, InputConfig, KeyboardConfig, MouseConfig, NumLockStartup, TouchpadConfig,
};
pub use keybinds::{
    default_chord, keybinds_config_path, load_keybinds_config, reserved_chords,
    reserved_system_rows, save_default_keybinds_config, save_keybinds_config, Chord, KeybindAction,
    KeybindGroup, KeybindsConfig, ModKey,
};
pub use kitty::{ensure_kitty_defaults, kitty_config_path, KITTY_DEFAULT_CONF};
pub use launch_tweaks::{apply_steam_launch_tweaks, LaunchTweaks};
pub use locale::{load_locale_config, locale_config_path, save_locale_config, LocaleConfig};
pub use lock::{
    load_lock_config, lock_config_path, save_lock_config, LockBackgroundSource, LockConfig,
};
pub use menu::{
    argv_in_terminal, binary_in_path, load_menu_config, menu_config_path, resolve_executable,
    resolve_file_manager, resolve_terminal, resolve_user_avatar_path, save_menu_config, MenuConfig,
    MenuStyle, KNOWN_FILE_MANAGERS, KNOWN_TERMINALS,
};
pub use outputs::{
    format_schedule_hhmm, format_schedule_minutes, load_outputs_config,
    load_outputs_config_with_fallback, minutes_to_hhmm, night_light_effective, output_prefs,
    outputs_config_path, parse_hhmm, parse_schedule_input, save_outputs_config,
    schedule_half_hour_presets, DisplayLayoutMode, NightLightSchedule, OutputPrefs, OutputsConfig,
};
pub use power::{
    load_power_config, power_config_path, save_power_config, LidCloseAction, PowerConfig,
    PowerProfile,
};
pub use remote::{
    load_remote_config, remote_config_path, save_remote_config, RemoteBackend, RemoteConfig,
};
pub use reset::{reset_metis_config, reset_metis_config_at, ResetOptions, ResetResult};
pub use sanitize::{is_safe_nm_token, validate_nm_id, validate_ssid, validate_vpn_data_fragment};
pub use screenshot::{
    expand_save_dir, load_screenshot_config, save_default_screenshot_config,
    save_screenshot_config, screenshot_config_path, AfterCaptureAction, ScreenshotConfig,
    ScreenshotMode,
};
pub use startup::{
    load_startup_config, resolve_desktop_launch_argv, sanitize_startup_config, save_startup_config,
    startup_config_path, StartupConfig, StartupEntry,
};
pub use theme::{SemanticColors, ThemeMode, ThemeTokens};
pub use updates::{
    load_updates_config, sanitize_updates_config, save_updates_config, updates_config_path,
    UpdateSources, UpdatesConfig,
};
pub use viewer::{
    load_viewer_config, remember_host, remove_recent, save_viewer_config, viewer_config_path,
    ViewerConfig, ViewerHost,
};
pub use wallpaper::{
    bundled_wallpaper_dir, bundled_wallpaper_dirs, collect_wallpaper_images,
    collect_wallpaper_images_depth, default_wallpaper_path, list_bundled_wallpapers,
    load_wallpaper_config, load_wallpaper_rgba_cache, parse_hex_rgb, save_wallpaper_config,
    store_wallpaper_rgba_cache, system_wallpaper_dirs, wallpaper_config_path,
    wallpaper_rgba_cache_fresh, wallpaper_rgba_cache_path, wallpaper_store_dir, BackgroundKind,
    GradientDirection, WallpaperConfig, WALLPAPER_IMAGE_EXTS,
};
pub use weather::{
    load_weather_config, save_weather_config, weather_config_path, TempUnit, WeatherConfig,
    WeatherLocation,
};
pub use widget_ext::{
    default_extension_settings, discover_widget_extensions, discover_widget_extensions_detailed,
    find_widget_extension, interpolate_settings, interpolate_template, is_safe_icon_name,
    is_safe_launch_exec, is_safe_launch_id, is_safe_open_uri, is_valid_extension_id,
    load_widget_extension, load_widget_layout, resolve_helper_exec, run_helper_snapshot,
    template_needs_host, validate_action, validate_widget_layout, widget_ext_search_dirs,
    DiscoveredWidgetExt, HostBindValues, WidgetExtAction, WidgetExtDiscoverResult, WidgetExtHelper,
    WidgetExtLabelStyle, WidgetExtManifest, WidgetExtNode, WidgetExtSetting, WidgetExtSettingType,
    WIDGET_EXT_API, WIDGET_EXT_HELPER_MAX_STDOUT, WIDGET_EXT_HELPER_TIMEOUT_SECS,
    WIDGET_EXT_MAX_COPY, WIDGET_EXT_MAX_DEPTH, WIDGET_EXT_MAX_JSON_BYTES, WIDGET_EXT_MAX_NODES,
    WIDGET_EXT_MAX_STRING,
};
pub use xwayland_policy::{
    command_uses_gaming_xwayland, XwaylandPolicy, DEFAULT_GAMING_XWAYLAND_PATTERNS,
};
