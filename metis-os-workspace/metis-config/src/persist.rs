//! Atomic config file replace helpers for `~/.config/metis/*`.
//!
//! Write to a sibling `*.tmp` then `rename` so watchers never observe a truncated
//! JSON body mid-save (GFileMonitor / compositor mtime polls).

use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Sibling temp path: `foo.json` → `foo.json.tmp`, `appearance.mode` →
/// `appearance.mode.tmp`, extensionless `known_hosts` → `known_hosts.tmp`.
fn atomic_tmp_path(path: &Path) -> PathBuf {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => path.with_extension(format!("{ext}.tmp")),
        None => {
            let mut os = path.as_os_str().to_owned();
            os.push(".tmp");
            PathBuf::from(os)
        }
    }
}

fn map_permission(err: io::Error, path: &Path, writing: bool) -> io::Error {
    if err.kind() != io::ErrorKind::PermissionDenied {
        return err;
    }
    let verb = if writing { "writing" } else { "replacing" };
    io::Error::new(
        err.kind(),
        format!(
            "permission denied {verb} {} — is the file owned by root? \
             Run: sudo chown -R \"$USER:$USER\" ~/.config/metis",
            path.display()
        ),
    )
}

/// Atomically replace `path` with `bytes` (tmp + rename; remove tmp on rename failure).
pub fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = atomic_tmp_path(path);
    std::fs::write(&tmp, bytes).map_err(|e| map_permission(e, path, true))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        map_permission(e, path, false)
    })
}

/// Pretty-print `value` as JSON and atomically replace `path`.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    write_bytes_atomic(path, json.as_bytes())
}

/// First-run seed: write pretty JSON only when `path` does not already exist.
///
/// Uses `create_new` so a corrupt file (or a racing writer) is never clobbered —
/// important when `XDG_CONFIG_HOME` flips mid-load in parallel tests and when
/// live-reload watchers must not see a rewrite loop after a parse failure.
///
/// Returns `Ok(true)` when this call created the file, `Ok(false)` when it
/// already existed.
pub fn write_json_create_new<T: Serialize>(path: &Path, value: &T) -> io::Result<bool> {
    use std::io::Write;
    let json = serde_json::to_string_pretty(value).map_err(io::Error::other)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(mut file) => {
            file.write_all(json.as_bytes())
                .map_err(|e| map_permission(e, path, true))?;
            Ok(true)
        }
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(err) => Err(map_permission(err, path, true)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use std::fs;

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Sample {
        n: u32,
        s: String,
    }

    #[test]
    fn write_json_atomic_round_trip_and_cleans_tmp() {
        let dir = std::env::temp_dir().join(format!(
            "metis-persist-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("sample.json");
        let value = Sample {
            n: 7,
            s: "metis".into(),
        };
        write_json_atomic(&path, &value).expect("write");
        assert!(path.exists());
        assert!(
            !atomic_tmp_path(&path).exists(),
            "tmp must be gone after rename"
        );
        let text = fs::read_to_string(&path).expect("read");
        let loaded: Sample = serde_json::from_str(&text).expect("parse");
        assert_eq!(loaded, value);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_tmp_path_preserves_stem_extension() {
        assert_eq!(
            atomic_tmp_path(Path::new("/tmp/bar.json")),
            PathBuf::from("/tmp/bar.json.tmp")
        );
        assert_eq!(
            atomic_tmp_path(Path::new("/tmp/appearance.mode")),
            PathBuf::from("/tmp/appearance.mode.tmp")
        );
        assert_eq!(
            atomic_tmp_path(Path::new("/tmp/known_hosts")),
            PathBuf::from("/tmp/known_hosts.tmp")
        );
    }

    #[test]
    fn write_json_create_new_does_not_clobber() {
        let dir = std::env::temp_dir().join(format!(
            "metis-persist-create-new-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("seed.json");
        fs::write(&path, "{not valid json").expect("corrupt");
        let value = Sample {
            n: 1,
            s: "nope".into(),
        };
        assert!(!write_json_create_new(&path, &value).expect("create_new"));
        assert_eq!(fs::read_to_string(&path).expect("read"), "{not valid json");
        let _ = fs::remove_dir_all(&dir);
    }
}
