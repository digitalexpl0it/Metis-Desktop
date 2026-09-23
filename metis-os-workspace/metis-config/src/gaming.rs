use serde::{Deserialize, Serialize};

/// How the session routes GPU work on hybrid laptops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsMode {
    /// Desktop on iGPU; games/launchers on dGPU when detected.
    #[default]
    Auto,
    /// Same as `auto` (explicit alias for Settings copy).
    DesktopIgpuGamesDgpu,
    /// Force discrete GPU for all compositor spawns.
    AlwaysDgpu,
    /// Never PRIME-offload; pin clients to the display GPU.
    AlwaysIgpu,
    /// Disable automatic dGPU offload (per-game Steam options still work).
    Off,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameScopeProfile {
    pub steam_app_id: u32,
    #[serde(default)]
    pub args: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GamingConfig {
    #[serde(default)]
    pub graphics_mode: GraphicsMode,
    /// When on battery, skip dGPU offload unless the launch is explicitly a game.
    #[serde(default = "default_true")]
    pub on_battery_prefer_igpu: bool,
    /// Switch to Performance power profile while a game session is active.
    #[serde(default = "default_true")]
    pub auto_performance_profile: bool,
    /// Register GameMode for detected game sessions when `gamemoded` is installed.
    #[serde(default = "default_true")]
    pub auto_gamemode: bool,
    /// Inject NVIDIA/Mesa offload environment into Flatpak gaming app overrides.
    #[serde(default = "default_true")]
    pub flatpak_gpu_env: bool,
    /// Recommend native `.deb` Steam over Flatpak in setup wizards.
    #[serde(default = "default_true")]
    pub steam_prefer_native: bool,
    /// Optional per-title Gamescope launch args (Steam app id → flags).
    /// Deferred: not yet wired to per-app launches (Phase 19 leaves empty).
    #[serde(default)]
    pub gamescope_profiles: Vec<GameScopeProfile>,
    /// Extra host directories granted to Flatpak Steam via `--filesystem`
    /// (canonical paths under `$HOME`, `/mnt`, `/media`, or `/run/media`).
    #[serde(default)]
    pub extra_steam_paths: Vec<String>,
    /// When Metis launches Steam / Big Picture and `mangohud` is on PATH, set
    /// `MANGOHUD=1` for that spawn only (never writes Steam Properties).
    #[serde(default)]
    pub mangohud_for_games: bool,
    /// When Metis launches Big Picture and `gamescope` is on PATH, prefix
    /// `gamescope --` for that spawn only.
    #[serde(default)]
    pub gamescope_big_picture: bool,
}

fn default_true() -> bool {
    true
}

impl Default for GamingConfig {
    fn default() -> Self {
        Self {
            graphics_mode: GraphicsMode::default(),
            on_battery_prefer_igpu: true,
            auto_performance_profile: true,
            auto_gamemode: true,
            flatpak_gpu_env: true,
            steam_prefer_native: true,
            gamescope_profiles: Vec::new(),
            extra_steam_paths: Vec::new(),
            mangohud_for_games: false,
            gamescope_big_picture: false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GamingFlatpakState {
    #[serde(default)]
    pub optimized_apps: Vec<String>,
    #[serde(default)]
    pub last_run_unix: Option<u64>,
}

pub fn gaming_config_path() -> std::path::PathBuf {
    super::config_dir().join("gaming.json")
}

pub fn gaming_flatpak_state_path() -> std::path::PathBuf {
    super::config_dir().join("gaming-flatpak.json")
}

pub fn load_gaming_config() -> GamingConfig {
    let path = gaming_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return sanitize(cfg);
    }
    GamingConfig::default()
}

pub fn save_gaming_config(cfg: &GamingConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json =
        serde_json::to_string_pretty(&sanitize(cfg.clone())).map_err(std::io::Error::other)?;
    std::fs::write(gaming_config_path(), json)
}

pub fn save_default_gaming_config() -> std::io::Result<()> {
    let path = gaming_config_path();
    if path.exists() {
        return Ok(());
    }
    save_gaming_config(&GamingConfig::default())
}

pub fn load_gaming_flatpak_state() -> GamingFlatpakState {
    let path = gaming_flatpak_state_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(state) = serde_json::from_str(&text)
    {
        return state;
    }
    GamingFlatpakState::default()
}

pub fn save_gaming_flatpak_state(state: &GamingFlatpakState) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(state).map_err(std::io::Error::other)?;
    std::fs::write(gaming_flatpak_state_path(), json)
}

fn sanitize(cfg: GamingConfig) -> GamingConfig {
    let mut out = cfg;
    let mut paths = Vec::new();
    for raw in out.extra_steam_paths.drain(..) {
        match crate::gaming_paths::validate_steam_library_path(&raw) {
            Some(canon) => {
                let s = canon.to_string_lossy().into_owned();
                if !paths.contains(&s) {
                    paths.push(s);
                }
            }
            None => {
                tracing::warn!(
                    path = %raw,
                    "rejected gaming.json extra_steam_paths entry (fail-closed)"
                );
            }
        }
    }
    out.extra_steam_paths = paths;
    out
}

/// Whether `program` looks like a game or game launcher (shared with compositor).
pub fn command_prefers_dgpu(program: &str) -> bool {
    let p = program.to_ascii_lowercase();
    const NEEDLES: &[&str] = &[
        "steam",
        "gamepadui",
        "gamescope",
        "lutris",
        "heroic",
        "bottles",
        "proton",
        "wine",
        ".exe",
        "mangohud",
        "gamemoderun",
        "steam_app_",
    ];
    NEEDLES.iter().any(|n| p.contains(n))
}

/// Whether `program` looks like a web browser (Chrome / Firefox / Edge / …).
///
/// Browsers are steered onto the dGPU in Auto mode (when on AC) so WebGL / canvas
/// games are not stuck on a weak iGPU. They do **not** count as a "game session"
/// for GameMode / performance-profile hooks.
pub fn command_is_web_browser(program: &str) -> bool {
    let p = program.to_ascii_lowercase();
    if p.contains("chromedriver")
        || p.contains("electron")
        || p.contains("/cursor")
        || p.contains("cursor-")
        || p.contains("code -")
        || p.contains("/code ")
        || p.contains("slack")
        || p.contains("discord")
    {
        return false;
    }
    // Flatpak / desktop-id style.
    if p.contains("org.mozilla.firefox")
        || p.contains("com.google.chrome")
        || p.contains("com.brave.browser")
        || p.contains("com.microsoft.edge")
        || p.contains("org.chromium.chromium")
        || p.contains("com.opera.opera")
        || p.contains("com.vivaldi.vivaldi")
    {
        return true;
    }
    let base = p
        .rsplit(['/', ' '])
        .next()
        .unwrap_or(p.as_str())
        .trim_end_matches(".desktop");
    const BROWSERS: &[&str] = &[
        "firefox",
        "google-chrome",
        "chromium",
        "chromium-browser",
        "brave",
        "brave-browser",
        "microsoft-edge",
        "msedge",
        "vivaldi",
        "vivaldi-bin",
        "opera",
        "chrome",
    ];
    BROWSERS.iter().any(|b| {
        base == *b || base.starts_with(&format!("{b}-")) || base.starts_with(&format!("{b}."))
    })
}

/// Resolve whether a compositor spawn should prefer the discrete GPU.
pub fn prefer_dgpu_for_launch(program: &str, cfg: &GamingConfig) -> bool {
    if let Ok(v) = std::env::var("METIS_GAME_GPU") {
        return match v.as_str() {
            "dgpu" => true,
            "igpu" | "off" => false,
            _ => resolve_graphics_mode(program, cfg),
        };
    }
    resolve_graphics_mode(program, cfg)
}

fn resolve_graphics_mode(program: &str, cfg: &GamingConfig) -> bool {
    match cfg.graphics_mode {
        GraphicsMode::Off | GraphicsMode::AlwaysIgpu => false,
        GraphicsMode::AlwaysDgpu => true,
        GraphicsMode::Auto | GraphicsMode::DesktopIgpuGamesDgpu => {
            let wants_dgpu = command_prefers_dgpu(program) || command_is_web_browser(program);
            if !wants_dgpu {
                return false;
            }
            if cfg.on_battery_prefer_igpu && on_battery() {
                return false;
            }
            true
        }
    }
}

/// Best-effort: laptop on battery (not charging / full).
pub fn on_battery() -> bool {
    let Ok(dir) = std::fs::read_dir("/sys/class/power_supply") else {
        return false;
    };
    for entry in dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("BAT") {
            continue;
        }
        if let Ok(status) = std::fs::read_to_string(entry.path().join("status")) {
            let s = status.trim();
            return s != "Charging" && s != "Full" && s != "Not charging";
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_web_browsers() {
        assert!(command_is_web_browser("firefox"));
        assert!(command_is_web_browser("/usr/bin/google-chrome-stable"));
        assert!(command_is_web_browser("chromium-browser"));
        assert!(command_is_web_browser("flatpak run com.google.Chrome"));
        assert!(command_is_web_browser("brave-browser"));
        assert!(!command_is_web_browser("cursor"));
        assert!(!command_is_web_browser("/usr/share/cursor/chrome-sandbox"));
        assert!(!command_is_web_browser("steam"));
        assert!(!command_is_web_browser("chromedriver"));
    }

    #[test]
    fn browsers_prefer_dgpu_in_auto() {
        let cfg = GamingConfig {
            graphics_mode: GraphicsMode::Auto,
            on_battery_prefer_igpu: false,
            ..GamingConfig::default()
        };
        assert!(prefer_dgpu_for_launch("firefox", &cfg));
        assert!(prefer_dgpu_for_launch("google-chrome-stable", &cfg));
        assert!(!prefer_dgpu_for_launch("gedit", &cfg));
        // Games still prefer dGPU.
        assert!(prefer_dgpu_for_launch("steam", &cfg));
    }
}
