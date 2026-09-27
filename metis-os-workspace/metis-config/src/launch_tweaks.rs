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

/// Best-effort Steam app id from a Metis-spawned argv (`-applaunch` / `steam://…`).
pub fn steam_app_id_from_argv(argv: &[String]) -> Option<u32> {
    for (i, arg) in argv.iter().enumerate() {
        let lower = arg.to_ascii_lowercase();
        if lower == "-applaunch" || lower == "--applaunch" {
            return argv.get(i + 1)?.parse().ok().filter(|&id| id > 0);
        }
        if let Some(rest) = lower.strip_prefix("steam://rungameid/") {
            return rest
                .split(['?', '&', '#'])
                .next()?
                .parse()
                .ok()
                .filter(|&id| id > 0);
        }
        if let Some(rest) = lower.strip_prefix("steam://run/") {
            return rest
                .split(['?', '&', '#', '/'])
                .next()?
                .parse()
                .ok()
                .filter(|&id| id > 0);
        }
    }
    None
}

fn argv_already_gamescope(argv: &[String]) -> bool {
    argv.first()
        .map(|p| {
            let base = p.rsplit('/').next().unwrap_or(p);
            base == "gamescope"
        })
        .unwrap_or(false)
}

fn wrap_with_gamescope(argv: &mut Vec<String>, extra_flags: &str) {
    let mut wrapped = vec!["gamescope".into()];
    for tok in extra_flags.split_whitespace() {
        wrapped.push(tok.to_string());
    }
    wrapped.push("--".into());
    wrapped.append(argv);
    *argv = wrapped;
}

/// Apply MangoHud / Gamescope Big Picture / per-appid Gamescope when binaries exist.
/// Safe to call for any Metis-spawned argv; no-ops when toggles/profiles are unused.
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

    if argv_already_gamescope(&out.argv) || !binary_in_path("gamescope") {
        return out;
    }

    // Per-title profile wins over the Big Picture toggle.
    if let Some(app_id) = steam_app_id_from_argv(&out.argv)
        && let Some(profile) = cfg
            .gamescope_profiles
            .iter()
            .find(|p| p.steam_app_id == app_id)
    {
        wrap_with_gamescope(&mut out.argv, &profile.args);
        return out;
    }

    if cfg.gamescope_big_picture && looks_like_big_picture(&out.argv) {
        wrap_with_gamescope(&mut out.argv, "");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GameScopeProfile;

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

    #[test]
    fn parses_steam_app_ids() {
        assert_eq!(
            steam_app_id_from_argv(&["steam".into(), "-applaunch".into(), "570".into()]),
            Some(570)
        );
        assert_eq!(
            steam_app_id_from_argv(&["steam".into(), "steam://rungameid/730".into()]),
            Some(730)
        );
        assert_eq!(
            steam_app_id_from_argv(&["steam".into(), "steam://run/440?arg=1".into()]),
            Some(440)
        );
        assert_eq!(
            steam_app_id_from_argv(&["steam".into(), "-gamepadui".into()]),
            None
        );
    }

    #[test]
    fn applies_per_app_gamescope_profile_when_binary_present() {
        // Skip when gamescope is not installed in the test environment.
        if !binary_in_path("gamescope") {
            return;
        }
        let cfg = GamingConfig {
            gamescope_profiles: vec![GameScopeProfile {
                steam_app_id: 570,
                args: "-W 1280 -H 720 -f".into(),
            }],
            ..GamingConfig::default()
        };
        let argv = vec!["steam".into(), "-applaunch".into(), "570".into()];
        let tweaked = apply_steam_launch_tweaks(&argv, &cfg);
        assert_eq!(tweaked.argv[0], "gamescope");
        assert!(tweaked.argv.iter().any(|a| a == "-W"));
        assert!(tweaked.argv.iter().any(|a| a == "--"));
        assert!(tweaked.argv.iter().any(|a| a == "-applaunch"));
    }
}
