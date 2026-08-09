//! Metis-owned Steam / Big Picture launch wrappers (never writes Steam VDF).

use crate::GamingConfig;

/// Result of applying optional MangoHud / Gamescope toggles to a Metis spawn argv.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchTweaks {
    pub argv: Vec<String>,
    /// Extra env pairs (e.g. `MANGOHUD=1`).
    pub env: Vec<(String, String)>,
}

fn binary_in_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(program);
        candidate.is_file()
            && std::fs::metadata(&candidate)
                .map(|m| {
                    use std::os::unix::fs::PermissionsExt;
                    m.permissions().mode() & 0o111 != 0
                })
                .unwrap_or(false)
    })
}

pub fn looks_like_steam_launch(argv: &[String]) -> bool {
    let joined = argv.join(" ").to_ascii_lowercase();
    if joined.contains("steam") || joined.contains("com.valvesoftware.steam") {
        return true;
    }
    argv.first()
        .map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            base == "steam" || base == "launch-steam"
        })
        .unwrap_or(false)
}

pub fn looks_like_big_picture(argv: &[String]) -> bool {
    argv.iter().any(|a| {
        let lower = a.to_ascii_lowercase();
        lower == "-gamepadui" || lower.contains("gamepadui") || lower.contains("bigpicture")
    })
}

/// Apply `mangohud_for_games` / `gamescope_big_picture` when binaries exist.
/// Safe to call for any Metis-spawned argv; no-ops when toggles are off.
pub fn apply_steam_launch_tweaks(argv: &[String], cfg: &GamingConfig) -> LaunchTweaks {
    let mut out = LaunchTweaks {
        argv: argv.to_vec(),
        env: Vec::new(),
    };
    if out.argv.is_empty() || !looks_like_steam_launch(&out.argv) {
        return out;
    }

    if cfg.mangohud_for_games && binary_in_path("mangohud") {
        out.env.push(("MANGOHUD".into(), "1".into()));
    }

    let already_gamescope = out
        .argv
        .first()
        .map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            base == "gamescope"
        })
        .unwrap_or(false);
    if cfg.gamescope_big_picture
        && !already_gamescope
        && looks_like_big_picture(&out.argv)
        && binary_in_path("gamescope")
    {
        let mut wrapped = vec!["gamescope".into(), "--".into()];
        wrapped.append(&mut out.argv);
        out.argv = wrapped;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_big_picture_flag() {
        let argv = vec!["steam".into(), "-gamepadui".into()];
        assert!(looks_like_steam_launch(&argv));
        assert!(looks_like_big_picture(&argv));
    }

    #[test]
    fn tweaks_noop_when_disabled() {
        let cfg = GamingConfig::default();
        let argv = vec!["steam".into(), "-gamepadui".into()];
        let tweaked = apply_steam_launch_tweaks(&argv, &cfg);
        assert_eq!(tweaked.argv, argv);
        assert!(tweaked.env.is_empty());
    }
}
