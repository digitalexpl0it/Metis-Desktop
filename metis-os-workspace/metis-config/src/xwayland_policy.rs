//! XWayland launch-class policy (Phase 18 D).
//!
//! Soft bucketing only: Metis-spawned clients get a gaming or desktop `DISPLAY`
//! when `xwayland_mode` is `isolated`. Same-UID processes can still open either
//! X socket — this is not a sandbox.

use serde::{Deserialize, Serialize};

/// Built-in substrings that select the gaming XWayland bucket.
pub const DEFAULT_GAMING_XWAYLAND_PATTERNS: &[&str] = &[
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
    "com.valvesoftware.steam",
    "net.lutris.lutris",
    "com.heroicgameslauncher.hgl",
];

const MAX_PATTERN_LEN: usize = 64;
const MAX_EXTRA_PATTERNS: usize = 64;

/// User-editable policy for which launches use the gaming XWayland bucket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct XwaylandPolicy {
    /// When non-empty, replaces the built-in pattern list as the base set.
    /// When empty (default), [`DEFAULT_GAMING_XWAYLAND_PATTERNS`] is used.
    pub gaming_patterns: Vec<String>,
    /// Always appended after the base set (built-in or `gaming_patterns`).
    pub extra_gaming_patterns: Vec<String>,
}

impl XwaylandPolicy {
    pub fn sanitize(mut self) -> Self {
        self.gaming_patterns = sanitize_patterns(self.gaming_patterns);
        self.extra_gaming_patterns = sanitize_patterns(self.extra_gaming_patterns);
        if self.extra_gaming_patterns.len() > MAX_EXTRA_PATTERNS {
            self.extra_gaming_patterns.truncate(MAX_EXTRA_PATTERNS);
        }
        self
    }

    fn base_patterns(&self) -> Vec<String> {
        if self.gaming_patterns.is_empty() {
            DEFAULT_GAMING_XWAYLAND_PATTERNS
                .iter()
                .map(|s| (*s).to_string())
                .collect()
        } else {
            self.gaming_patterns.clone()
        }
    }

    /// All patterns consulted for gaming-bucket membership.
    pub fn effective_patterns(&self) -> Vec<String> {
        let mut out = self.base_patterns();
        for p in &self.extra_gaming_patterns {
            if !out.iter().any(|e| e == p) {
                out.push(p.clone());
            }
        }
        out
    }
}

fn sanitize_patterns(raw: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    for p in raw {
        let s = p.trim().to_ascii_lowercase();
        if s.is_empty() || s.len() > MAX_PATTERN_LEN {
            continue;
        }
        if s.contains('/') || s.contains('\\') || s.contains('\0') {
            continue;
        }
        if !out.iter().any(|e| e == &s) {
            out.push(s);
        }
    }
    out
}

/// Whether a Metis-spawned program should use the gaming XWayland `DISPLAY`
/// when isolation is enabled. Independent of GPU offload / battery policy.
pub fn command_uses_gaming_xwayland(program: &str, policy: &XwaylandPolicy) -> bool {
    let p = program.to_ascii_lowercase();
    policy
        .effective_patterns()
        .iter()
        .any(|n| p.contains(n.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gaming_class_table() {
        let policy = XwaylandPolicy::default();
        let gaming = [
            "steam",
            "/usr/bin/steam -gamepadui",
            "flatpak run com.valvesoftware.Steam",
            "proton run game.exe",
            "lutris",
            "wine game.exe",
        ];
        for prog in gaming {
            assert!(
                command_uses_gaming_xwayland(prog, &policy),
                "expected gaming for {prog}"
            );
        }
        let desktop = ["gedit", "firefox", "metis-settings", "nautilus"];
        for prog in desktop {
            assert!(
                !command_uses_gaming_xwayland(prog, &policy),
                "expected desktop for {prog}"
            );
        }
    }

    #[test]
    fn extra_patterns_append() {
        let policy = XwaylandPolicy {
            extra_gaming_patterns: vec!["my-launcher".into()],
            ..XwaylandPolicy::default()
        }
        .sanitize();
        assert!(command_uses_gaming_xwayland("my-launcher", &policy));
        assert!(command_uses_gaming_xwayland("steam", &policy));
    }

    #[test]
    fn custom_base_replaces_builtins() {
        let policy = XwaylandPolicy {
            gaming_patterns: vec!["only-this".into()],
            extra_gaming_patterns: vec![],
        }
        .sanitize();
        assert!(command_uses_gaming_xwayland("only-this-app", &policy));
        assert!(!command_uses_gaming_xwayland("steam", &policy));
    }

    #[test]
    fn sanitize_rejects_paths_and_empty() {
        let policy = XwaylandPolicy {
            extra_gaming_patterns: vec![
                "".into(),
                "/bin/sh".into(),
                "OK-Pattern".into(),
                "a".repeat(MAX_PATTERN_LEN + 1),
            ],
            ..XwaylandPolicy::default()
        }
        .sanitize();
        assert_eq!(policy.extra_gaming_patterns, vec!["ok-pattern".to_string()]);
    }

    #[test]
    fn class_independent_of_dgpu_heuristic_overlap() {
        // Browsers prefer dGPU in Auto but must stay on desktop XWayland.
        let policy = XwaylandPolicy::default();
        assert!(!command_uses_gaming_xwayland("firefox", &policy));
        assert!(!command_uses_gaming_xwayland(
            "google-chrome-stable",
            &policy
        ));
    }
}
