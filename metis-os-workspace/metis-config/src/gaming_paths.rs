//! Fail-closed validation for gaming Flatpak Steam library mounts and offload
//! env export lines (Phase 18 A).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Roots under which `extra_steam_paths` may resolve after canonicalize.
const ALLOWED_STEAM_LIBRARY_ROOTS: &[&str] = &["/mnt", "/media", "/run/media"];

/// Env keys that Metis may inject for PRIME offload (must match `offload_env_vars`).
pub const OFFLOAD_ENV_KEY_ALLOWLIST: &[&str] = &[
    "__NV_PRIME_RENDER_OFFLOAD",
    "__GLX_VENDOR_LIBRARY_NAME",
    "__VK_LAYER_NV_optimus",
    "DRI_PRIME",
    "MESA_VK_DEVICE_SELECT",
];

/// Expand `~` / `~/…` via `$HOME` only, then canonicalize and require a directory
/// under `$HOME`, `/mnt`, `/media`, or `/run/media`. Missing paths fail closed.
pub fn validate_steam_library_path(raw: &str) -> Option<PathBuf> {
    if raw.is_empty() || raw.contains('\0') {
        return None;
    }
    let expanded = expand_home_prefix(raw)?;
    if !Path::new(&expanded).is_absolute() {
        return None;
    }
    let canon = PathBuf::from(&expanded).canonicalize().ok()?;
    if !canon.is_dir() {
        return None;
    }
    if !is_under_allowed_steam_root(&canon) {
        return None;
    }
    Some(canon)
}

fn expand_home_prefix(raw: &str) -> Option<String> {
    if raw == "~" {
        return std::env::var("HOME").ok();
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        let home = std::env::var("HOME").ok()?;
        return Some(format!("{home}/{rest}"));
    }
    if raw.starts_with('~') {
        // `~other` is not supported — fail closed.
        return None;
    }
    Some(raw.to_string())
}

fn is_under_allowed_steam_root(canon: &Path) -> bool {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(home) = std::env::var("HOME")
        && let Ok(h) = PathBuf::from(home).canonicalize()
    {
        roots.push(h);
    }
    for r in ALLOWED_STEAM_LIBRARY_ROOTS {
        if let Ok(p) = PathBuf::from(r).canonicalize() {
            roots.push(p);
        }
    }
    roots.iter().any(|root| canon.starts_with(root))
}

/// Keep only allowlisted offload keys with values safe for shell / Flatpak argv.
pub fn sanitize_offload_env(env: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for (key, val) in env {
        if let Some((k, v)) = sanitize_offload_env_pair(key, val) {
            out.insert(k, v);
        }
    }
    out
}

/// Validate a single KEY/VAL pair for Flatpak `--env` or a shell `export`.
pub fn sanitize_offload_env_pair(key: &str, val: &str) -> Option<(String, String)> {
    if !OFFLOAD_ENV_KEY_ALLOWLIST.contains(&key) {
        return None;
    }
    if !is_safe_env_value(val) {
        return None;
    }
    Some((key.to_string(), val.to_string()))
}

fn is_safe_env_value(val: &str) -> bool {
    !val.is_empty() && !val.contains('\0') && !val.contains('\n') && !val.contains('\r')
}

/// POSIX single-quoted shell assignment: `export KEY='…'`.
pub fn shell_export_line(key: &str, val: &str) -> Option<String> {
    let (k, v) = sanitize_offload_env_pair(key, val)?;
    Some(format!("export {k}='{}'\n", posix_single_quote_escape(&v)))
}

fn posix_single_quote_escape(s: &str) -> String {
    // 'foo'bar' → 'foo'"'"'bar'
    s.replace('\'', "'\"'\"'")
}

/// Flatpak argv form `KEY=VAL` after the same checks (no shell quoting).
pub fn flatpak_env_arg(key: &str, val: &str) -> Option<String> {
    let (k, v) = sanitize_offload_env_pair(key, val)?;
    Some(format!("{k}={v}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn with_temp_home(test: impl FnOnce(&Path)) {
        let dir = std::env::temp_dir().join(format!(
            "metis-gaming-paths-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("temp home");
        let prev = std::env::var("HOME").ok();
        // SAFETY: tests run single-threaded for this helper; restore after.
        unsafe { std::env::set_var("HOME", &dir) };
        test(&dir);
        match prev {
            Some(h) => unsafe { std::env::set_var("HOME", h) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn accepts_home_subdir() {
        with_temp_home(|home| {
            let games = home.join("Games");
            fs::create_dir_all(&games).unwrap();
            let got = validate_steam_library_path("~/Games").expect("tilde path");
            assert_eq!(got, games.canonicalize().unwrap());
            let abs = validate_steam_library_path(&games.to_string_lossy()).expect("abs");
            assert_eq!(abs, games.canonicalize().unwrap());
        });
    }

    #[test]
    fn rejects_empty_nul_relative_and_etc() {
        assert!(validate_steam_library_path("").is_none());
        assert!(validate_steam_library_path("games\0s").is_none());
        assert!(validate_steam_library_path("games").is_none());
        assert!(validate_steam_library_path("../etc").is_none());
        assert!(validate_steam_library_path("/etc").is_none());
        assert!(validate_steam_library_path("/tmp/../etc/passwd").is_none());
        assert!(validate_steam_library_path("~other/Games").is_none());
    }

    #[test]
    fn rejects_missing_path() {
        with_temp_home(|home| {
            let missing = home.join("no-such-library");
            assert!(validate_steam_library_path(&missing.to_string_lossy()).is_none());
        });
    }

    #[test]
    fn rejects_symlink_escape_from_home() {
        with_temp_home(|home| {
            let link = home.join("escape");
            // Point at /etc if present; otherwise skip.
            if !Path::new("/etc").is_dir() {
                return;
            }
            symlink("/etc", &link).unwrap();
            assert!(validate_steam_library_path(&link.to_string_lossy()).is_none());
        });
    }

    #[test]
    fn accepts_mnt_when_present() {
        let Ok(mnt) = PathBuf::from("/mnt").canonicalize() else {
            return;
        };
        if !mnt.is_dir() {
            return;
        }
        // /mnt itself is under the allowlist root.
        assert_eq!(validate_steam_library_path("/mnt"), Some(mnt));
    }

    #[test]
    fn reject_path_table() {
        let cases = [
            "",
            "games\0s",
            "games",
            "../etc",
            "/etc",
            "/tmp/../etc/passwd",
            "~other/Games",
            "/var/lib/steam",
        ];
        for raw in cases {
            assert!(
                validate_steam_library_path(raw).is_none(),
                "expected reject for {raw:?}"
            );
        }
    }

    #[test]
    fn env_sanitize_table() {
        let accept = [
            ("DRI_PRIME", "pci-0000_01_00_0"),
            ("__NV_PRIME_RENDER_OFFLOAD", "1"),
            ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"),
            ("__VK_LAYER_NV_optimus", "NVIDIA_only"),
            ("MESA_VK_DEVICE_SELECT", "1002:73ff"),
        ];
        for (k, v) in accept {
            assert!(sanitize_offload_env_pair(k, v).is_some(), "{k}={v}");
            assert!(flatpak_env_arg(k, v).is_some());
            assert!(shell_export_line(k, v).is_some());
        }
        let reject = [
            ("PATH", "/usr/bin"),
            ("DRI;PRIME", "1"),
            ("DRI_PRIME", "bad\nline"),
            ("DRI_PRIME", "bad\r"),
            ("DRI_PRIME", "bad\0"),
            ("DRI_PRIME", ""),
            (" EVIL ", "1"),
        ];
        for (k, v) in reject {
            assert!(
                sanitize_offload_env_pair(k, v).is_none(),
                "expected reject for {k}={v:?}"
            );
        }
    }
}
