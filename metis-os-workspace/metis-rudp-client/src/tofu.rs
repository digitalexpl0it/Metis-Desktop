//! Client errors and TOFU helpers.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("{0}")]
    Message(String),
    #[error("TOFU: unknown host fingerprint {fingerprint} (pin to trust)")]
    TofuUnknown { fingerprint: String },
    #[error("TOFU mismatch: got {got}, pinned {pinned}")]
    TofuMismatch { got: String, pinned: String },
    #[error("rejected: {reason}")]
    Rejected { reason: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

impl ClientError {
    pub fn msg(s: impl Into<String>) -> Self {
        Self::Message(s.into())
    }
}

/// Pin `fingerprint` for `host_port` in known_hosts.
pub fn pin_host(host_port: &str, fingerprint: &str) -> Result<(), ClientError> {
    metis_config::known_hosts_pin(host_port, fingerprint).map_err(ClientError::from)
}

/// Remove a known_hosts pin for `host_port`. Returns whether a pin existed.
pub fn clear_host_pin(host_port: &str) -> Result<bool, ClientError> {
    metis_config::known_hosts_remove(host_port).map_err(ClientError::from)
}
