use serde::{Deserialize, Serialize};

/// Pointer acceleration profile (libinput).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AccelProfile {
    #[default]
    Adaptive,
    Flat,
}

/// Caps Lock remapping via xkb options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CapsLockBehavior {
    #[default]
    Default,
    Escape,
    Control,
}

/// When to enable Num Lock at session start / keyboard hotplug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum NumLockStartup {
    /// Turn Num Lock on when a keyboard with a numeric keypad is present.
    #[default]
    Auto,
    /// Always enable Num Lock.
    On,
    /// Leave Num Lock off (user can still toggle with the key).
    Off,
}

/// Compose key placement via xkb options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ComposeKey {
    #[default]
    Disabled,
    RightAlt,
    Menu,
    LeftAlt,
    ScrollLock,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MouseConfig {
    /// libinput pointer speed (−1.0 … 1.0).
    #[serde(default = "default_pointer_speed")]
    pub speed: f64,
    #[serde(default)]
    pub accel_profile: AccelProfile,
    #[serde(default)]
    pub natural_scroll: bool,
    #[serde(default)]
    pub left_handed: bool,
    /// Wheel / trackball scroll multiplier (0.25–4.0, default 1.0).
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchpadConfig {
    #[serde(default = "default_true")]
    pub tap_to_click: bool,
    #[serde(default = "default_true")]
    pub tap_and_drag: bool,
    #[serde(default = "default_true")]
    pub natural_scroll: bool,
    #[serde(default = "default_true")]
    pub disable_while_typing: bool,
    #[serde(default = "default_pointer_speed")]
    pub speed: f64,
    #[serde(default)]
    pub accel_profile: AccelProfile,
    /// Two-finger / edge scroll multiplier (0.25–4.0, default 1.0).
    #[serde(default = "default_scroll_speed")]
    pub scroll_speed: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyboardConfig {
    /// xkb layout (e.g. `us`). Empty uses the system default from the environment.
    #[serde(default)]
    pub layout: String,
    #[serde(default)]
    pub variant: String,
    /// Extra xkb options, comma-separated (merged with caps/compose presets).
    #[serde(default)]
    pub options: String,
    #[serde(default = "default_repeat_delay_ms")]
    pub repeat_delay_ms: i32,
    #[serde(default = "default_repeat_rate_hz")]
    pub repeat_rate_hz: i32,
    #[serde(default)]
    pub caps_lock: CapsLockBehavior,
    #[serde(default)]
    pub compose_key: ComposeKey,
    /// Num Lock on login / keyboard hotplug (default: auto-detect keypad).
    #[serde(default)]
    pub num_lock: NumLockStartup,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct InputConfig {
    #[serde(default)]
    pub mouse: MouseConfig,
    #[serde(default)]
    pub touchpad: TouchpadConfig,
    #[serde(default)]
    pub keyboard: KeyboardConfig,
}

fn default_pointer_speed() -> f64 {
    0.0
}

fn default_true() -> bool {
    true
}

fn default_scroll_speed() -> f64 {
    1.0
}

fn default_repeat_delay_ms() -> i32 {
    500
}

fn default_repeat_rate_hz() -> i32 {
    25
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            speed: default_pointer_speed(),
            accel_profile: AccelProfile::default(),
            natural_scroll: false,
            left_handed: false,
            scroll_speed: default_scroll_speed(),
        }
    }
}

impl Default for TouchpadConfig {
    fn default() -> Self {
        Self {
            tap_to_click: true,
            tap_and_drag: true,
            natural_scroll: true,
            disable_while_typing: true,
            speed: default_pointer_speed(),
            accel_profile: AccelProfile::default(),
            scroll_speed: default_scroll_speed(),
        }
    }
}

impl Default for KeyboardConfig {
    fn default() -> Self {
        Self {
            layout: String::new(),
            variant: String::new(),
            options: String::new(),
            repeat_delay_ms: default_repeat_delay_ms(),
            repeat_rate_hz: default_repeat_rate_hz(),
            caps_lock: CapsLockBehavior::default(),
            compose_key: ComposeKey::default(),
            num_lock: NumLockStartup::default(),
        }
    }
}

impl KeyboardConfig {
    /// Build the xkb `options` string from presets plus any custom entries.
    pub fn merged_xkb_options(&self) -> Option<String> {
        let mut parts: Vec<String> = self
            .options
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();

        match self.caps_lock {
            CapsLockBehavior::Default => {}
            CapsLockBehavior::Escape => parts.push("caps:escape".into()),
            CapsLockBehavior::Control => parts.push("ctrl:nocaps".into()),
        }

        if let Some(opt) = compose_key_option(self.compose_key) {
            parts.push(opt.to_string());
        }

        if parts.is_empty() {
            None
        } else {
            Some(parts.join(","))
        }
    }
}

fn compose_key_option(key: ComposeKey) -> Option<&'static str> {
    match key {
        ComposeKey::Disabled => None,
        ComposeKey::RightAlt => Some("compose:ralt"),
        ComposeKey::Menu => Some("compose:menu"),
        ComposeKey::LeftAlt => Some("compose:lalt"),
        ComposeKey::ScrollLock => Some("compose:sclk"),
    }
}

pub fn input_config_path() -> std::path::PathBuf {
    super::config_dir().join("input.json")
}

pub fn load_input_config() -> InputConfig {
    let path = input_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
        && let Ok(cfg) = serde_json::from_str(&text)
    {
        return cfg;
    }
    InputConfig::default()
}

pub fn save_input_config(config: &InputConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let json = serde_json::to_string_pretty(config).map_err(std::io::Error::other)?;
    std::fs::write(input_config_path(), json)
}
