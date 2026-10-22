//! Metis Remote (RUDP) network host (Phase 23 §D).
//!
//! The library holds everything that terminates the network side of Metis
//! Remote — Quinn / TLS identity ([`rudp_identity`]), per-IP auth rate limits
//! ([`rudp_auth_guard`]), PAM ([`pam_auth`]) and the session / fan-out
//! [`engine`]. Two binaries drive it:
//!
//! - `metis-rudp-host` (default): a standalone process the compositor spawns
//!   and talks to over a private Unix socket ([`ipc_sock`],
//!   `metis_protocol::rudp_host_ipc`). A bug in the network edge costs one
//!   process, not the Wayland session.
//! - `metis-compositor` with `METIS_RUDP_INPROCESS=1`: the emergency fallback
//!   that runs the same engine on a compositor thread (one release only).
//!
//! The compositor also reuses [`pam_auth`] for the lock screen.
#![cfg_attr(not(test), deny(clippy::unwrap_used))]

pub mod engine;
pub mod ipc_sock;
pub mod pam_auth;
pub mod rudp_auth_guard;
pub mod rudp_identity;
