//! Desktop shortcut bindings (`~/.config/metis/keybinds.json`).
//!
//! Chords are absolute modifier+key strings (e.g. `Super+L`). The `mod_key`
//! field seeds defaults and labels; reserved DRM session binds are not stored
//! as editable bindings.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Which key acts as the Metis modifier when generating defaults / labels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ModKey {
    #[default]
    Super,
    Alt,
    Ctrl,
}

impl ModKey {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Super => "Super",
            Self::Alt => "Alt",
            Self::Ctrl => "Ctrl",
        }
    }

    pub fn from_env_or_default() -> Self {
        match std::env::var("METIS_MOD")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "alt" => Self::Alt,
            "ctrl" | "control" => Self::Ctrl,
            _ => Self::Super,
        }
    }
}

/// Stable action identifiers for compositor shortcuts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeybindAction {
    Lock,
    OpenTerminal,
    CloseWindow,
    Maximize,
    Fullscreen,
    Minimize,
    ExitFullscreenStack,
    LayoutGrid,
    LayoutFree,
    Screenshot,
    ScreenshotFull,
    ScreenshotWindow,
    CycleWorkspacePrev,
    CycleWorkspaceNext,
    Workspace1,
    Workspace2,
    Workspace3,
    Workspace4,
    Workspace5,
    Workspace6,
    Workspace7,
    Workspace8,
    Workspace9,
    MoveToWorkspace1,
    MoveToWorkspace2,
    MoveToWorkspace3,
    MoveToWorkspace4,
    MoveToWorkspace5,
    MoveToWorkspace6,
    MoveToWorkspace7,
    MoveToWorkspace8,
    MoveToWorkspace9,
    ScrollFocusLeft,
    ScrollFocusRight,
    ScrollFocusUp,
    ScrollFocusDown,
    ScrollMoveLeft,
    ScrollMoveRight,
    ScrollMoveUp,
    ScrollMoveDown,
    ScrollConsume,
    ScrollExpel,
    ScrollCycleWidth,
    MoveWorkspaceOutputLeft,
    MoveWorkspaceOutputRight,
    WindowSwitcherNext,
    WindowSwitcherPrev,
    WorkspaceOverview,
}

impl KeybindAction {
    pub fn all() -> &'static [KeybindAction] {
        ALL_ACTIONS
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lock => "Lock session",
            Self::OpenTerminal => "Open terminal",
            Self::CloseWindow => "Close window",
            Self::Maximize => "Maximize",
            Self::Fullscreen => "Fullscreen",
            Self::Minimize => "Minimize",
            Self::ExitFullscreenStack => "Exit fullscreen / maximize / tile",
            Self::LayoutGrid => "Enable grid tiling",
            Self::LayoutFree => "Disable tiling (free desktop)",
            Self::Screenshot => "Screenshot (interactive)",
            Self::ScreenshotFull => "Screenshot full screen",
            Self::ScreenshotWindow => "Screenshot window",
            Self::CycleWorkspacePrev => "Previous workspace",
            Self::CycleWorkspaceNext => "Next workspace",
            Self::Workspace1 => "Switch to workspace 1",
            Self::Workspace2 => "Switch to workspace 2",
            Self::Workspace3 => "Switch to workspace 3",
            Self::Workspace4 => "Switch to workspace 4",
            Self::Workspace5 => "Switch to workspace 5",
            Self::Workspace6 => "Switch to workspace 6",
            Self::Workspace7 => "Switch to workspace 7",
            Self::Workspace8 => "Switch to workspace 8",
            Self::Workspace9 => "Switch to workspace 9",
            Self::MoveToWorkspace1 => "Move window to workspace 1",
            Self::MoveToWorkspace2 => "Move window to workspace 2",
            Self::MoveToWorkspace3 => "Move window to workspace 3",
            Self::MoveToWorkspace4 => "Move window to workspace 4",
            Self::MoveToWorkspace5 => "Move window to workspace 5",
            Self::MoveToWorkspace6 => "Move window to workspace 6",
            Self::MoveToWorkspace7 => "Move window to workspace 7",
            Self::MoveToWorkspace8 => "Move window to workspace 8",
            Self::MoveToWorkspace9 => "Move window to workspace 9",
            Self::ScrollFocusLeft => "Scroll: focus left",
            Self::ScrollFocusRight => "Scroll: focus right",
            Self::ScrollFocusUp => "Scroll: focus up",
            Self::ScrollFocusDown => "Scroll: focus down",
            Self::ScrollMoveLeft => "Scroll / move window left",
            Self::ScrollMoveRight => "Scroll / move window right",
            Self::ScrollMoveUp => "Scroll: move up",
            Self::ScrollMoveDown => "Scroll: move down",
            Self::ScrollConsume => "Scroll: consume into column",
            Self::ScrollExpel => "Scroll: expel to new column",
            Self::ScrollCycleWidth => "Scroll: cycle column width",
            Self::MoveWorkspaceOutputLeft => "Move workspace to output left",
            Self::MoveWorkspaceOutputRight => "Move workspace to output right",
            Self::WindowSwitcherNext => "Task View (next)",
            Self::WindowSwitcherPrev => "Task View (previous)",
            Self::WorkspaceOverview => "Task View",
        }
    }

    pub fn group(self) -> KeybindGroup {
        match self {
            Self::Lock | Self::OpenTerminal => KeybindGroup::Session,
            Self::CloseWindow
            | Self::Maximize
            | Self::Fullscreen
            | Self::Minimize
            | Self::ExitFullscreenStack => KeybindGroup::Windows,
            Self::LayoutGrid | Self::LayoutFree => KeybindGroup::Layout,
            Self::Screenshot | Self::ScreenshotFull | Self::ScreenshotWindow => {
                KeybindGroup::Screenshots
            }
            Self::CycleWorkspacePrev
            | Self::CycleWorkspaceNext
            | Self::Workspace1
            | Self::Workspace2
            | Self::Workspace3
            | Self::Workspace4
            | Self::Workspace5
            | Self::Workspace6
            | Self::Workspace7
            | Self::Workspace8
            | Self::Workspace9
            | Self::MoveToWorkspace1
            | Self::MoveToWorkspace2
            | Self::MoveToWorkspace3
            | Self::MoveToWorkspace4
            | Self::MoveToWorkspace5
            | Self::MoveToWorkspace6
            | Self::MoveToWorkspace7
            | Self::MoveToWorkspace8
            | Self::MoveToWorkspace9
            | Self::WindowSwitcherNext
            | Self::WindowSwitcherPrev
            | Self::WorkspaceOverview => KeybindGroup::Workspaces,
            Self::ScrollFocusLeft
            | Self::ScrollFocusRight
            | Self::ScrollFocusUp
            | Self::ScrollFocusDown
            | Self::ScrollMoveLeft
            | Self::ScrollMoveRight
            | Self::ScrollMoveUp
            | Self::ScrollMoveDown
            | Self::ScrollConsume
            | Self::ScrollExpel
            | Self::ScrollCycleWidth => KeybindGroup::Scroll,
            Self::MoveWorkspaceOutputLeft | Self::MoveWorkspaceOutputRight => {
                KeybindGroup::Displays
            }
        }
    }

    pub fn workspace_number(self) -> Option<u32> {
        match self {
            Self::Workspace1 | Self::MoveToWorkspace1 => Some(1),
            Self::Workspace2 | Self::MoveToWorkspace2 => Some(2),
            Self::Workspace3 | Self::MoveToWorkspace3 => Some(3),
            Self::Workspace4 | Self::MoveToWorkspace4 => Some(4),
            Self::Workspace5 | Self::MoveToWorkspace5 => Some(5),
            Self::Workspace6 | Self::MoveToWorkspace6 => Some(6),
            Self::Workspace7 | Self::MoveToWorkspace7 => Some(7),
            Self::Workspace8 | Self::MoveToWorkspace8 => Some(8),
            Self::Workspace9 | Self::MoveToWorkspace9 => Some(9),
            _ => None,
        }
    }

    pub fn is_move_to_workspace(self) -> bool {
        matches!(
            self,
            Self::MoveToWorkspace1
                | Self::MoveToWorkspace2
                | Self::MoveToWorkspace3
                | Self::MoveToWorkspace4
                | Self::MoveToWorkspace5
                | Self::MoveToWorkspace6
                | Self::MoveToWorkspace7
                | Self::MoveToWorkspace8
                | Self::MoveToWorkspace9
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeybindGroup {
    Session,
    Windows,
    Workspaces,
    Layout,
    Screenshots,
    Scroll,
    Displays,
    System,
}

impl KeybindGroup {
    pub fn label(self) -> &'static str {
        match self {
            Self::Session => "Session",
            Self::Windows => "Windows",
            Self::Workspaces => "Workspaces",
            Self::Layout => "Layout",
            Self::Screenshots => "Screenshots",
            Self::Scroll => "Scroll layout",
            Self::Displays => "Displays",
            Self::System => "System",
        }
    }

    pub fn all() -> &'static [KeybindGroup] {
        &[
            Self::Session,
            Self::Windows,
            Self::Workspaces,
            Self::Layout,
            Self::Screenshots,
            Self::Scroll,
            Self::Displays,
            Self::System,
        ]
    }
}

const ALL_ACTIONS: &[KeybindAction] = &[
    KeybindAction::Lock,
    KeybindAction::OpenTerminal,
    KeybindAction::CloseWindow,
    KeybindAction::Maximize,
    KeybindAction::Fullscreen,
    KeybindAction::Minimize,
    KeybindAction::ExitFullscreenStack,
    KeybindAction::LayoutGrid,
    KeybindAction::LayoutFree,
    KeybindAction::Screenshot,
    KeybindAction::ScreenshotFull,
    KeybindAction::ScreenshotWindow,
    KeybindAction::CycleWorkspacePrev,
    KeybindAction::CycleWorkspaceNext,
    KeybindAction::Workspace1,
    KeybindAction::Workspace2,
    KeybindAction::Workspace3,
    KeybindAction::Workspace4,
    KeybindAction::Workspace5,
    KeybindAction::Workspace6,
    KeybindAction::Workspace7,
    KeybindAction::Workspace8,
    KeybindAction::Workspace9,
    KeybindAction::MoveToWorkspace1,
    KeybindAction::MoveToWorkspace2,
    KeybindAction::MoveToWorkspace3,
    KeybindAction::MoveToWorkspace4,
    KeybindAction::MoveToWorkspace5,
    KeybindAction::MoveToWorkspace6,
    KeybindAction::MoveToWorkspace7,
    KeybindAction::MoveToWorkspace8,
    KeybindAction::MoveToWorkspace9,
    KeybindAction::ScrollFocusLeft,
    KeybindAction::ScrollFocusRight,
    KeybindAction::ScrollFocusUp,
    KeybindAction::ScrollFocusDown,
    KeybindAction::ScrollMoveLeft,
    KeybindAction::ScrollMoveRight,
    KeybindAction::ScrollMoveUp,
    KeybindAction::ScrollMoveDown,
    KeybindAction::ScrollConsume,
    KeybindAction::ScrollExpel,
    KeybindAction::ScrollCycleWidth,
    KeybindAction::MoveWorkspaceOutputLeft,
    KeybindAction::MoveWorkspaceOutputRight,
    KeybindAction::WindowSwitcherNext,
    KeybindAction::WindowSwitcherPrev,
    KeybindAction::WorkspaceOverview,
];

/// A keyboard chord: optional modifiers + one key name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Chord {
    #[serde(default)]
    pub ctrl: bool,
    #[serde(default)]
    pub alt: bool,
    #[serde(default)]
    pub shift: bool,
    #[serde(default)]
    pub super_key: bool,
    /// Key token: `L`, `Print`, `Left`, `F1`, `1`, `slash`, `backslash`, …
    pub key: String,
}

impl Chord {
    pub fn new(super_key: bool, ctrl: bool, alt: bool, shift: bool, key: &str) -> Self {
        Self {
            ctrl,
            alt,
            shift,
            super_key,
            key: key.to_string(),
        }
    }

    pub fn display(&self) -> String {
        if self.key.trim().is_empty() {
            return "Unbound".into();
        }
        let mut parts = Vec::new();
        if self.ctrl {
            parts.push("Ctrl");
        }
        if self.alt {
            parts.push("Alt");
        }
        if self.shift {
            parts.push("Shift");
        }
        if self.super_key {
            parts.push("Super");
        }
        parts.push(self.key.as_str());
        parts.join("+")
    }

    pub fn is_reserved(&self) -> bool {
        reserved_chords().iter().any(|r| r == self)
    }
}

impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.display())
    }
}

impl FromStr for Chord {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut ctrl = false;
        let mut alt = false;
        let mut shift = false;
        let mut super_key = false;
        let mut key: Option<String> = None;
        for part in s.split('+').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" => ctrl = true,
                "alt" => alt = true,
                "shift" => shift = true,
                "super" | "meta" | "logo" | "mod" | "win" => super_key = true,
                other => {
                    if key.is_some() {
                        return Err(format!("multiple keys in chord: {s}"));
                    }
                    key = Some(normalize_key_token(other));
                }
            }
        }
        let key = key.ok_or_else(|| format!("missing key in chord: {s}"))?;
        Ok(Self {
            ctrl,
            alt,
            shift,
            super_key,
            key,
        })
    }
}

fn normalize_key_token(raw: &str) -> String {
    match raw.to_ascii_lowercase().as_str() {
        "esc" | "escape" => "Escape".into(),
        "return" | "enter" => "Return".into(),
        "space" => "Space".into(),
        "print" | "printscreen" | "sys_req" | "sysreq" => "Print".into(),
        "left" => "Left".into(),
        "right" => "Right".into(),
        "up" => "Up".into(),
        "down" => "Down".into(),
        "slash" | "/" => "slash".into(),
        "backslash" | "\\" => "backslash".into(),
        "comma" | "," => "comma".into(),
        "period" | "." => "period".into(),
        "minus" | "-" => "minus".into(),
        "equal" | "=" | "plus" => "equal".into(),
        "tab" => "Tab".into(),
        other if other.len() == 1 => other.to_ascii_uppercase(),
        other => {
            // F1..F12 and multi-letter tokens
            let mut chars = other.chars();
            match chars.next() {
                Some(c) => format!("{}{}", c.to_ascii_uppercase(), chars.as_str()),
                None => other.to_string(),
            }
        }
    }
}

/// DRM / session binds shown in Settings but never editable.
pub fn reserved_chords() -> Vec<Chord> {
    reserved_system_rows().into_iter().map(|(_, c)| c).collect()
}

/// Reserved system shortcuts for the Settings list (label, chord).
pub fn reserved_system_rows() -> Vec<(String, Chord)> {
    let mut rows = Vec::with_capacity(28);
    for n in 1..=12 {
        rows.push((
            format!("Switch to VT {n}"),
            Chord::new(false, true, true, false, &format!("F{n}")),
        ));
    }
    rows.push((
        "Quit session (return to greeter)".into(),
        Chord::new(false, true, true, false, "BackSpace"),
    ));
    rows.extend(hardware_key_rows());
    rows
}

/// Fixed multimedia / hardware keys (`XF86*`) serviced by the compositor and
/// shell. Shown in Settings for discoverability; the key caps are wired in
/// firmware/xkb and are not user-rebindable.
fn hardware_key_rows() -> Vec<(String, Chord)> {
    let hw = |label: &str, cap: &str| {
        (
            label.to_string(),
            Chord::new(false, false, false, false, cap),
        )
    };
    vec![
        hw("Volume up", "Vol +"),
        hw("Volume down", "Vol -"),
        hw("Mute", "Mute"),
        hw("Microphone mute", "Mic mute"),
        hw("Display brightness up", "Bright +"),
        hw("Display brightness down", "Bright -"),
        hw("Keyboard backlight up", "Kbd +"),
        hw("Keyboard backlight down", "Kbd -"),
        hw("Play / Pause", "Play"),
        hw("Stop", "Stop"),
        hw("Next track", "Next"),
        hw("Previous track", "Prev"),
        hw("Fast-forward", "Fwd"),
        hw("Rewind", "Rew"),
        hw("Mirror / extend displays", "Display"),
    ]
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeybindsConfig {
    #[serde(default)]
    pub mod_key: ModKey,
    /// Sparse overrides keyed by action; missing entries use [`default_bindings`].
    #[serde(default)]
    pub bindings: HashMap<KeybindAction, Chord>,
}

impl Default for KeybindsConfig {
    fn default() -> Self {
        Self {
            mod_key: ModKey::from_env_or_default(),
            bindings: HashMap::new(),
        }
    }
}

impl KeybindsConfig {
    /// Effective chord for an action (override or default for `mod_key`).
    pub fn chord_for(&self, action: KeybindAction) -> Chord {
        self.bindings
            .get(&action)
            .cloned()
            .unwrap_or_else(|| default_chord(action, self.mod_key))
    }

    pub fn resolved_map(&self) -> HashMap<KeybindAction, Chord> {
        let mut map = HashMap::new();
        for &action in KeybindAction::all() {
            map.insert(action, self.chord_for(action));
        }
        map
    }

    /// Reverse lookup: first action that owns this chord (for conflict checks).
    pub fn action_for_chord(&self, chord: &Chord) -> Option<KeybindAction> {
        KeybindAction::all()
            .iter()
            .find(|&&action| &self.chord_for(action) == chord)
            .copied()
    }

    pub fn set_binding(&mut self, action: KeybindAction, chord: Chord) {
        let def = default_chord(action, self.mod_key);
        if chord == def {
            self.bindings.remove(&action);
        } else {
            self.bindings.insert(action, chord);
        }
    }

    pub fn reset_binding(&mut self, action: KeybindAction) {
        self.bindings.remove(&action);
    }

    pub fn sanitize(mut self) -> Self {
        let mod_key = self.mod_key;
        self.bindings.retain(|action, chord| {
            !chord.key.trim().is_empty()
                && !chord.is_reserved()
                && chord != &default_chord(*action, mod_key)
        });
        self
    }
}

pub fn default_chord(action: KeybindAction, mod_key: ModKey) -> Chord {
    let (logo, alt, ctrl) = match mod_key {
        ModKey::Super => (true, false, false),
        ModKey::Alt => (false, true, false),
        ModKey::Ctrl => (false, false, true),
    };
    let m = |shift: bool, key: &str| Chord::new(logo, ctrl, alt, shift, key);
    match action {
        KeybindAction::Lock => m(false, "L"),
        KeybindAction::OpenTerminal => m(false, "T"),
        KeybindAction::CloseWindow => m(false, "Q"),
        KeybindAction::Maximize => m(false, "F"),
        KeybindAction::Fullscreen => m(true, "F"),
        KeybindAction::Minimize => m(false, "M"),
        KeybindAction::ExitFullscreenStack => m(false, "Escape"),
        KeybindAction::LayoutGrid => m(false, "slash"),
        KeybindAction::LayoutFree => m(false, "backslash"),
        KeybindAction::Screenshot => Chord::new(false, false, false, false, "Print"),
        KeybindAction::ScreenshotFull => Chord::new(false, false, false, true, "Print"),
        KeybindAction::ScreenshotWindow => Chord::new(false, true, false, false, "Print"),
        // Cycle always Super+Alt historically (independent of mod_key).
        KeybindAction::CycleWorkspacePrev => Chord::new(true, false, true, false, "Left"),
        KeybindAction::CycleWorkspaceNext => Chord::new(true, false, true, false, "Right"),
        KeybindAction::Workspace1 => m(false, "1"),
        KeybindAction::Workspace2 => m(false, "2"),
        KeybindAction::Workspace3 => m(false, "3"),
        KeybindAction::Workspace4 => m(false, "4"),
        KeybindAction::Workspace5 => m(false, "5"),
        KeybindAction::Workspace6 => m(false, "6"),
        KeybindAction::Workspace7 => m(false, "7"),
        KeybindAction::Workspace8 => m(false, "8"),
        KeybindAction::Workspace9 => m(false, "9"),
        KeybindAction::MoveToWorkspace1 => m(true, "1"),
        KeybindAction::MoveToWorkspace2 => m(true, "2"),
        KeybindAction::MoveToWorkspace3 => m(true, "3"),
        KeybindAction::MoveToWorkspace4 => m(true, "4"),
        KeybindAction::MoveToWorkspace5 => m(true, "5"),
        KeybindAction::MoveToWorkspace6 => m(true, "6"),
        KeybindAction::MoveToWorkspace7 => m(true, "7"),
        KeybindAction::MoveToWorkspace8 => m(true, "8"),
        KeybindAction::MoveToWorkspace9 => m(true, "9"),
        KeybindAction::ScrollFocusLeft => m(false, "Left"),
        KeybindAction::ScrollFocusRight => m(false, "Right"),
        KeybindAction::ScrollFocusUp => m(false, "Up"),
        KeybindAction::ScrollFocusDown => m(false, "Down"),
        KeybindAction::ScrollMoveLeft => m(true, "Left"),
        KeybindAction::ScrollMoveRight => m(true, "Right"),
        KeybindAction::ScrollMoveUp => m(true, "Up"),
        KeybindAction::ScrollMoveDown => m(true, "Down"),
        KeybindAction::ScrollConsume => m(false, "comma"),
        KeybindAction::ScrollExpel => m(false, "period"),
        KeybindAction::ScrollCycleWidth => m(false, "minus"),
        KeybindAction::MoveWorkspaceOutputLeft => Chord::new(logo, true, alt, true, "Left"),
        KeybindAction::MoveWorkspaceOutputRight => Chord::new(logo, true, alt, true, "Right"),
        // Super+Shift+Tab cycles Task View backward while Super+Tab goes forward.
        KeybindAction::WindowSwitcherNext => Chord::new(false, false, false, false, ""),
        KeybindAction::WindowSwitcherPrev => Chord::new(true, false, false, true, "Tab"),
        KeybindAction::WorkspaceOverview => Chord::new(true, false, false, false, "Tab"),
    }
}

pub fn keybinds_config_path() -> PathBuf {
    super::config_dir().join("keybinds.json")
}

pub fn load_keybinds_config() -> KeybindsConfig {
    let path = keybinds_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        match serde_json::from_str::<KeybindsConfig>(&text) {
            Ok(cfg) => return cfg.sanitize(),
            Err(err) => tracing::warn!(%err, "keybinds.json parse failed — using defaults"),
        }
    }
    KeybindsConfig::default().sanitize()
}

pub fn save_keybinds_config(config: &KeybindsConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let sanitized = config.clone().sanitize();
    let json = serde_json::to_string_pretty(&sanitized).map_err(std::io::Error::other)?;
    std::fs::write(keybinds_config_path(), json)
}

pub fn save_default_keybinds_config() -> std::io::Result<()> {
    let path = keybinds_config_path();
    if path.exists() {
        return Ok(());
    }
    save_keybinds_config(&KeybindsConfig::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_super_l() {
        let c: Chord = "Super+L".parse().unwrap();
        assert!(c.super_key && !c.shift && c.key == "L");
    }

    #[test]
    fn reserved_ctrl_alt_f2() {
        let c: Chord = "Ctrl+Alt+F2".parse().unwrap();
        assert!(c.is_reserved());
    }

    #[test]
    fn parse_alt_tab() {
        let c: Chord = "Alt+Tab".parse().unwrap();
        assert!(c.alt && !c.shift && !c.super_key && c.key == "Tab");
        let prev: Chord = "Alt+Shift+Tab".parse().unwrap();
        assert!(prev.alt && prev.shift && prev.key == "Tab");
        let overview: Chord = "Super+Tab".parse().unwrap();
        assert!(overview.super_key && overview.key == "Tab");
    }

    #[test]
    fn default_task_view_chords() {
        let next = default_chord(KeybindAction::WindowSwitcherNext, ModKey::Super);
        assert!(next.key.is_empty(), "Task View next has no default chord");
        let prev = default_chord(KeybindAction::WindowSwitcherPrev, ModKey::Super);
        assert_eq!(prev.display(), "Shift+Super+Tab");
        let overview = default_chord(KeybindAction::WorkspaceOverview, ModKey::Super);
        assert_eq!(overview.display(), "Super+Tab");
    }
}
