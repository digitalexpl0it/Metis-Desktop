//! Metis Remote (RUDP) host preferences — `~/.config/metis/rudp.json`.
//!
//! Primary low-latency streaming path. Classic RDP/GRD stays in `remote.json`.
//! Credentials are never stored here; `allowed_users` are local PAM account names.
//! Host TLS identity lives under `~/.config/metis/rudp/` (`host.crt` / `host.key`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default Quinn / RUDP listen port (must match compositor `DEFAULT_RUDP_PORT`).
pub const DEFAULT_RUDP_PORT: u16 = 7843;
/// Default target bitrate for hardware encode (kbps).
pub const DEFAULT_RUDP_BITRATE_KBPS: u32 = 25_000;

/// Hardware encode backend preference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RudpEncoderBackend {
    #[default]
    Auto,
    Vaapi,
    Nvenc,
}

impl RudpEncoderBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Vaapi => "vaapi",
            Self::Nvenc => "nvenc",
        }
    }
}

/// Preferred video codec (open ladder may fall back to the other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RudpVideoCodec {
    #[default]
    Hevc,
    H264,
}

impl RudpVideoCodec {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Hevc => "hevc",
            Self::H264 => "h264",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RudpConfig {
    /// Host should listen and arm compositor frame export.
    #[serde(default)]
    pub enabled: bool,
    /// UDP listen port (Quinn). Clamped to 1024–65535 on save/load.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Prefer LAN-only exposure; Settings applies nft/ufw when true.
    #[serde(default = "default_true")]
    pub lan_only: bool,
    /// Local usernames allowed to authenticate (PAM). Empty while disabled is
    /// fine; on first enable Settings seeds `$USER`.
    #[serde(default)]
    pub allowed_users: Vec<String>,
    /// Hardware encode backend: Auto (NVIDIA→NVENC else VAAPI), VAAPI, or NVENC.
    #[serde(default)]
    pub encoder: RudpEncoderBackend,
    /// Preferred codec; open tries this first then the other of HEVC/H.264.
    #[serde(default)]
    pub codec: RudpVideoCodec,
    /// Target encode bitrate in kbps.
    #[serde(default = "default_bitrate")]
    pub bitrate_kbps: u32,
    /// Whether Metis believes LAN-only UDP firewall rules are present.
    #[serde(default)]
    pub firewall_applied: bool,
    /// Backend that owns the rules: `nft`, `ufw`, or empty.
    #[serde(default)]
    pub firewall_backend: String,
    /// Last firewall error (shown in Settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub firewall_last_error: Option<String>,
}

fn default_port() -> u16 {
    DEFAULT_RUDP_PORT
}

fn default_true() -> bool {
    true
}

fn default_bitrate() -> u32 {
    DEFAULT_RUDP_BITRATE_KBPS
}

impl Default for RudpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_port(),
            lan_only: default_true(),
            allowed_users: Vec::new(),
            encoder: RudpEncoderBackend::default(),
            codec: RudpVideoCodec::default(),
            bitrate_kbps: default_bitrate(),
            firewall_applied: false,
            firewall_backend: String::new(),
            firewall_last_error: None,
        }
    }
}

impl RudpConfig {
    /// Clamp port/bitrate and drop empty / duplicate usernames.
    pub fn sanitize(mut self) -> Self {
        if !(1024..=65535).contains(&self.port) {
            self.port = DEFAULT_RUDP_PORT;
        }
        if self.bitrate_kbps < 500 {
            self.bitrate_kbps = DEFAULT_RUDP_BITRATE_KBPS;
        }
        if self.bitrate_kbps > 200_000 {
            self.bitrate_kbps = 200_000;
        }
        let mut seen = std::collections::HashSet::new();
        self.allowed_users = self
            .allowed_users
            .into_iter()
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty() && seen.insert(u.clone()))
            .collect();
        self
    }

    /// If enabling with an empty allowlist, insert `username` (typically `$USER`).
    pub fn seed_current_user_if_needed(&mut self, username: &str) {
        let name = username.trim();
        if name.is_empty() {
            return;
        }
        if self.enabled && self.allowed_users.is_empty() {
            self.allowed_users.push(name.to_string());
        }
    }
}

pub fn rudp_config_path() -> PathBuf {
    super::config_dir().join("rudp.json")
}

/// Directory for RUDP host cert, key, identity meta, and client known_hosts.
pub fn rudp_dir() -> PathBuf {
    super::config_dir().join("rudp")
}

pub fn rudp_host_cert_path() -> PathBuf {
    rudp_dir().join("host.crt")
}

pub fn rudp_host_key_path() -> PathBuf {
    rudp_dir().join("host.key")
}

pub fn rudp_identity_meta_path() -> PathBuf {
    rudp_dir().join("identity.json")
}

pub fn rudp_known_hosts_path() -> PathBuf {
    rudp_dir().join("known_hosts")
}

/// On-disk TOFU metadata written when the host cert is created/ensured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RudpIdentityMeta {
    /// SHA-256 of the certificate DER, colon-separated lowercase hex.
    pub fingerprint: String,
}

/// SHA-256 fingerprint of a certificate DER (colon-hex lowercase).
pub fn fingerprint_cert_der(der: &[u8]) -> String {
    let hash = Sha256::digest(der);
    hash.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// Format a fingerprint for display (already colon-hex; no-op normalize).
pub fn normalize_fingerprint(fp: &str) -> String {
    fp.trim().to_ascii_lowercase()
}

pub fn load_rudp_identity_meta() -> Option<RudpIdentityMeta> {
    let path = rudp_identity_meta_path();
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn save_rudp_identity_meta(meta: &RudpIdentityMeta) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    std::fs::create_dir_all(rudp_dir())?;
    let path = rudp_identity_meta_path();
    let json = serde_json::to_string_pretty(meta).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Read SHA-256 fingerprint from `identity.json`, or compute from `host.crt` PEM.
pub fn read_rudp_host_fingerprint() -> Option<String> {
    if let Some(meta) = load_rudp_identity_meta() {
        return Some(normalize_fingerprint(&meta.fingerprint));
    }
    let pem = std::fs::read_to_string(rudp_host_cert_path()).ok()?;
    let der = pem_cert_to_der(&pem)?;
    Some(fingerprint_cert_der(&der))
}

fn pem_cert_to_der(pem: &str) -> Option<Vec<u8>> {
    let mut b64 = String::new();
    let mut in_cert = false;
    for line in pem.lines() {
        let line = line.trim();
        if line.starts_with("-----BEGIN CERTIFICATE-----") {
            in_cert = true;
            continue;
        }
        if line.starts_with("-----END CERTIFICATE-----") {
            break;
        }
        if in_cert {
            b64.push_str(line);
        }
    }
    if b64.is_empty() {
        return None;
    }
    base64_decode(&b64)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // Minimal base64 (std alphabet) for PEM bodies — no extra crate.
    fn val(c: u8) -> Option<u8> {
        match c {
            b'A'..=b'Z' => Some(c - b'A'),
            b'a'..=b'z' => Some(c - b'a' + 26),
            b'0'..=b'9' => Some(c - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 4 <= bytes.len() {
        let (a, b, c, d) = (bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]);
        i += 4;
        let pad_c = a == b'=' || b == b'='; // invalid
        if pad_c {
            return None;
        }
        let va = val(a)?;
        let vb = val(b)?;
        out.push((va << 2) | (vb >> 4));
        if c == b'=' {
            break;
        }
        let vc = val(c)?;
        out.push(((vb & 0x0f) << 4) | (vc >> 2));
        if d == b'=' {
            break;
        }
        let vd = val(d)?;
        out.push(((vc & 0x03) << 6) | vd);
    }
    Some(out)
}

/// TOFU known_hosts helpers (smoke CLI / future Viewer).
pub fn known_hosts_lookup(host_port: &str) -> Option<String> {
    let path = rudp_known_hosts_path();
    let text = std::fs::read_to_string(path).ok()?;
    let key = host_port.trim().to_ascii_lowercase();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(hp) = parts.next() else {
            continue;
        };
        let Some(fp) = parts.next() else {
            continue;
        };
        if hp.eq_ignore_ascii_case(&key) {
            return Some(normalize_fingerprint(fp));
        }
    }
    None
}

pub fn known_hosts_pin(host_port: &str, fingerprint: &str) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    std::fs::create_dir_all(rudp_dir())?;
    let path = rudp_known_hosts_path();
    let key = host_port.trim().to_ascii_lowercase();
    let fp = normalize_fingerprint(fingerprint);
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(&path)?
            .lines()
            .filter(|l| {
                let t = l.trim();
                if t.is_empty() || t.starts_with('#') {
                    return true;
                }
                let hp = t.split_whitespace().next().unwrap_or("");
                !hp.eq_ignore_ascii_case(&key)
            })
            .map(str::to_string)
            .collect()
    } else {
        Vec::new()
    };
    lines.push(format!("{key} {fp}"));
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, lines.join("\n") + "\n")?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

pub fn known_hosts_remove(host_port: &str) -> std::io::Result<bool> {
    let path = rudp_known_hosts_path();
    if !path.exists() {
        return Ok(false);
    }
    let key = host_port.trim().to_ascii_lowercase();
    let text = std::fs::read_to_string(&path)?;
    let mut removed = false;
    let lines: Vec<String> = text
        .lines()
        .filter(|l| {
            let t = l.trim();
            if t.is_empty() || t.starts_with('#') {
                return true;
            }
            let hp = t.split_whitespace().next().unwrap_or("");
            if hp.eq_ignore_ascii_case(&key) {
                removed = true;
                false
            } else {
                true
            }
        })
        .map(str::to_string)
        .collect();
    if removed {
        std::fs::write(&path, lines.join("\n") + "\n")?;
    }
    Ok(removed)
}

pub fn ensure_rudp_dir() -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    std::fs::create_dir_all(rudp_dir())?;
    Ok(())
}

pub fn load_rudp_config() -> RudpConfig {
    let path = rudp_config_path();
    if path.exists()
        && let Ok(text) = std::fs::read_to_string(&path)
    {
        if let Ok(cfg) = serde_json::from_str::<RudpConfig>(&text) {
            return cfg.sanitize();
        }
        tracing::warn!("rudp.json parse failed — using defaults");
    }
    RudpConfig::default()
}

pub fn save_rudp_config(cfg: &RudpConfig) -> std::io::Result<()> {
    super::ensure_config_dirs()?;
    let cfg = cfg.clone().sanitize();
    let path = rudp_config_path();
    let json = serde_json::to_string_pretty(&cfg).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &json)?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// True when `path` looks like an existing PEM certificate file.
pub fn host_cert_exists() -> bool {
    Path::new(&rudp_host_cert_path()).is_file() && Path::new(&rudp_host_key_path()).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_clamps_port() {
        let cfg = RudpConfig {
            port: 80,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(cfg.port, DEFAULT_RUDP_PORT);
    }

    #[test]
    fn sanitize_clamps_bitrate() {
        let low = RudpConfig {
            bitrate_kbps: 100,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(low.bitrate_kbps, DEFAULT_RUDP_BITRATE_KBPS);
        let high = RudpConfig {
            bitrate_kbps: 500_000,
            ..Default::default()
        }
        .sanitize();
        assert_eq!(high.bitrate_kbps, 200_000);
    }

    #[test]
    fn seed_only_when_enabled_and_empty() {
        let mut cfg = RudpConfig::default();
        cfg.seed_current_user_if_needed("alice");
        assert!(cfg.allowed_users.is_empty());
        cfg.enabled = true;
        cfg.seed_current_user_if_needed("alice");
        assert_eq!(cfg.allowed_users, vec!["alice".to_string()]);
        cfg.seed_current_user_if_needed("bob");
        assert_eq!(cfg.allowed_users, vec!["alice".to_string()]);
    }

    #[test]
    fn encode_defaults_roundtrip() {
        let json = serde_json::to_string(&RudpConfig::default()).expect("ser");
        let cfg: RudpConfig = serde_json::from_str(&json).expect("de");
        assert_eq!(cfg.encoder, RudpEncoderBackend::Auto);
        assert_eq!(cfg.codec, RudpVideoCodec::Hevc);
        assert_eq!(cfg.bitrate_kbps, DEFAULT_RUDP_BITRATE_KBPS);
    }

    #[test]
    fn fingerprint_stable() {
        let fp = fingerprint_cert_der(b"test-der");
        assert!(fp.contains(':'));
        assert_eq!(fp, fingerprint_cert_der(b"test-der"));
    }

    #[test]
    fn pem_roundtrip_decode() {
        // "hi" as base64 is aGkg (with padding aGk=)
        let pem = "-----BEGIN CERTIFICATE-----\naGk=\n-----END CERTIFICATE-----\n";
        let der = pem_cert_to_der(pem).expect("der");
        assert_eq!(der, b"hi");
    }
}
