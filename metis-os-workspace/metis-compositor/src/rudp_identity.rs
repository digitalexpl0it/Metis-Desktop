//! Persistent RUDP host TLS identity (`~/.config/metis/rudp/host.{crt,key}`).

use std::fs;
use std::sync::Arc;

use metis_config::{
    RudpIdentityMeta, ensure_rudp_dir, fingerprint_cert_der, host_cert_exists, rudp_host_cert_path,
    rudp_host_key_path, save_rudp_identity_meta,
};
use quinn::ServerConfig;
use rcgen::{CertificateParams, KeyPair};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// Ensure host cert/key exist; return Quinn [`ServerConfig`] + fingerprint.
pub fn load_or_create_server_config() -> Result<(ServerConfig, String), String> {
    ensure_rudp_dir().map_err(|e| format!("rudp identity dir: {e}"))?;

    let (cert_der, key_der, fingerprint) = if host_cert_exists() {
        load_existing()?
    } else {
        create_new()?
    };

    let mut server_config = ServerConfig::with_single_cert(vec![cert_der], key_der)
        .map_err(|e| format!("rudp ServerConfig: {e}"))?;

    let mut transport = quinn::TransportConfig::default();
    let idle = std::time::Duration::from_secs(30)
        .try_into()
        .map_err(|e| format!("rudp idle timeout: {e}"))?;
    transport.max_idle_timeout(Some(idle));
    transport.keep_alive_interval(Some(std::time::Duration::from_secs(2)));
    server_config.transport_config(Arc::new(transport));

    Ok((server_config, fingerprint))
}

fn create_new() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>, String), String> {
    let mut names = vec!["localhost".to_string()];
    if let Some(h) = read_hostname()
        && !h.is_empty()
        && h != "localhost"
    {
        names.push(h);
    }
    let key = KeyPair::generate().map_err(|e| format!("rudp keygen: {e}"))?;
    let cert = CertificateParams::new(names)
        .map_err(|e| format!("rudp cert params: {e}"))?
        .self_signed(&key)
        .map_err(|e| format!("rudp self-sign: {e}"))?;

    let cert_pem = cert.pem();
    let key_pem = key.serialize_pem();
    let cert_path = rudp_host_cert_path();
    let key_path = rudp_host_key_path();
    fs::write(&cert_path, &cert_pem).map_err(|e| format!("write {}: {e}", cert_path.display()))?;
    fs::write(&key_path, &key_pem).map_err(|e| format!("write {}: {e}", key_path.display()))?;
    // Restrict key permissions best-effort.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&key_path, fs::Permissions::from_mode(0o600));
    }

    let der = cert.der().to_vec();
    let fingerprint = fingerprint_cert_der(&der);
    save_rudp_identity_meta(&RudpIdentityMeta {
        fingerprint: fingerprint.clone(),
    })
    .map_err(|e| format!("write identity.json: {e}"))?;

    tracing::info!(%fingerprint, "rudp host: created persistent TLS identity");

    Ok((
        CertificateDer::from(der),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        fingerprint,
    ))
}

fn load_existing() -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>, String), String> {
    let cert_pem =
        fs::read_to_string(rudp_host_cert_path()).map_err(|e| format!("read host.crt: {e}"))?;
    let key_pem =
        fs::read_to_string(rudp_host_key_path()).map_err(|e| format!("read host.key: {e}"))?;

    let cert_der = pem_extract_der(&cert_pem, "CERTIFICATE")
        .ok_or_else(|| "host.crt: no CERTIFICATE block".to_string())?;
    let key_der = pem_extract_der(&key_pem, "PRIVATE KEY")
        .or_else(|| pem_extract_der(&key_pem, "RSA PRIVATE KEY"))
        .ok_or_else(|| "host.key: no PRIVATE KEY block".to_string())?;

    let fingerprint = fingerprint_cert_der(&cert_der);
    let _ = save_rudp_identity_meta(&RudpIdentityMeta {
        fingerprint: fingerprint.clone(),
    });

    tracing::info!(%fingerprint, "rudp host: loaded persistent TLS identity");

    Ok((
        CertificateDer::from(cert_der),
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der)),
        fingerprint,
    ))
}

fn pem_extract_der(pem: &str, label: &str) -> Option<Vec<u8>> {
    let begin = format!("-----BEGIN {label}-----");
    let end = format!("-----END {label}-----");
    let mut b64 = String::new();
    let mut inside = false;
    for line in pem.lines() {
        let line = line.trim();
        if line.starts_with(&begin) {
            inside = true;
            continue;
        }
        if line.starts_with(&end) {
            break;
        }
        if inside {
            b64.push_str(line);
        }
    }
    if b64.is_empty() {
        return None;
    }
    base64_decode(&b64)
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
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

fn read_hostname() -> Option<String> {
    if let Ok(h) = std::env::var("HOSTNAME")
        && !h.is_empty()
    {
        return Some(h);
    }
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..len].to_vec()).ok()
}
