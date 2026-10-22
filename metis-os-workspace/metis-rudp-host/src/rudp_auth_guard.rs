//! Per-peer IP auth rate limiting for the Metis Remote (RUDP) host.
//!
//! Pure logic — no Quinn/PAM. Caps concurrent unauthenticated handshakes and
//! authenticated sessions; lockouts emit [`metis_protocol::RudpRejectReason::RateLimited`].

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Failed PAM/auth attempts per IP inside [`FAIL_WINDOW`] before lockout.
pub const MAX_FAILS_PER_WINDOW: usize = 5;
/// Sliding window for counting failures.
pub const FAIL_WINDOW: Duration = Duration::from_secs(60);
/// How long an IP stays locked out after tripping the failure budget.
pub const LOCKOUT: Duration = Duration::from_secs(60);
/// Concurrent TLS connections mid-auth (Hello → SessionOk).
pub const MAX_IN_FLIGHT_AUTH: usize = 8;
/// Concurrent authenticated sessions.
pub const MAX_AUTHENTICATED_SESSIONS: usize = 4;
/// Base delay after a failed auth; doubles per failure in the window (capped).
pub const BASE_BACKOFF: Duration = Duration::from_millis(400);
const MAX_BACKOFF_EXP: u32 = 4; // 400ms * 2^4 = 6.4s

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthAdmitDeny {
    /// Peer IP is in lockout — send `rate_limited`.
    RateLimited,
    /// Too many handshakes or sessions — close without PAM.
    CapExceeded,
}

#[derive(Debug, Default)]
struct PeerState {
    failures: VecDeque<Instant>,
    locked_until: Option<Instant>,
}

/// Shared host-side auth admission state.
#[derive(Debug, Default)]
pub struct AuthGuard {
    peers: HashMap<IpAddr, PeerState>,
    in_flight: usize,
    sessions: usize,
}

impl AuthGuard {
    /// Admit a new auth handshake for `ip`. Increments `in_flight` on success.
    pub fn try_begin_auth(&mut self, ip: IpAddr, now: Instant) -> Result<(), AuthAdmitDeny> {
        self.evict_stale(now);
        if self.is_locked_out(ip, now) {
            return Err(AuthAdmitDeny::RateLimited);
        }
        if self.sessions >= MAX_AUTHENTICATED_SESSIONS {
            return Err(AuthAdmitDeny::CapExceeded);
        }
        if self.in_flight >= MAX_IN_FLIGHT_AUTH {
            return Err(AuthAdmitDeny::CapExceeded);
        }
        self.in_flight += 1;
        Ok(())
    }

    /// Release an in-flight auth slot (success or failure path).
    pub fn end_auth_attempt(&mut self) {
        self.in_flight = self.in_flight.saturating_sub(1);
    }

    /// Record a failed auth for `ip`. Returns how long the caller should sleep
    /// before sending `auth_failed` / closing.
    pub fn record_failure(&mut self, ip: IpAddr, now: Instant) -> Duration {
        let peer = self.peers.entry(ip).or_default();
        peer.failures.push_back(now);
        Self::evict_peer_failures(peer, now);
        let n = peer.failures.len();
        if n >= MAX_FAILS_PER_WINDOW {
            peer.locked_until = Some(now + LOCKOUT);
            peer.failures.clear();
        }
        backoff_for_failures(n)
    }

    /// Clear failure streak after a successful PAM auth.
    pub fn record_success(&mut self, ip: IpAddr) {
        if let Some(peer) = self.peers.get_mut(&ip) {
            peer.failures.clear();
            peer.locked_until = None;
        }
    }

    /// Reserve an authenticated session slot. Call after successful PAM, before
    /// exposing the session. Returns false if the session cap is full.
    pub fn try_begin_session(&mut self) -> bool {
        if self.sessions >= MAX_AUTHENTICATED_SESSIONS {
            return false;
        }
        self.sessions += 1;
        true
    }

    pub fn end_session(&mut self) {
        self.sessions = self.sessions.saturating_sub(1);
    }

    pub fn is_locked_out(&self, ip: IpAddr, now: Instant) -> bool {
        self.peers
            .get(&ip)
            .and_then(|p| p.locked_until)
            .is_some_and(|until| until > now)
    }

    #[cfg(test)]
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    #[cfg(test)]
    pub fn session_count(&self) -> usize {
        self.sessions
    }

    fn evict_stale(&mut self, now: Instant) {
        self.peers.retain(|_, peer| {
            Self::evict_peer_failures(peer, now);
            if peer.locked_until.is_some_and(|u| u <= now) {
                peer.locked_until = None;
            }
            !peer.failures.is_empty() || peer.locked_until.is_some()
        });
    }

    fn evict_peer_failures(peer: &mut PeerState, now: Instant) {
        while let Some(&front) = peer.failures.front() {
            if now.saturating_duration_since(front) >= FAIL_WINDOW {
                peer.failures.pop_front();
            } else {
                break;
            }
        }
    }
}

/// Backoff after `failure_count` failures in the window (1-based after record).
pub fn backoff_for_failures(failure_count: usize) -> Duration {
    if failure_count == 0 {
        return BASE_BACKOFF;
    }
    let exp = (failure_count.saturating_sub(1) as u32).min(MAX_BACKOFF_EXP);
    BASE_BACKOFF * 2u32.pow(exp)
}

/// RAII: decrements in-flight on drop.
pub struct InFlightAuthGuard {
    guard: std::sync::Arc<std::sync::Mutex<AuthGuard>>,
    active: bool,
}

impl InFlightAuthGuard {
    pub fn new(guard: std::sync::Arc<std::sync::Mutex<AuthGuard>>) -> Self {
        Self {
            guard,
            active: true,
        }
    }

    pub fn disarm(mut self) {
        self.active = false;
    }
}

impl Drop for InFlightAuthGuard {
    fn drop(&mut self) {
        if self.active
            && let Ok(mut g) = self.guard.lock()
        {
            g.end_auth_attempt();
        }
    }
}

/// RAII: decrements authenticated session count on drop.
pub struct AuthSessionSlot {
    guard: std::sync::Arc<std::sync::Mutex<AuthGuard>>,
}

impl AuthSessionSlot {
    pub fn new(guard: std::sync::Arc<std::sync::Mutex<AuthGuard>>) -> Self {
        Self { guard }
    }
}

impl Drop for AuthSessionSlot {
    fn drop(&mut self) {
        if let Ok(mut g) = self.guard.lock() {
            g.end_session();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn ip() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50))
    }

    #[test]
    fn lockout_after_max_fails() {
        let mut g = AuthGuard::default();
        let t0 = Instant::now();
        for _ in 0..MAX_FAILS_PER_WINDOW {
            assert!(g.try_begin_auth(ip(), t0).is_ok());
            let backoff = g.record_failure(ip(), t0);
            assert!(backoff >= BASE_BACKOFF);
            g.end_auth_attempt();
        }
        assert!(g.is_locked_out(ip(), t0 + Duration::from_millis(10)));
        assert_eq!(
            g.try_begin_auth(ip(), t0 + Duration::from_millis(10)),
            Err(AuthAdmitDeny::RateLimited)
        );
        assert!(
            g.try_begin_auth(ip(), t0 + LOCKOUT + Duration::from_millis(1))
                .is_ok()
        );
        g.end_auth_attempt();
    }

    #[test]
    fn helpers_report_counts() {
        let mut g = AuthGuard::default();
        let t0 = Instant::now();
        assert_eq!(g.in_flight(), 0);
        assert_eq!(g.session_count(), 0);
        assert!(g.try_begin_auth(ip(), t0).is_ok());
        assert_eq!(g.in_flight(), 1);
        assert!(g.try_begin_session());
        assert_eq!(g.session_count(), 1);
        g.end_auth_attempt();
        g.end_session();
    }

    #[test]
    fn success_clears_streak() {
        let mut g = AuthGuard::default();
        let t0 = Instant::now();
        for _ in 0..3 {
            assert!(g.try_begin_auth(ip(), t0).is_ok());
            let _ = g.record_failure(ip(), t0);
            g.end_auth_attempt();
        }
        assert!(g.try_begin_auth(ip(), t0).is_ok());
        g.record_success(ip());
        g.end_auth_attempt();
        // Four more failures needed for lockout (streak reset).
        for _ in 0..4 {
            assert!(g.try_begin_auth(ip(), t0).is_ok());
            let _ = g.record_failure(ip(), t0);
            g.end_auth_attempt();
        }
        assert!(!g.is_locked_out(ip(), t0));
    }

    #[test]
    fn in_flight_cap() {
        let mut g = AuthGuard::default();
        let t0 = Instant::now();
        for _ in 0..MAX_IN_FLIGHT_AUTH {
            assert!(g.try_begin_auth(ip(), t0).is_ok());
        }
        assert_eq!(
            g.try_begin_auth(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), t0),
            Err(AuthAdmitDeny::CapExceeded)
        );
        g.end_auth_attempt();
        assert!(
            g.try_begin_auth(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), t0)
                .is_ok()
        );
    }

    #[test]
    fn session_cap_blocks_before_pam() {
        let mut g = AuthGuard::default();
        let t0 = Instant::now();
        for _ in 0..MAX_AUTHENTICATED_SESSIONS {
            assert!(g.try_begin_session());
        }
        assert_eq!(g.try_begin_auth(ip(), t0), Err(AuthAdmitDeny::CapExceeded));
        g.end_session();
        assert!(g.try_begin_auth(ip(), t0).is_ok());
    }

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff_for_failures(1), Duration::from_millis(400));
        assert_eq!(backoff_for_failures(2), Duration::from_millis(800));
        assert_eq!(backoff_for_failures(5), Duration::from_millis(6400));
        assert_eq!(backoff_for_failures(99), Duration::from_millis(6400));
    }
}
