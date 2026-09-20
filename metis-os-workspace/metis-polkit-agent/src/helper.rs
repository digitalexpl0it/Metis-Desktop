//! Drive `polkit-agent-helper-1` for PAM authentication.
//!
//! On polkit ≥ 127 the helper is intentionally not setuid. Agents must connect
//! to `/run/polkit/agent-helper.socket` (systemd socket activation). Spawning
//! the binary directly only works when the setuid bit is present (legacy).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::process::Command;
use zeroize::Zeroize;

const HELPER_SOCKET: &str = "/run/polkit/agent-helper.socket";

const HELPER_CANDIDATES: &[&str] = &[
    "/usr/lib/polkit-1/polkit-agent-helper-1",
    "/usr/libexec/polkit-1/polkit-agent-helper-1",
    "/usr/lib/policykit-1/polkit-agent-helper-1",
];

pub fn locate_helper() -> Option<PathBuf> {
    HELPER_CANDIDATES
        .iter()
        .map(Path::new)
        .find(|p| p.is_file())
        .map(Path::to_path_buf)
}

/// Prefer the socket-activated helper; fall back to a setuid binary.
pub fn helper_backend_ready(helper: &Path) -> Result<(), String> {
    if Path::new(HELPER_SOCKET).exists() {
        return Ok(());
    }
    if helper_is_setuid(helper) {
        return Ok(());
    }
    Err(format!(
        "PolicyKit helper is not usable: {HELPER_SOCKET} is missing and {} \
         is not setuid root. Enable polkit-agent-helper.socket, or restore \
         setuid on the helper (chmod 4755).",
        helper.display()
    ))
}

fn helper_is_setuid(helper: &Path) -> bool {
    std::fs::metadata(helper)
        .map(|m| m.permissions().mode() & 0o4000 != 0)
        .unwrap_or(false)
}

#[derive(Debug)]
pub enum HelperOutcome {
    Success,
    Failure { message: Option<String> },
    Error(String),
}

/// Authenticate via socket-activated helper, or legacy setuid spawn.
pub async fn authenticate(
    helper: &Path,
    username: &str,
    cookie: &str,
    mut password: String,
) -> HelperOutcome {
    let result = authenticate_inner(helper, username, cookie, &password).await;
    password.zeroize();
    result
}

async fn authenticate_inner(
    helper: &Path,
    username: &str,
    cookie: &str,
    password: &str,
) -> HelperOutcome {
    match connect_socket_helper().await {
        Ok(stream) => {
            tracing::debug!(path = HELPER_SOCKET, "using socket-activated polkit helper");
            let (reader, writer) = stream.into_split();
            // Socket mode: username then cookie on the same stream.
            run_conversation(reader, writer, Some(username), cookie, password).await
        }
        Err(socket_err) => {
            if !helper_is_setuid(helper) {
                return HelperOutcome::Error(format!(
                    "cannot use PolicyKit helper: socket ({socket_err}); \
                     binary {} is not setuid root (expected on polkit ≥ 127 — \
                     ensure polkit-agent-helper.socket is active)",
                    helper.display()
                ));
            }
            tracing::debug!(
                path = %helper.display(),
                %socket_err,
                "socket unavailable; spawning setuid helper"
            );
            spawn_setuid_helper(helper, username, cookie, password).await
        }
    }
}

async fn connect_socket_helper() -> Result<UnixStream, String> {
    if !Path::new(HELPER_SOCKET).exists() {
        return Err(format!("{HELPER_SOCKET} missing"));
    }
    UnixStream::connect(HELPER_SOCKET)
        .await
        .map_err(|e| format!("connect {HELPER_SOCKET}: {e}"))
}

async fn spawn_setuid_helper(
    helper: &Path,
    username: &str,
    cookie: &str,
    password: &str,
) -> HelperOutcome {
    let mut child = match Command::new(helper)
        .arg(username)
        .env("LC_ALL", "C")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return HelperOutcome::Error(format!("spawn helper: {e}")),
    };

    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => return HelperOutcome::Error("helper stdin missing".into()),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return HelperOutcome::Error("helper stdout missing".into()),
    };

    // Username is argv; only cookie (+ PAM replies) go on stdin.
    let outcome = run_conversation(stdout, stdin, None, cookie, password).await;
    let _ = child.wait().await;
    outcome
}

async fn run_conversation<R, W>(
    reader: R,
    mut writer: W,
    username_on_stdin: Option<&str>,
    cookie: &str,
    password: &str,
) -> HelperOutcome
where
    R: tokio::io::AsyncRead + Unpin,
    W: tokio::io::AsyncWrite + Unpin,
{
    if let Some(username) = username_on_stdin {
        if let Err(e) = writer.write_all(username.as_bytes()).await {
            return HelperOutcome::Error(format!("write username: {e}"));
        }
        if let Err(e) = writer.write_all(b"\n").await {
            return HelperOutcome::Error(format!("write username newline: {e}"));
        }
    }

    if let Err(e) = writer.write_all(cookie.as_bytes()).await {
        return HelperOutcome::Error(format!("write cookie: {e}"));
    }
    if let Err(e) = writer.write_all(b"\n").await {
        return HelperOutcome::Error(format!("write cookie newline: {e}"));
    }
    let _ = writer.flush().await;

    let mut lines = BufReader::new(reader).lines();
    let mut last_info: Option<String> = None;
    let mut password_sent = false;

    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => return HelperOutcome::Error(format!("read helper: {e}")),
        };
        tracing::debug!(%line, "helper stdout");

        if let Some(rest) = line.strip_prefix("PAM_PROMPT_ECHO_OFF") {
            let prompt = rest.trim();
            if !password_sent {
                if let Err(e) = writer.write_all(password.as_bytes()).await {
                    return HelperOutcome::Error(format!("write password: {e}"));
                }
                if let Err(e) = writer.write_all(b"\n").await {
                    return HelperOutcome::Error(format!("write password newline: {e}"));
                }
                let _ = writer.flush().await;
                password_sent = true;
                tracing::debug!(%prompt, "sent password to helper");
            }
        } else if let Some(info) = line.strip_prefix("PAM_TEXT_INFO") {
            last_info = Some(info.trim().to_string());
        } else if let Some(err) = line.strip_prefix("PAM_ERROR_MSG") {
            last_info = Some(err.trim().to_string());
        } else if line.starts_with("SUCCESS") {
            return HelperOutcome::Success;
        } else if line.starts_with("FAILURE") {
            return HelperOutcome::Failure {
                message: last_info.take(),
            };
        }
    }

    HelperOutcome::Failure {
        message: Some(last_info.unwrap_or_else(|| {
            if password_sent {
                "authentication failed".into()
            } else {
                "helper closed before password prompt".into()
            }
        })),
    }
}
