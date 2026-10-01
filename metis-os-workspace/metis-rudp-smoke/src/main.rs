//! Metis Remote Phase 4 smoke client: QUIC + TOFU + PAM auth (no video).
//!
//! Usage:
//!   metis-rudp-smoke <host:port> --user <name>
//!   Password on stdin (or METIS_RUDP_PASSWORD).
//!   --tofu-reset clears the known_hosts pin for this host.

use std::io::{Read, Write};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use metis_config::{
    fingerprint_cert_der, known_hosts_lookup, known_hosts_pin, known_hosts_remove,
    normalize_fingerprint,
};
use metis_protocol::{
    RUDP_PROTOCOL_VERSION, RudpControlMsg, encode_rudp_frame, try_decode_rudp_frame,
};
use quinn::{ClientConfig, Endpoint};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName};
use zeroize::Zeroize;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_rudp_smoke=info,warn".into()),
        )
        .init();

    let code = match run(std::env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err(err) => {
            eprintln!("metis-rudp-smoke: {err}");
            1
        }
    };
    std::process::exit(code);
}

fn run(args: Vec<String>) -> Result<(), String> {
    if rustls::crypto::ring::default_provider()
        .install_default()
        .is_err()
    {
        // already installed
    }

    let mut host_port = None;
    let mut user = None;
    let mut tofu_reset = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--user" | "-u" => {
                i += 1;
                user = args.get(i).cloned();
            }
            "--tofu-reset" => tofu_reset = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other if !other.starts_with('-') && host_port.is_none() => {
                host_port = Some(other.to_string());
            }
            other => return Err(format!("unknown argument: {other}")),
        }
        i += 1;
    }

    let host_port = host_port.ok_or_else(|| {
        "usage: metis-rudp-smoke <host:port> --user <name> [--tofu-reset]".to_string()
    })?;
    let user = user.ok_or_else(|| "--user <name> required".to_string())?;

    if tofu_reset {
        let removed = known_hosts_remove(&host_port).map_err(|e| e.to_string())?;
        eprintln!(
            "TOFU: {}",
            if removed {
                "cleared pin for {host_port}"
            } else {
                "no pin to clear"
            }
            .replace("{host_port}", &host_port)
        );
    }

    let addr: SocketAddr = host_port
        .parse()
        .map_err(|e| format!("invalid host:port '{host_port}': {e}"))?;

    let mut password = if let Ok(p) = std::env::var("METIS_RUDP_PASSWORD") {
        p
    } else {
        eprint!("Password: ");
        let _ = std::io::stderr().flush();
        let mut p = String::new();
        std::io::stdin()
            .read_to_string(&mut p)
            .map_err(|e| format!("read password: {e}"))?;
        p.trim_end_matches(['\r', '\n']).to_string()
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("tokio: {e}"))?;

    let result = rt.block_on(async { smoke_session(addr, &host_port, &user, &password).await });
    password.zeroize();
    result
}

async fn smoke_session(
    addr: SocketAddr,
    host_port: &str,
    user: &str,
    password: &str,
) -> Result<(), String> {
    let verifier = Arc::new(TofuVerifier {
        host_port: host_port.to_string(),
    });
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"metis-rudp".to_vec()];

    // Quinn ignores ALPN mismatch if server doesn't set it — clear ALPN to match host.
    crypto.alpn_protocols.clear();

    let client_config = ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .map_err(|e| format!("quic client config: {e}"))?,
    ));

    let mut endpoint = Endpoint::client("0.0.0.0:0".parse().unwrap())
        .map_err(|e| format!("client endpoint: {e}"))?;
    endpoint.set_default_client_config(client_config);

    let server_name = match addr {
        SocketAddr::V4(_) => "localhost",
        SocketAddr::V6(_) => "localhost",
    };
    let conn = endpoint
        .connect(addr, server_name)
        .map_err(|e| format!("connect: {e}"))?
        .await
        .map_err(|e| format!("handshake: {e}"))?;

    let (mut send, mut recv) = conn.open_bi().await.map_err(|e| format!("open_bi: {e}"))?;

    write_msg(
        &mut send,
        &RudpControlMsg::Hello {
            protocol: RUDP_PROTOCOL_VERSION,
            username: user.to_string(),
        },
    )
    .await?;

    match read_msg(&mut recv).await? {
        RudpControlMsg::AuthChallenge { .. } => {}
        RudpControlMsg::Reject { reason, detail } => {
            return Err(format!(
                "rejected: {}{}",
                reason.as_str(),
                detail.map(|d| format!(" ({d})")).unwrap_or_default()
            ));
        }
        other => return Err(format!("unexpected after hello: {other:?}")),
    }

    write_msg(
        &mut send,
        &RudpControlMsg::AuthResponse {
            password: password.to_string(),
        },
    )
    .await?;

    match read_msg(&mut recv).await? {
        RudpControlMsg::SessionOk { session_id } => {
            println!("SessionOk {session_id}");
            Ok(())
        }
        RudpControlMsg::Reject { reason, detail } => Err(format!(
            "rejected: {}{}",
            reason.as_str(),
            detail.map(|d| format!(" ({d})")).unwrap_or_default()
        )),
        other => Err(format!("unexpected after auth: {other:?}")),
    }
}

async fn write_msg(send: &mut quinn::SendStream, msg: &RudpControlMsg) -> Result<(), String> {
    let bytes = encode_rudp_frame(msg)?;
    send.write_all(&bytes)
        .await
        .map_err(|e| format!("write: {e}"))
}

async fn read_msg(recv: &mut quinn::RecvStream) -> Result<RudpControlMsg, String> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        if let Some((msg, n)) = try_decode_rudp_frame(&buf)? {
            buf.drain(..n);
            return Ok(msg);
        }
        let n = tokio::time::timeout(Duration::from_secs(30), recv.read(&mut tmp))
            .await
            .map_err(|_| "read timed out".to_string())?
            .map_err(|e| format!("read: {e}"))?
            .ok_or_else(|| "stream closed".to_string())?;
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn print_help() {
    println!(
        "metis-rudp-smoke — Metis Remote auth smoke test\n\n\
         Usage:\n  \
         metis-rudp-smoke <host:port> --user <name> [--tofu-reset]\n\n\
         Password: stdin or METIS_RUDP_PASSWORD.\n\
         TOFU pins live in ~/.config/metis/rudp/known_hosts."
    );
}

#[derive(Debug)]
struct TofuVerifier {
    host_port: String,
}

impl ServerCertVerifier for TofuVerifier {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        let fp = fingerprint_cert_der(end_entity.as_ref());
        match known_hosts_lookup(&self.host_port) {
            Some(pinned) if pinned == normalize_fingerprint(&fp) => {
                Ok(ServerCertVerified::assertion())
            }
            Some(pinned) => Err(rustls::Error::General(format!(
                "TOFU mismatch for {}: got {fp}, pinned {pinned} (use --tofu-reset)",
                self.host_port
            ))),
            None => {
                known_hosts_pin(&self.host_port, &fp)
                    .map_err(|e| rustls::Error::General(format!("TOFU pin write failed: {e}")))?;
                eprintln!("TOFU: first connect — pinned {fp}");
                Ok(ServerCertVerified::assertion())
            }
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
