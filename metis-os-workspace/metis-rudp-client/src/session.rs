//! Quinn session: auth, control, video pump, input.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use metis_config::{
    fingerprint_cert_der, known_hosts_lookup, known_hosts_pin, normalize_fingerprint,
};
use metis_protocol::{
    DatagramHeader, RUDP_DATAGRAM_HEADER_LEN, RUDP_PROTOCOL_VERSION, ReliableAccessUnit,
    RudpControlMsg, encode_rudp_frame, is_audio_datagram, try_decode_audio_datagram,
    try_decode_rudp_frame, try_reassemble_media,
};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, SendStream};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName};
use tokio::sync::mpsc;

use crate::tofu::ClientError;

/// How to handle an unknown / mismatched host certificate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TofuMode {
    /// Pin unknown hosts automatically; fail on mismatch (smoke CLI).
    #[default]
    Auto,
    /// Do not pin: return [`ClientError::TofuUnknown`] / [`ClientError::TofuMismatch`].
    Interactive,
}

#[derive(Debug, Clone)]
pub struct RudpClientConfig {
    pub addr: SocketAddr,
    /// Key used in known_hosts (usually `host:port` as given by the user).
    pub host_key: String,
    pub username: String,
    pub password: String,
    pub tofu: TofuMode,
}

#[derive(Debug, Clone)]
pub struct AccessUnitEvent {
    pub data: Vec<u8>,
    pub codec: String,
    pub keyframe: bool,
    pub damage_full: bool,
    pub pts_us: i64,
}

#[derive(Debug, Clone)]
pub enum SessionEvent {
    VideoReady {
        width: u32,
        height: u32,
        codec: String,
    },
    AudioReady {
        sample_rate: u32,
        channels: u8,
        codec: String,
    },
    AudioPacket {
        seq: u32,
        pts_us: i64,
        data: Vec<u8>,
    },
    PointerLock {
        locked: bool,
    },
    ClipboardSet {
        mime: String,
        text: String,
        serial: u64,
    },
    AccessUnit(AccessUnitEvent),
    Disconnected,
}

/// Authenticated RUDP session. Drive with [`Self::recv_event`] / [`Self::send_input`].
pub struct RudpSession {
    events: mpsc::Receiver<SessionEvent>,
    input_tx: mpsc::Sender<RudpControlMsg>,
    session_id: String,
    _endpoint: Endpoint,
    _conn: Connection,
}

impl RudpSession {
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub async fn recv_event(&mut self) -> Option<SessionEvent> {
        self.events.recv().await
    }

    pub fn try_recv_event(&mut self) -> Option<SessionEvent> {
        self.events.try_recv().ok()
    }

    pub async fn send_input(&self, msg: RudpControlMsg) -> Result<(), ClientError> {
        self.input_tx
            .send(msg)
            .await
            .map_err(|_| ClientError::msg("input channel closed"))
    }
}

/// Connect, authenticate, and spawn media/control pumps.
pub async fn connect(cfg: RudpClientConfig) -> Result<RudpSession, ClientError> {
    ensure_rustls_provider();

    let verifier = Arc::new(TofuVerifier {
        host_key: cfg.host_key.clone(),
        mode: cfg.tofu,
        seen: std::sync::Mutex::new(None),
    });
    let mut crypto = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(verifier.clone())
        .with_no_client_auth();
    crypto.alpn_protocols.clear();

    let mut client_config = ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto)
            .map_err(|e| ClientError::msg(format!("quic client config: {e}")))?,
    ));
    let mut transport = quinn::TransportConfig::default();
    transport.datagram_receive_buffer_size(Some(2 * 1024 * 1024));
    client_config.transport_config(Arc::new(transport));

    let mut endpoint = Endpoint::client(
        "0.0.0.0:0"
            .parse()
            .map_err(|e| ClientError::msg(format!("bind: {e}")))?,
    )
    .map_err(|e| ClientError::msg(format!("client endpoint: {e}")))?;
    endpoint.set_default_client_config(client_config);

    let conn = endpoint
        .connect(cfg.addr, "localhost")
        .map_err(|e| ClientError::msg(format!("connect: {e}")))?
        .await
        .map_err(|e| map_handshake_err(e, &verifier))?;

    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| ClientError::msg(format!("open_bi: {e}")))?;

    write_msg(
        &mut send,
        &RudpControlMsg::Hello {
            protocol: RUDP_PROTOCOL_VERSION,
            username: cfg.username.clone(),
        },
    )
    .await?;

    match read_msg(&mut recv).await? {
        RudpControlMsg::AuthChallenge { .. } => {}
        RudpControlMsg::Reject { reason, detail } => {
            return Err(ClientError::Rejected {
                reason: format!(
                    "{}{}",
                    reason.as_str(),
                    detail.map(|d| format!(" ({d})")).unwrap_or_default()
                ),
            });
        }
        other => {
            return Err(ClientError::msg(format!(
                "unexpected after hello: {other:?}"
            )));
        }
    }

    write_msg(
        &mut send,
        &RudpControlMsg::AuthResponse {
            password: cfg.password.clone(),
        },
    )
    .await?;

    let session_id = match read_msg(&mut recv).await? {
        RudpControlMsg::SessionOk { session_id } => session_id,
        RudpControlMsg::Reject { reason, detail } => {
            return Err(ClientError::Rejected {
                reason: format!(
                    "{}{}",
                    reason.as_str(),
                    detail.map(|d| format!(" ({d})")).unwrap_or_default()
                ),
            });
        }
        other => {
            return Err(ClientError::msg(format!(
                "unexpected after auth: {other:?}"
            )));
        }
    };

    let (event_tx, event_rx) = mpsc::channel(64);
    let (input_tx, input_rx) = mpsc::channel::<RudpControlMsg>(256);

    let conn_pump = conn.clone();
    tokio::spawn(async move {
        session_pump(conn_pump, send, recv, event_tx, input_rx).await;
    });

    Ok(RudpSession {
        events: event_rx,
        input_tx,
        session_id,
        _endpoint: endpoint,
        _conn: conn,
    })
}

async fn session_pump(
    conn: Connection,
    mut control_send: SendStream,
    mut control_recv: RecvStream,
    event_tx: mpsc::Sender<SessionEvent>,
    mut input_rx: mpsc::Receiver<RudpControlMsg>,
) {
    let mut pending: HashMap<u64, FrameAssembly> = HashMap::new();
    let mut uni_buf = Vec::new();
    let mut video_recv: Option<RecvStream> = None;
    let mut control_buf = Vec::new();
    let mut ctrl_tmp = [0u8; 8192];
    let mut video_tmp = [0u8; 8192];
    let mut accepting = true;

    loop {
        tokio::select! {
            biased;
            msg = input_rx.recv() => {
                let Some(msg) = msg else { break; };
                if write_msg(&mut control_send, &msg).await.is_err() {
                    break;
                }
            }
            n = control_recv.read(&mut ctrl_tmp) => {
                match n {
                    Ok(Some(n)) => {
                        control_buf.extend_from_slice(&ctrl_tmp[..n]);
                        while let Ok(Some((msg, consumed))) = try_decode_rudp_frame(&control_buf) {
                            control_buf.drain(..consumed);
                            match msg {
                                RudpControlMsg::VideoReady { width, height, codec } => {
                                    let _ = event_tx.send(SessionEvent::VideoReady {
                                        width,
                                        height,
                                        codec,
                                    }).await;
                                }
                                RudpControlMsg::AudioReady {
                                    sample_rate,
                                    channels,
                                    codec,
                                } => {
                                    let _ = event_tx
                                        .send(SessionEvent::AudioReady {
                                            sample_rate,
                                            channels,
                                            codec,
                                        })
                                        .await;
                                }
                                RudpControlMsg::PointerLock { locked } => {
                                    let _ = event_tx.send(SessionEvent::PointerLock { locked }).await;
                                }
                                RudpControlMsg::ClipboardSet { mime, text, serial } => {
                                    let _ = event_tx
                                        .send(SessionEvent::ClipboardSet { mime, text, serial })
                                        .await;
                                }
                                RudpControlMsg::Keepalive => {}
                                other => {
                                    tracing::debug!(?other, "rudp-client: ignore control");
                                }
                            }
                        }
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            dgram = conn.read_datagram() => {
                match dgram {
                    Ok(bytes_buf) => {
                        if is_audio_datagram(&bytes_buf) {
                            if let Ok(pkt) = try_decode_audio_datagram(&bytes_buf) {
                                let _ = event_tx
                                    .send(SessionEvent::AudioPacket {
                                        seq: pkt.seq,
                                        pts_us: pkt.pts_us,
                                        data: pkt.payload,
                                    })
                                    .await;
                            }
                        } else if let Ok((hdr, total)) = DatagramHeader::decode(&bytes_buf) {
                            let payload = bytes_buf[RUDP_DATAGRAM_HEADER_LEN..total].to_vec();
                            let entry = pending.entry(hdr.frame_seq).or_insert_with(|| FrameAssembly {
                                pts_us: hdr.pts_us,
                                codec: hdr.codec,
                                damage_full: (hdr.flags & metis_protocol::FLAG_DAMAGE_FULL) != 0,
                                shard_n: hdr.shard_n as usize,
                                fec_m: hdr.fec_m as usize,
                                slots: vec![None; hdr.shard_n as usize + hdr.fec_m as usize],
                            });
                            let idx = hdr.shard_i as usize;
                            if idx < entry.slots.len() {
                                entry.slots[idx] = Some(payload);
                            }
                            if let Ok(Some(frame)) = try_reassemble_media(
                                hdr.frame_seq,
                                entry.pts_us,
                                entry.codec,
                                entry.damage_full,
                                entry.shard_n,
                                entry.fec_m,
                                &entry.slots,
                            ) {
                                pending.remove(&hdr.frame_seq);
                                let codec = metis_protocol::codec_to_str(frame.codec).to_string();
                                let _ = event_tx.send(SessionEvent::AccessUnit(AccessUnitEvent {
                                    data: frame.data,
                                    codec,
                                    keyframe: false,
                                    damage_full: frame.damage_full,
                                    pts_us: frame.pts_us,
                                })).await;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            accept = conn.accept_uni(), if accepting => {
                match accept {
                    Ok(stream) => {
                        video_recv = Some(stream);
                        accepting = false;
                    }
                    Err(_) => accepting = false,
                }
            }
            n = async {
                match video_recv.as_mut() {
                    Some(stream) => stream.read(&mut video_tmp).await,
                    None => {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        Ok(None)
                    }
                }
            } => {
                match n {
                    Ok(Some(n)) => {
                        uni_buf.extend_from_slice(&video_tmp[..n]);
                        while let Ok(Some((au, consumed))) =
                            ReliableAccessUnit::try_decode_framed(&uni_buf)
                        {
                            uni_buf.drain(..consumed);
                            let codec = metis_protocol::codec_to_str(au.codec).to_string();
                            let _ = event_tx.send(SessionEvent::AccessUnit(AccessUnitEvent {
                                data: au.data,
                                codec,
                                keyframe: true,
                                damage_full: au.damage_full,
                                pts_us: au.pts_us,
                            })).await;
                        }
                    }
                    Ok(None) => {
                        if video_recv.is_some() {
                            video_recv = None;
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }
    let _ = event_tx.send(SessionEvent::Disconnected).await;
}

struct FrameAssembly {
    pts_us: i64,
    codec: u8,
    damage_full: bool,
    shard_n: usize,
    fec_m: usize,
    slots: Vec<Option<Vec<u8>>>,
}

async fn write_msg(send: &mut SendStream, msg: &RudpControlMsg) -> Result<(), ClientError> {
    let bytes = encode_rudp_frame(msg).map_err(ClientError::msg)?;
    send.write_all(&bytes)
        .await
        .map_err(|e| ClientError::msg(format!("write: {e}")))
}

async fn read_msg(recv: &mut RecvStream) -> Result<RudpControlMsg, ClientError> {
    read_msg_timeout(recv, Duration::from_secs(30)).await
}

async fn read_msg_timeout(
    recv: &mut RecvStream,
    timeout: Duration,
) -> Result<RudpControlMsg, ClientError> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 2048];
    loop {
        if let Some((msg, n)) = try_decode_rudp_frame(&buf).map_err(ClientError::msg)? {
            buf.drain(..n);
            return Ok(msg);
        }
        let n = tokio::time::timeout(timeout, recv.read(&mut tmp))
            .await
            .map_err(|_| ClientError::msg("read timed out"))?
            .map_err(|e| ClientError::msg(format!("read: {e}")))?
            .ok_or_else(|| ClientError::msg("stream closed"))?;
        buf.extend_from_slice(&tmp[..n]);
    }
}

fn ensure_rustls_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

fn map_handshake_err(err: quinn::ConnectionError, verifier: &TofuVerifier) -> ClientError {
    if let Ok(guard) = verifier.seen.lock()
        && let Some(err) = guard.as_ref()
    {
        return match err {
            TofuSeen::Unknown(fp) => ClientError::TofuUnknown {
                fingerprint: fp.clone(),
            },
            TofuSeen::Mismatch { got, pinned } => ClientError::TofuMismatch {
                got: got.clone(),
                pinned: pinned.clone(),
            },
        };
    }
    ClientError::msg(format!("handshake: {err}"))
}

#[derive(Debug, Clone)]
enum TofuSeen {
    Unknown(String),
    Mismatch { got: String, pinned: String },
}

#[derive(Debug)]
struct TofuVerifier {
    host_key: String,
    mode: TofuMode,
    seen: std::sync::Mutex<Option<TofuSeen>>,
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
        match known_hosts_lookup(&self.host_key) {
            Some(pinned) if pinned == normalize_fingerprint(&fp) => {
                Ok(ServerCertVerified::assertion())
            }
            Some(pinned) => {
                if let Ok(mut g) = self.seen.lock() {
                    *g = Some(TofuSeen::Mismatch {
                        got: fp.clone(),
                        pinned: pinned.clone(),
                    });
                }
                Err(rustls::Error::General(format!(
                    "TOFU mismatch for {}: got {fp}, pinned {pinned}",
                    self.host_key
                )))
            }
            None => match self.mode {
                TofuMode::Auto => {
                    known_hosts_pin(&self.host_key, &fp).map_err(|e| {
                        rustls::Error::General(format!("TOFU pin write failed: {e}"))
                    })?;
                    tracing::info!(fingerprint = %fp, "TOFU: first connect — pinned");
                    Ok(ServerCertVerified::assertion())
                }
                TofuMode::Interactive => {
                    if let Ok(mut g) = self.seen.lock() {
                        *g = Some(TofuSeen::Unknown(fp.clone()));
                    }
                    Err(rustls::Error::General(format!(
                        "TOFU unknown host {}: {fp}",
                        self.host_key
                    )))
                }
            },
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
