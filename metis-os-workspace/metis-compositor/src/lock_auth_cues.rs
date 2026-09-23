//! Soft-fail detection of lock-screen biometric / security-key cues.
//!
//! Detection is UI-only: real unlock still goes through host PAM
//! (`pam_fprintd` / `pam_u2f`). Prefer `false` on any I/O error so password-only
//! installs never show a misleading “touch sensor” hint.

use std::path::Path;

/// Yubico USB vendor id (hex, as sysfs `idVendor`).
const YUBICO_VENDOR: &str = "1050";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AuthCues {
    pub fingerprint: bool,
    pub security_key: bool,
}

impl AuthCues {
    pub fn any(self) -> bool {
        self.fingerprint || self.security_key
    }

    /// Fluent message id for the lock status / placeholder cue line.
    pub fn status_ftl_id(self) -> Option<&'static str> {
        match (self.fingerprint, self.security_key) {
            (true, true) => Some("lock-cue-fingerprint-or-key"),
            (true, false) => Some("lock-cue-fingerprint"),
            (false, true) => Some("lock-cue-security-key"),
            (false, false) => None,
        }
    }
}

/// Probe the running system for fingerprint / YubiKey cues.
pub fn detect_auth_cues() -> AuthCues {
    let user = std::env::var("USER")
        .ok()
        .filter(|u| !u.is_empty())
        .or_else(|| std::env::var("LOGNAME").ok().filter(|u| !u.is_empty()));
    detect_auth_cues_at(
        Path::new("/sys/class/fingerprint"),
        Path::new("/var/lib/fprint"),
        Path::new("/sys/bus/usb/devices"),
        user.as_deref(),
    )
}

/// Testable probe with explicit paths.
pub fn detect_auth_cues_at(
    fingerprint_sysfs: &Path,
    fprint_lib_dir: &Path,
    usb_devices: &Path,
    username: Option<&str>,
) -> AuthCues {
    AuthCues {
        fingerprint: fingerprint_available(fingerprint_sysfs, fprint_lib_dir, username),
        security_key: yubico_usb_present(usb_devices),
    }
}

fn fingerprint_available(
    fingerprint_sysfs: &Path,
    fprint_lib_dir: &Path,
    username: Option<&str>,
) -> bool {
    // Prefer hardware / enrollment signals — not merely an installed fprintd
    // binary (that would auto-attempt PAM and fail on password-only stacks).
    if dir_has_entries(fingerprint_sysfs) {
        return true;
    }
    if let Some(user) = username {
        let enroll = fprint_lib_dir.join(user);
        if dir_has_entries(&enroll) {
            return true;
        }
    }
    false
}

fn yubico_usb_present(usb_devices: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(usb_devices) else {
        return false;
    };
    for entry in entries.flatten() {
        let vendor = entry.path().join("idVendor");
        if let Ok(text) = std::fs::read_to_string(&vendor)
            && text.trim().eq_ignore_ascii_case(YUBICO_VENDOR)
        {
            return true;
        }
    }
    false
}

fn dir_has_entries(path: &Path) -> bool {
    let Ok(mut rd) = std::fs::read_dir(path) else {
        return false;
    };
    rd.next().is_some()
}

/// Helper for tests that need a writable fake tree under a temp dir.
#[cfg(test)]
pub fn ensure_dir(path: &Path) -> std::path::PathBuf {
    std::fs::create_dir_all(path).expect("mkdir");
    path.to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_devices_means_no_cues() {
        let root =
            std::env::temp_dir().join(format!("metis-auth-cues-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fp = ensure_dir(&root.join("fingerprint"));
        let fprint = ensure_dir(&root.join("fprint"));
        let usb = ensure_dir(&root.join("usb"));
        let cues = detect_auth_cues_at(&fp, &fprint, &usb, Some("alice"));
        assert!(!cues.any());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn fingerprint_sysfs_and_enrollment() {
        let root = std::env::temp_dir().join(format!("metis-auth-cues-fp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fp = ensure_dir(&root.join("fingerprint"));
        std::fs::write(fp.join("dev0"), b"").unwrap();
        let fprint = ensure_dir(&root.join("fprint"));
        let usb = ensure_dir(&root.join("usb"));
        let cues = detect_auth_cues_at(&fp, &fprint, &usb, Some("alice"));
        assert!(cues.fingerprint);
        assert!(!cues.security_key);

        // enrollment path alone
        let fp_empty = ensure_dir(&root.join("fingerprint-empty"));
        let enroll = ensure_dir(&fprint.join("bob"));
        std::fs::write(enroll.join("print0"), b"").unwrap();
        let cues = detect_auth_cues_at(&fp_empty, &fprint, &usb, Some("bob"));
        assert!(cues.fingerprint);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn yubico_vendor_detected() {
        let root = std::env::temp_dir().join(format!("metis-auth-cues-yk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let fp = ensure_dir(&root.join("fingerprint"));
        let fprint = ensure_dir(&root.join("fprint"));
        let usb = ensure_dir(&root.join("usb"));
        let dev = ensure_dir(&usb.join("1-2"));
        std::fs::write(dev.join("idVendor"), b"1050\n").unwrap();
        let cues = detect_auth_cues_at(&fp, &fprint, &usb, None);
        assert!(cues.security_key);
        assert_eq!(cues.status_ftl_id(), Some("lock-cue-security-key"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
