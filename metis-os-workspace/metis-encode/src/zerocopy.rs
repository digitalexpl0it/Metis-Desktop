//! Phase 24 E — prefer GPU RGB→NV12 (VAAPI VPP / CUDA) over CPU mmap+swscale.
//!
//! Default: try zero-copy; `METIS_ENCODE_ZEROCOPY=0` forces the mmap path.
//! Soft encode never uses these helpers.

/// Whether to attempt GPU import + VPP on hardware encoder open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZerocopyPreference {
    /// Always use LINEAR mmap + swscale + `av_hwframe_transfer_data`.
    Off,
    /// Try zero-copy; fall back to mmap for the encoder instance on failure.
    Try,
}

/// Parse `METIS_ENCODE_ZEROCOPY` (`0` / `false` / `off` / `mmap` → Off; else Try).
pub fn zerocopy_preference() -> ZerocopyPreference {
    match std::env::var("METIS_ENCODE_ZEROCOPY") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            if matches!(v.as_str(), "0" | "false" | "off" | "no" | "mmap") {
                ZerocopyPreference::Off
            } else {
                ZerocopyPreference::Try
            }
        }
        Err(_) => ZerocopyPreference::Try,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preference_parses_off_values() {
        // SAFETY: test-only env mutation; serialised by calling once per process
        // in this unit (other crates should not race these keys in the same test
        // binary — metis-encode lib tests are single-threaded by default).
        unsafe {
            std::env::set_var("METIS_ENCODE_ZEROCOPY", "0");
            assert_eq!(zerocopy_preference(), ZerocopyPreference::Off);
            std::env::set_var("METIS_ENCODE_ZEROCOPY", "mmap");
            assert_eq!(zerocopy_preference(), ZerocopyPreference::Off);
            std::env::set_var("METIS_ENCODE_ZEROCOPY", "1");
            assert_eq!(zerocopy_preference(), ZerocopyPreference::Try);
            std::env::remove_var("METIS_ENCODE_ZEROCOPY");
            assert_eq!(zerocopy_preference(), ZerocopyPreference::Try);
        }
    }
}
