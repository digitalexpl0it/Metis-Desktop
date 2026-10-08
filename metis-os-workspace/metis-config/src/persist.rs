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
}
