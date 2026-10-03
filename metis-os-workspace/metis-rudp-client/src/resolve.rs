//! Hostname / IP → [`SocketAddr`] for Metis Remote clients.

use std::net::{SocketAddr, ToSocketAddrs};

use crate::tofu::ClientError;

/// Resolve a `host:port` endpoint (IP or hostname) to a [`SocketAddr`].
///
/// Accepts `127.0.0.1:7843`, `[::1]:7843`, and `localhost:7843`.
pub fn resolve_endpoint(endpoint: &str) -> Result<SocketAddr, ClientError> {
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return Err(ClientError::msg("host address is empty"));
    }
    if let Ok(addr) = endpoint.parse::<SocketAddr>() {
        return Ok(addr);
    }
    let (host, port) = split_host_port(endpoint)?;
    resolve_host_port(&host, port)
}

/// Resolve `host` + `port` to a socket address.
///
/// Accepts IPs (`127.0.0.1`, `[::1]`) and hostnames (`localhost`). Prefer the
/// first result from the system resolver.
pub fn resolve_host_port(host: &str, port: u16) -> Result<SocketAddr, ClientError> {
    let host = host.trim();
    if host.is_empty() {
        return Err(ClientError::msg("host is empty"));
    }
    if port == 0 {
        return Err(ClientError::msg("port must be between 1 and 65535"));
    }

    // Bracketed IPv6 without port: `[::1]` → parse as IP then attach port.
    if host.starts_with('[')
        && let Some(end) = host.find(']')
        && end == host.len() - 1
    {
        let inner = &host[1..end];
        if let Ok(ip) = inner.parse::<std::net::IpAddr>() {
            return Ok(SocketAddr::new(ip, port));
        }
    }

    // Bare IP (no DNS).
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Ok(SocketAddr::new(ip, port));
    }

    // Hostname (e.g. localhost) — system resolver via ToSocketAddrs.
    format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|e| ClientError::msg(format!("could not resolve {host}:{port}: {e}")))?
        .next()
        .ok_or_else(|| ClientError::msg(format!("no addresses for {host}:{port}")))
}

fn split_host_port(endpoint: &str) -> Result<(String, u16), ClientError> {
    if let Some(rest) = endpoint.strip_prefix('[') {
        let Some(end) = rest.find(']') else {
            return Err(ClientError::msg("invalid IPv6 host:port (missing ']')"));
        };
        let host = format!("[{}]", &rest[..end]);
        let after = &rest[end + 1..];
        let Some(port_s) = after.strip_prefix(':') else {
            return Err(ClientError::msg("invalid IPv6 host:port (missing port)"));
        };
        let port: u16 = port_s
            .parse()
            .map_err(|_| ClientError::msg(format!("invalid port '{port_s}'")))?;
        return Ok((host, port));
    }
    let Some((host, port_s)) = endpoint.rsplit_once(':') else {
        return Err(ClientError::msg(
            "invalid host address (expected host:port)",
        ));
    };
    if host.is_empty() {
        return Err(ClientError::msg("host is empty"));
    }
    let port: u16 = port_s
        .parse()
        .map_err(|_| ClientError::msg(format!("invalid port '{port_s}'")))?;
    Ok((host.to_string(), port))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn resolves_ipv4_and_localhost() {
        let a = resolve_host_port("127.0.0.1", 7843).expect("ip");
        assert_eq!(a, SocketAddr::from((Ipv4Addr::LOCALHOST, 7843)));
        let b = resolve_host_port("localhost", 7843).expect("localhost");
        assert_eq!(b.port(), 7843);
        assert!(b.ip().is_loopback());
    }

    #[test]
    fn resolves_bracketed_ipv6() {
        let a = resolve_host_port("[::1]", 7843).expect("v6");
        assert_eq!(a, SocketAddr::from((Ipv6Addr::LOCALHOST, 7843)));
    }

    #[test]
    fn rejects_empty_host() {
        assert!(resolve_host_port("  ", 7843).is_err());
        assert!(resolve_host_port("127.0.0.1", 0).is_err());
    }

    #[test]
    fn resolves_endpoint_hostname() {
        let a = resolve_endpoint("localhost:7843").expect("localhost");
        assert_eq!(a.port(), 7843);
        assert!(a.ip().is_loopback());
        let b = resolve_endpoint("127.0.0.1:7843").expect("ip");
        assert_eq!(b, SocketAddr::from((Ipv4Addr::LOCALHOST, 7843)));
    }
}
