use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Layout preset for the Metis app menu. `Default` is today's Metis menu
/// (utility/power rail + Frequent/search + Pinned grid).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MenuStyle {
    /// Metis default — rail | Frequent + search | Pinned.
    #[default]
    Default,
    /// Search on top; rail + app list (no pinned column).
    Whisker,
    /// Optional user header above the usual Metis columns.
    ArcMenu,
    /// Search on top spanning the content; rail + list + pinned.
    Mint,
    /// Category list beside the app list (Brisk-inspired).
    Bracket,
    /// Single tall app list with search (GNOME list-inspired).
    Ledger,
    /// Icon tile grid with search (Elementary-inspired).
    Mosaic,
    /// Utility rail, wide center, pinned column (Dash-inspired).
    Ramp,
    /// Category column cascading into an app column.
    Ladder,
    /// Search + icon grid + footer user/power row (Start-menu inspired).
    Plaza,
    /// Large centered avatar header + icon grid.
    Crest,
    /// Dense small-icon grid with search on top.
    Chip,
}

impl MenuStyle {
    pub const ALL: &[MenuStyle] = &[
        MenuStyle::Default,
        MenuStyle::Whisker,
        MenuStyle::ArcMenu,
        MenuStyle::Mint,
        MenuStyle::Bracket,
        MenuStyle::Ledger,
        MenuStyle::Mosaic,
        MenuStyle::Ramp,
        MenuStyle::Ladder,
        MenuStyle::Plaza,
        MenuStyle::Crest,
        MenuStyle::Chip,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            MenuStyle::Default => "default",
            MenuStyle::Whisker => "whisker",
            MenuStyle::ArcMenu => "arc_menu",
            MenuStyle::Mint => "mint",
            MenuStyle::Bracket => "bracket",
            MenuStyle::Ledger => "ledger",
            MenuStyle::Mosaic => "mosaic",
            MenuStyle::Ramp => "ramp",
            MenuStyle::Ladder => "ladder",
            MenuStyle::Plaza => "plaza",
            MenuStyle::Crest => "crest",
            MenuStyle::Chip => "chip",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            MenuStyle::Default => "Metis",
            MenuStyle::Whisker => "Whisker",
            MenuStyle::ArcMenu => "ArcMenu",
            MenuStyle::Mint => "Mint",
            MenuStyle::Bracket => "Bracket",
            MenuStyle::Ledger => "Ledger",
            MenuStyle::Mosaic => "Mosaic",
            MenuStyle::Ramp => "Ramp",
            MenuStyle::Ladder => "Ladder",
            MenuStyle::Plaza => "Plaza",
            MenuStyle::Crest => "Crest",
            MenuStyle::Chip => "Chip",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            MenuStyle::Default => {
                "Classic Metis menu: quick rail, Frequent Apps with search, and Pinned."
            }
            MenuStyle::Whisker => {
                "Search on top, places rail beside the app list (no pinned column)."
            }
            MenuStyle::ArcMenu => "Metis columns with an optional user header on top.",
            MenuStyle::Mint => "Search across the top; rail, apps, and pinned below.",
            MenuStyle::Bracket => "Categories on the left, apps on the right.",
            MenuStyle::Ledger => "A single searchable list of every installed app.",
            MenuStyle::Mosaic => "Browse apps as a large icon grid with search on top.",
            MenuStyle::Ramp => "Utility rail, wide app list, and pinned apps on the right.",
            MenuStyle::Ladder => "Pick a category, then browse apps in the next column.",
            MenuStyle::Plaza => "Search, icon grid, and a footer with profile and power.",
            MenuStyle::Crest => "Centered profile header above an icon grid.",
            MenuStyle::Chip => "Compact icon grid with search — dense and fast.",
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            MenuStyle::Default => "metis-menu-style-default",
            MenuStyle::Whisker => "metis-menu-style-whisker",
            MenuStyle::ArcMenu => "metis-menu-style-arc",
            MenuStyle::Mint => "metis-menu-style-mint",
            MenuStyle::Bracket => "metis-menu-style-bracket",
            MenuStyle::Ledger => "metis-menu-style-ledger",
            MenuStyle::Mosaic => "metis-menu-style-mosaic",
            MenuStyle::Ramp => "metis-menu-style-ramp",
            MenuStyle::Ladder => "metis-menu-style-ladder",
            MenuStyle::Plaza => "metis-menu-style-plaza",
            MenuStyle::Crest => "metis-menu-style-crest",
            MenuStyle::Chip => "metis-menu-style-chip",
        }
    }

    /// Search field sits above the body (not under the Frequent list).
    pub fn search_on_top(self) -> bool {
        matches!(
            self,
            MenuStyle::Whisker
                | MenuStyle::Mint
                | MenuStyle::Bracket
                | MenuStyle::Ledger
                | MenuStyle::Mosaic
                | MenuStyle::Ladder
                | MenuStyle::Plaza
                | MenuStyle::Crest
                | MenuStyle::Chip
        )
    }

    /// Layout never shows the pinned column (ignores `show_pinned`).
    pub fn hides_pinned(self) -> bool {
        matches!(
            self,
            MenuStyle::Whisker
                | MenuStyle::Bracket
                | MenuStyle::Ledger
                | MenuStyle::Mosaic
                | MenuStyle::Ladder
                | MenuStyle::Plaza
                | MenuStyle::Crest
                | MenuStyle::Chip
        )
    }

    /// Uses Freedesktop category browsing instead of Frequent/alpha list.
    pub fn uses_categories(self) -> bool {
        matches!(self, MenuStyle::Bracket | MenuStyle::Ladder)
    }

    /// Main apps surface is an icon grid (not a list).
    pub fn uses_app_grid(self) -> bool {
        matches!(
            self,
            MenuStyle::Mosaic | MenuStyle::Plaza | MenuStyle::Crest | MenuStyle::Chip
        )
    }

    /// Dense chip-sized tiles.
    pub fn dense_grid(self) -> bool {
        matches!(self, MenuStyle::Chip)
    }

    /// Force a user header (Crest / Plaza footer still builds its own chrome).
    pub fn prefers_user_header(self) -> bool {
        matches!(self, MenuStyle::Crest | MenuStyle::ArcMenu)
    }

    /// Classic three-column rail | list | pinned family (including Ramp).
    pub fn classic_columns(self) -> bool {
        matches!(
            self,
            MenuStyle::Default
                | MenuStyle::Whisker
                | MenuStyle::ArcMenu
                | MenuStyle::Mint
                | MenuStyle::Ramp
        )
    }
}

/// Persistent app-menu state, stored at `~/.config/metis/menu.json`.
///
/// `pinned` is an ordered list of `.desktop` ids shown in the menu's pinned grid.
/// `launch_counts` tracks how often each app id has been launched from the menu,
/// driving the "Frequent Apps" ordering.
///
/// `terminal` / `file_manager` are the user's chosen quick-launch programs for the
/// menu rail. Each is a binary name on `$PATH` *or* an absolute path. `None` or an
/// empty string means "auto-detect" — fall back to the first installed entry in
/// [`KNOWN_TERMINALS`] / [`KNOWN_FILE_MANAGERS`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenuConfig {
    #[serde(default)]
    pub pinned: Vec<String>,
    #[serde(default)]
    pub launch_counts: HashMap<String, u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_manager: Option<String>,
    /// Layout preset. Missing / unknown → Metis [`MenuStyle::Default`].
    #[serde(default)]
    pub style: MenuStyle,
    /// Show avatar + display name in styles that have a header slot (and on
    /// Default when enabled — header sits above the Frequent column).
    #[serde(default)]
    pub show_user_header: bool,
    /// Show the left utility / power icon rail.
    #[serde(default = "default_true")]
    pub show_rail: bool,
    /// Show the Pinned apps column.
    #[serde(default = "default_true")]
    pub show_pinned: bool,
    /// Optional display-name override; empty / missing → GECOS / `$USER`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_display_name: Option<String>,
    /// Optional avatar image path; empty / missing → DE face files when present
    /// (`~/.face`, `~/.face.icon`, AccountsService icon).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar_path: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for MenuConfig {
    fn default() -> Self {
        Self {
            pinned: Vec::new(),
            launch_counts: HashMap::new(),
            terminal: None,
            file_manager: None,
            style: MenuStyle::Default,
            show_user_header: false,
            show_rail: true,
            show_pinned: true,
            user_display_name: None,
            avatar_path: None,
        }
    }
}

impl MenuConfig {
    /// Resolve the name shown in the menu user header.
    pub fn resolved_display_name(&self) -> String {
        if let Some(name) = self
            .user_display_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return name.to_string();
        }
        if let Some(gecos) = gecos_full_name() {
            return gecos;
        }
        std::env::var("USER").unwrap_or_else(|_| "User".into())
    }

    /// Avatar file to load, if any.
    pub fn resolved_avatar_path(&self) -> Option<PathBuf> {
        if let Some(p) = self
            .avatar_path
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let path = PathBuf::from(p);
            if path.is_file() {
                return Some(path);
            }
        }
        let home = directories::UserDirs::new()
            .map(|u| u.home_dir().to_path_buf())
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))?;
        let username = std::env::var("USER").unwrap_or_default();
        resolve_user_avatar_path(&username, &home)
    }
}

/// Locate a user's avatar from common desktop-environment locations.
///
/// Order: `~/.face` → `~/.face.icon` → AccountsService
/// (`/var/lib/AccountsService/icons/<username>`), which is what GNOME/KDE
/// typically write when a picture is set in their user panels.
pub fn resolve_user_avatar_path(username: &str, home: impl AsRef<Path>) -> Option<PathBuf> {
    let home = home.as_ref();
    let mut candidates = Vec::with_capacity(3);
    candidates.push(home.join(".face"));
    candidates.push(home.join(".face.icon"));
    // AccountsService icons are keyed by login name (GNOME / KDE user pictures).
    if !username.is_empty() && !username.contains(['/', '\\', '\0']) {
        candidates.push(PathBuf::from("/var/lib/AccountsService/icons").join(username));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn gecos_full_name() -> Option<String> {
    let user = std::env::var("USER").ok()?;
    let passwd = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in passwd.lines() {
        let mut parts = line.split(':');
        let name = parts.next()?;
        if name != user {
            continue;
        }
        // name:x:uid:gid:gecos:home:shell
        let gecos = parts.nth(3)?;
        let full = gecos.split(',').next().unwrap_or(gecos).trim();
        if !full.is_empty() {
            return Some(full.to_string());
        }
    }
    None
}

/// Known terminal emulators in auto-detect preference order: `(binary, label)`.
/// Kitty is first — Metis ships it as a package dependency and prefers it when
/// Settings → Menu terminal is left on auto-detect.
pub const KNOWN_TERMINALS: &[(&str, &str)] = &[
    ("kitty", "kitty"),
    ("kgx", "GNOME Console"),
    ("gnome-terminal", "GNOME Terminal"),
    ("konsole", "Konsole"),
    ("foot", "foot"),
    ("alacritty", "Alacritty"),
    ("wezterm", "WezTerm"),
    ("xterm", "xterm"),
];

/// Known file managers in auto-detect preference order: `(binary, label)`.
pub const KNOWN_FILE_MANAGERS: &[(&str, &str)] = &[
    ("nautilus", "Files (Nautilus)"),
    ("dolphin", "Dolphin"),
    ("nemo", "Nemo"),
    ("thunar", "Thunar"),
    ("pcmanfm", "PCManFM"),
    ("pcmanfm-qt", "PCManFM-Qt"),
    ("caja", "Caja"),
];

/// True when `bin` is launchable: an executable on `$PATH`, or — when it contains a
/// `/` — an absolute/relative path pointing at an executable file.
pub fn binary_in_path(bin: &str) -> bool {
    let bin = bin.trim();
    if bin.is_empty() {
        return false;
    }
    if bin.contains('/') {
        return is_executable_file(Path::new(bin));
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| is_executable_file(&dir.join(bin)))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && meta.permissions().mode() & 0o111 != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

pub fn menu_config_path() -> PathBuf {
    super::config_dir().join("menu.json")
}

pub fn load_menu_config() -> MenuConfig {
    let path = menu_config_path();
    if path.exists() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<MenuConfig>(&text) {
                return cfg;
            }
            tracing::warn!("menu.json parse failed — using defaults");
        }
    }
    MenuConfig::default()
}

pub fn save_menu_config(config: &MenuConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(menu_config_path(), json)
}

/// Pick the first usable executable from user choice → env var → known list.
/// Never invokes a shell.
pub fn resolve_executable(
    chosen: Option<&str>,
    env_var: &str,
    known: &[(&str, &str)],
) -> Option<String> {
    if let Some(c) = chosen.map(str::trim).filter(|s| !s.is_empty()) {
        if binary_in_path(c) {
            return Some(c.to_string());
        }
    }
    if let Ok(v) = std::env::var(env_var) {
        let v = v.trim();
        if !v.is_empty() && binary_in_path(v) {
            return Some(v.to_string());
        }
    }
    for (bin, _) in known {
        if binary_in_path(bin) {
            return Some((*bin).to_string());
        }
    }
    None
}

/// Resolve the configured/auto-detected terminal binary.
pub fn resolve_terminal() -> Option<String> {
    let cfg = load_menu_config();
    resolve_executable(cfg.terminal.as_deref(), "TERMINAL", KNOWN_TERMINALS)
}

/// Resolve the configured/auto-detected file manager binary.
pub fn resolve_file_manager() -> Option<String> {
    let cfg = load_menu_config();
    resolve_executable(
        cfg.file_manager.as_deref(),
        "FILE_MANAGER",
        KNOWN_FILE_MANAGERS,
    )
}

/// Argv to run `program` inside `terminal` without a shell (`term -e program`).
pub fn argv_in_terminal(terminal: &str, program: &str) -> Vec<String> {
    vec![terminal.to_string(), "-e".to_string(), program.to_string()]
}
