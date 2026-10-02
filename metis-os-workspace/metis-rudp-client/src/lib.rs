//! Metis Remote Quinn client: TOFU, PAM auth, video reassembly, input send.

#![cfg_attr(not(test), deny(clippy::unwrap_used))]

mod session;
mod tofu;

pub use session::{
    AccessUnitEvent, RudpClientConfig, RudpSession, SessionEvent, TofuMode, connect,
};
pub use tofu::{ClientError, clear_host_pin, pin_host};
