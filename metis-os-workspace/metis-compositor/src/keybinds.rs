//! Configurable desktop shortcuts loaded from `keybinds.json`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use metis_config::{Chord, KeybindAction, KeybindsConfig, ModKey, load_keybinds_config};
use smithay::input::keyboard::ModifiersState;
use smithay::input::keyboard::keysyms;

static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ChordKey {
    ctrl: bool,
    alt: bool,
    shift: bool,
    super_key: bool,
    /// Normalized key token (same as [`Chord::key`]).
    key: String,
}

impl From<&Chord> for ChordKey {
    fn from(c: &Chord) -> Self {
        Self {
            ctrl: c.ctrl,
            alt: c.alt,
            shift: c.shift,
            super_key: c.super_key,
            key: c.key.clone(),
        }
    }
}

#[derive(Debug)]
pub struct KeybindRuntime {
    pub config: KeybindsConfig,
    by_chord: HashMap<ChordKey, KeybindAction>,
    path_mtime: Option<SystemTime>,
}

impl Default for KeybindRuntime {
    fn default() -> Self {
        Self::load()
    }
}

impl KeybindRuntime {
    pub fn load() -> Self {
        let config = load_keybinds_config();
        let path_mtime = std::fs::metadata(metis_config::keybinds_config_path())
            .and_then(|m| m.modified())
            .ok();
        let by_chord = build_index(&config);
        Self {
            config,
            by_chord,
            path_mtime,
        }
    }

    pub fn reload(&mut self) {
        *self = Self::load();
        tracing::info!(
            mod_key = ?self.config.mod_key,
            bindings = self.config.bindings.len(),
            "keybinds reloaded"
        );
    }

    pub fn maybe_refresh(&mut self) {
        let path = metis_config::keybinds_config_path();
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if mtime != self.path_mtime {
            self.reload();
        }
    }

    pub fn mod_key(&self) -> ModKey {
        self.config.mod_key
    }

    pub fn lookup(&self, modifiers: &ModifiersState, key_token: &str) -> Option<KeybindAction> {
        let key = ChordKey {
            ctrl: modifiers.ctrl,
            alt: modifiers.alt,
            shift: modifiers.shift,
            super_key: modifiers.logo,
            key: key_token.to_string(),
        };
        self.by_chord.get(&key).copied()
    }
}

fn build_index(config: &KeybindsConfig) -> HashMap<ChordKey, KeybindAction> {
    let mut map = HashMap::new();
    for &action in KeybindAction::all() {
        let chord = config.chord_for(action);
        if chord.key.trim().is_empty() {
            continue;
        }
        map.insert(ChordKey::from(&chord), action);
    }
    // Alias: equal key also cycles scroll width (historical Mod+=).
    let width = config.chord_for(KeybindAction::ScrollCycleWidth);
    if width.key == "minus" {
        let mut eq = width.clone();
        eq.key = "equal".into();
        map.insert(ChordKey::from(&eq), KeybindAction::ScrollCycleWidth);
    }
    map
}

pub fn set_capture_active(active: bool) {
    CAPTURE_ACTIVE.store(active, Ordering::SeqCst);
}

pub fn capture_active() -> bool {
    CAPTURE_ACTIVE.load(Ordering::SeqCst)
}

/// True when the configured Metis modifier is held.
pub fn mod_active(runtime: &KeybindRuntime, modifiers: &ModifiersState) -> bool {
    match runtime.mod_key() {
        ModKey::Super => modifiers.logo,
        ModKey::Alt => modifiers.alt,
        ModKey::Ctrl => modifiers.ctrl,
    }
}

pub fn keybind_mod_label(runtime: &KeybindRuntime) -> &'static str {
    runtime.mod_key().as_str()
}

/// Map a keysym to the chord key token used in `keybinds.json`.
pub fn keysym_to_token(sym: u32, digit_sym: u32) -> Option<String> {
    // Prefer latin digit for workspace rows when Shift remaps the modified sym.
    if let Some(d) = digit_token(digit_sym).or_else(|| digit_token(sym)) {
        return Some(d);
    }
    Some(match sym {
        keysyms::KEY_Escape => "Escape".into(),
        keysyms::KEY_Return | keysyms::KEY_KP_Enter => "Return".into(),
        keysyms::KEY_space => "Space".into(),
        keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab => "Tab".into(),
        keysyms::KEY_Print | keysyms::KEY_Sys_Req => "Print".into(),
        keysyms::KEY_Left => "Left".into(),
        keysyms::KEY_Right => "Right".into(),
        keysyms::KEY_Up => "Up".into(),
        keysyms::KEY_Down => "Down".into(),
        keysyms::KEY_slash => "slash".into(),
        keysyms::KEY_backslash => "backslash".into(),
        keysyms::KEY_comma => "comma".into(),
        keysyms::KEY_period => "period".into(),
        keysyms::KEY_minus => "minus".into(),
        keysyms::KEY_equal => "equal".into(),
        keysyms::KEY_BackSpace => "BackSpace".into(),
        keysyms::KEY_F1 => "F1".into(),
        keysyms::KEY_F2 => "F2".into(),
        keysyms::KEY_F3 => "F3".into(),
        keysyms::KEY_F4 => "F4".into(),
        keysyms::KEY_F5 => "F5".into(),
        keysyms::KEY_F6 => "F6".into(),
        keysyms::KEY_F7 => "F7".into(),
        keysyms::KEY_F8 => "F8".into(),
        keysyms::KEY_F9 => "F9".into(),
        keysyms::KEY_F10 => "F10".into(),
        keysyms::KEY_F11 => "F11".into(),
        keysyms::KEY_F12 => "F12".into(),
        s if (keysyms::KEY_a..=keysyms::KEY_z).contains(&s) => {
            ((b'A' + (s - keysyms::KEY_a) as u8) as char).to_string()
        }
        s if (keysyms::KEY_A..=keysyms::KEY_Z).contains(&s) => {
            ((b'A' + (s - keysyms::KEY_A) as u8) as char).to_string()
        }
        _ => return None,
    })
}

fn digit_token(sym: u32) -> Option<String> {
    let n = match sym {
        keysyms::KEY_1 | keysyms::KEY_KP_1 => 1,
        keysyms::KEY_2 | keysyms::KEY_KP_2 => 2,
        keysyms::KEY_3 | keysyms::KEY_KP_3 => 3,
        keysyms::KEY_4 | keysyms::KEY_KP_4 => 4,
        keysyms::KEY_5 | keysyms::KEY_KP_5 => 5,
        keysyms::KEY_6 | keysyms::KEY_KP_6 => 6,
        keysyms::KEY_7 | keysyms::KEY_KP_7 => 7,
        keysyms::KEY_8 | keysyms::KEY_KP_8 => 8,
        keysyms::KEY_9 | keysyms::KEY_KP_9 => 9,
        _ => return None,
    };
    Some(n.to_string())
}
