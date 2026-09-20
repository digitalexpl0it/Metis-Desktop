//! Factory-reset helpers for `~/.config/metis`.
//!
//! Clears persisted Metis prefs so the next load regenerates defaults. Optional
//! backup, custom-theme keep, and onboarding re-run are caller-controlled.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Local;

use crate::{config_dir, ensure_config_dirs, AppConfig};

/// Stock theme basenames always regenerated from embedded defaults.
const STOCK_THEMES: &[&str] = &["dark", "light"];

#[derive(Debug, Clone)]
pub struct ResetOptions {
    /// Copy the current config tree to `~/metis-config-backup-…` before wiping.
    pub backup: bool,
    /// Keep user theme JSON files other than stock `dark` / `light`.
    pub keep_custom_themes: bool,
    /// Leave `onboarding_complete` false so the first-run wizard runs again.
    pub rerun_onboarding: bool,
}

impl Default for ResetOptions {
    fn default() -> Self {
        Self {
            backup: true,
            keep_custom_themes: true,
            rerun_onboarding: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResetResult {
    pub backup_path: Option<PathBuf>,
}

/// Wipe `~/.config/metis` (with options) and recreate the empty layout.
pub fn reset_metis_config(opts: &ResetOptions) -> io::Result<ResetResult> {
    let result = reset_metis_config_at(&config_dir(), opts)?;
    ensure_config_dirs()?;
    Ok(result)
}

/// Same as [`reset_metis_config`], but operate on an explicit directory (tests).
pub fn reset_metis_config_at(dir: &Path, opts: &ResetOptions) -> io::Result<ResetResult> {
    let mut backup_path = None;

    if opts.backup && dir.is_dir() && dir_has_entries(dir)? {
        let dest = backup_destination()?;
        copy_dir_all(dir, &dest)?;
        backup_path = Some(dest);
    }

    let custom_themes = if opts.keep_custom_themes {
        collect_custom_themes(&dir.join("themes"))?
    } else {
        Vec::new()
    };

    if dir.exists() {
        remove_dir_contents(dir)?;
    }
    fs::create_dir_all(dir)?;
    fs::create_dir_all(dir.join("themes"))?;

    for (name, bytes) in custom_themes {
        fs::write(dir.join("themes").join(name), bytes)?;
    }

    if !opts.rerun_onboarding {
        write_onboarding_complete_at(dir)?;
    }

    Ok(ResetResult { backup_path })
}

fn write_onboarding_complete_at(dir: &Path) -> io::Result<()> {
    let cfg = AppConfig {
        onboarding_complete: true,
        gaming_setup_complete: true,
        ..Default::default()
    };
    let path = dir.join("config.json");
    let json = serde_json::to_string_pretty(&cfg).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(())
}

fn backup_destination() -> io::Result<PathBuf> {
    let stamp = Local::now().format("%Y%m%d-%H%M%S");
    let home = directories::UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let mut dest = home.join(format!("metis-config-backup-{stamp}"));
    let mut n = 1u32;
    while dest.exists() {
        dest = home.join(format!("metis-config-backup-{stamp}-{n}"));
        n += 1;
    }
    Ok(dest)
}

fn dir_has_entries(dir: &Path) -> io::Result<bool> {
    Ok(fs::read_dir(dir)?.next().is_some())
}

fn collect_custom_themes(themes_dir: &Path) -> io::Result<Vec<(PathBuf, Vec<u8>)>> {
    let mut out = Vec::new();
    if !themes_dir.is_dir() {
        return Ok(out);
    }
    for entry in fs::read_dir(themes_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        if STOCK_THEMES.contains(&stem) {
            continue;
        }
        let name = entry.file_name();
        let bytes = fs::read(&path)?;
        out.push((PathBuf::from(name), bytes));
    }
    Ok(out)
}

fn remove_dir_contents(dir: &Path) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs::remove_dir_all(path)?;
        } else {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn copy_dir_all(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&from, &to)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config() -> PathBuf {
        let tmp = std::env::temp_dir().join(format!(
            "metis-reset-test-{}-{}",
            std::process::id(),
            Local::now().timestamp_nanos_opt().unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("themes")).expect("temp themes dir");
        tmp
    }

    #[test]
    fn reset_wipes_json_keeps_custom_theme() {
        let config = temp_config();
        fs::write(config.join("bar.json"), "{}").expect("bar");
        fs::write(config.join("themes/dark.json"), "{\"stock\":true}").expect("dark");
        fs::write(config.join("themes/mine.json"), "{\"custom\":true}").expect("mine");

        let result = reset_metis_config_at(
            &config,
            &ResetOptions {
                backup: false,
                keep_custom_themes: true,
                rerun_onboarding: true,
            },
        )
        .expect("reset");
        assert!(result.backup_path.is_none());
        assert!(!config.join("bar.json").exists());
        assert!(!config.join("themes/dark.json").exists());
        assert_eq!(
            fs::read_to_string(config.join("themes/mine.json")).expect("read mine"),
            "{\"custom\":true}"
        );
        let _ = fs::remove_dir_all(&config);
    }

    #[test]
    fn reset_preserves_onboarding_when_requested() {
        let config = temp_config();
        fs::write(config.join("bar.json"), "{}").expect("bar");
        reset_metis_config_at(
            &config,
            &ResetOptions {
                backup: false,
                keep_custom_themes: false,
                rerun_onboarding: false,
            },
        )
        .expect("reset");
        let text = fs::read_to_string(config.join("config.json")).expect("config");
        assert!(text.contains("\"onboarding_complete\": true"));
        let _ = fs::remove_dir_all(&config);
    }
}
