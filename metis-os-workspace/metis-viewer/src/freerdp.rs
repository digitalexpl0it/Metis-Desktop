//! Resolve and spawn FreeRDP without a shell.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use metis_config::ViewerRdpOptions;
use zeroize::Zeroize;

/// Preferred FreeRDP clients under `/usr/bin` only (no `PATH`, no `sh -c`).
const CANDIDATES: &[&str] = &["wlfreerdp3", "wlfreerdp", "xfreerdp3", "xfreerdp"];

/// How long to watch for immediate FreeRDP exit (auth/connect failures).
pub const EARLY_FAILURE_WINDOW: Duration = Duration::from_secs(2);

pub fn freerdp_install_hint() -> String {
    "sudo apt install freerdp3-wayland".into()
}

pub fn freerdp_install_hint_full() -> String {
    "FreeRDP is not installed. On Ubuntu: sudo apt install freerdp3-wayland \
     (or freerdp2-x11)."
        .into()
}

/// Absolute path to the first available FreeRDP binary, if any.
pub fn resolve_freerdp() -> Option<PathBuf> {
    for name in CANDIDATES {
        let path = Path::new("/usr/bin").join(name);
        if is_executable(&path) {
            return Some(path);
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match path.metadata() {
        Ok(meta) if meta.is_file() => meta.permissions().mode() & 0o111 != 0,
        _ => false,
    }
}

#[derive(Debug, Clone)]
pub struct ConnectRequest {
    pub host: String,
    pub port: u16,
    pub username: String,
    /// When set and non-empty, passed as `/p:` on the child argv only (never logged).
    /// When empty/`None`, `/p:` is omitted so FreeRDP can prompt (GUI dialog).
    ///
    /// Note: argv passwords are briefly visible in `/proc/<pid>/cmdline`.
    pub password: Option<String>,
    pub options: ViewerRdpOptions,
}

pub struct SpawnedFreerdp {
    pub binary: PathBuf,
    pub child: Child,
}

/// Spawn FreeRDP with argv-only arguments. Caller should poll [`poll_early_failure`].
pub fn spawn_freerdp(mut req: ConnectRequest) -> Result<SpawnedFreerdp, String> {
    let bin = resolve_freerdp().ok_or_else(freerdp_install_hint_full)?;
    let host = req.host.trim();
    if host.is_empty() {
        return Err("Host is required.".into());
    }
    if req.port == 0 {
        return Err("Port must be between 1 and 65535.".into());
    }

    let mut args: Vec<String> = vec![
        format!("/v:{host}:{}", req.port),
        format!("/u:{}", req.username.trim()),
    ];
    args.extend(req.options.freerdp_args());

    if let Some(pw) = req.password.as_mut() {
        if !pw.is_empty() {
            args.push(format!("/p:{pw}"));
        }
        pw.zeroize();
    }

    tracing::info!(
        binary = %bin.display(),
        host = %host,
        port = req.port,
        user = %req.username.trim(),
        placement = ?req.options.placement,
        "spawning FreeRDP"
    );

    // GUI FreeRDP clients use their own password dialog when `/p:` is omitted.
    // Pipe stderr so early failures can be surfaced; leave the process detached
    // from our stdin/stdout.
    let child = Command::new(&bin)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", bin.display()))?;

    for a in &mut args {
        if a.starts_with("/p:") {
            a.zeroize();
        }
    }

    Ok(SpawnedFreerdp { binary: bin, child })
}

#[derive(Debug)]
pub enum EarlyWatch {
    /// Still running inside the early-failure window — keep polling.
    Running,
    /// Exited cleanly, or still alive after the window (fire-and-forget).
    Done,
    /// Quick non-zero exit — surface to the user.
    Failed(String),
}

/// Poll for a quick non-zero exit during [`EARLY_FAILURE_WINDOW`].
pub fn poll_early_failure(child: &mut Child, started: Instant) -> EarlyWatch {
    match child.try_wait() {
        Ok(Some(status)) => {
            if status.success() {
                EarlyWatch::Done
            } else {
                let detail = read_child_stderr(child);
                EarlyWatch::Failed(format_failure(status.code(), detail.as_deref()))
            }
        }
        Ok(None) => {
            if started.elapsed() >= EARLY_FAILURE_WINDOW {
                let _ = child.stderr.take();
                EarlyWatch::Done
            } else {
                EarlyWatch::Running
            }
        }
        Err(e) => EarlyWatch::Failed(format!("FreeRDP status check failed: {e}")),
    }
}

fn read_child_stderr(child: &mut Child) -> Option<String> {
    let mut stderr = child.stderr.take()?;
    let mut buf = String::new();
    let _ = stderr.read_to_string(&mut buf);
    useful_stderr_detail(&buf)
}

/// Pick a user-facing line from FreeRDP logs; skip deprecation / WARN noise.
fn useful_stderr_detail(stderr: &str) -> Option<String> {
    let mut fallback: Option<String> = None;
    for line in stderr.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // wlfreerdp3 always prints these; they are not the failure reason.
        if lower.contains("deprecated")
            || lower.contains("freerdp_warn_deprecated")
            || lower.contains("warn_contact_details")
            || lower.contains("as replacement there is a sdl")
            || lower.contains("if you are interested in keeping")
            || lower.contains("be prepared to fix issues")
            || lower.contains("known issues")
        {
            continue;
        }
        if lower.contains("[warn]") || lower.contains("[info]") || lower.contains("[debug]") {
            continue;
        }

        // Prefer the full "Failed at index N [/bad-arg]: Unexpected keyword"
        // line — stripping only the trailing "Unexpected keyword" hides which
        // FreeRDP flag our build rejected.
        if lower.contains("unexpected keyword") || lower.contains("commandlineparsearguments") {
            return Some(truncate_msg(trimmed, 160));
        }

        let msg = strip_freerdp_prefix(trimmed);
        if msg.is_empty() {
            continue;
        }
        let msg_lower = msg.to_ascii_lowercase();
        if msg_lower.contains("[error]")
            || msg_lower.contains("error")
            || msg_lower.contains("failed")
            || msg_lower.contains("authentication")
            || msg_lower.contains("logon")
            || msg_lower.contains("refused")
            || msg_lower.contains("timeout")
            || msg_lower.contains("unreachable")
            || msg_lower.contains("unable")
        {
            return Some(truncate_msg(&msg, 120));
        }
        if fallback.is_none() {
            fallback = Some(truncate_msg(&msg, 120));
        }
    }
    fallback
}

/// Strip `[ts] [pid] [LEVEL][module] - [func]: ` style FreeRDP prefixes when present.
fn strip_freerdp_prefix(line: &str) -> String {
    if let Some(idx) = line.rfind("]: ") {
        let rest = line[idx + 3..].trim();
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    if let Some(idx) = line.find(" - ") {
        let rest = line[idx + 3..].trim();
        if !rest.is_empty() {
            return rest.to_string();
        }
    }
    line.to_string()
}

fn truncate_msg(s: &str, max: usize) -> String {
    if s.len() > max {
        format!("{}…", &s[..max.saturating_sub(1)])
    } else {
        s.to_string()
    }
}

fn format_failure(_code: Option<i32>, detail: Option<&str>) -> String {
    if let Some(d) = detail {
        let lower = d.to_ascii_lowercase();
        if lower.contains("auth") || lower.contains("logon") || lower.contains("password") {
            return "Authentication failed. Check username and password.".into();
        }
        if lower.contains("bio_new")
            || lower.contains("certificate")
            || lower.contains("host key")
            || lower.contains("x509")
        {
            return "TLS certificate problem. If this persists, remove the stale entry under \
                    ~/.config/freerdp/server/ and try again."
                .into();
        }
        if lower.contains("cliprdr") || lower.contains("clipboard") {
            return "Connection failed during clipboard setup. Update Metis Viewer, \
                    or connect from another machine (not from inside the same shared session)."
                .into();
        }
        if lower.contains("refused")
            || lower.contains("unreachable")
            || lower.contains("timed out")
            || lower.contains("timeout")
            || lower.contains("name or service not known")
            || lower.contains("no route")
            || lower.contains("failed to connect")
        {
            return "Could not reach the host. Check address, port, and that sharing is on.".into();
        }
        if lower.contains("unexpected keyword") {
            return format!(
                "FreeRDP rejected a connection option ({d}). Update Metis Viewer, \
                 or clear Advanced Desktop Settings toggles and try again."
            );
        }
        // Keep a short FreeRDP detail when it is actually useful (not log spam).
        if d.len() <= 160 && !lower.contains("[com.freerdp") {
            return format!("Connection failed: {d}");
        }
    }
    "Connection failed. Check host, credentials, and that sharing is enabled.".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_wlfreerdp_deprecation_noise() {
        let stderr = "\
[16:42:57:414] [587225:0008f5d9] [WARN][com.freerdp.client.common.cmdline] - [freerdp_warn_deprecated]: [deprecated] /usr/bin/wlfreerdp3 client has been deprecated
[16:42:57:414] [587225:0008f5d9] [WARN][com.freerdp.client.common.cmdline] - [freerdp_warn_deprecated]: As replacement there is a SDL3 based client available.
";
        assert!(useful_stderr_detail(stderr).is_none());
        let msg = format_failure(Some(255), useful_stderr_detail(stderr).as_deref());
        assert_eq!(
            msg,
            "Connection failed. Check host, credentials, and that sharing is enabled."
        );
    }

    #[test]
    fn prefers_real_error_line() {
        let stderr = "\
[16:42:57:414] [1:1] [WARN][com.freerdp.client.common.cmdline] - [freerdp_warn_deprecated]: [deprecated] wlfreerdp3
[16:42:58:000] [1:1] [ERROR][com.freerdp.core] - [freerdp_tcp_connect]: failed to connect to 192.168.0.11
";
        let detail = useful_stderr_detail(stderr).expect("detail");
        assert!(detail.to_ascii_lowercase().contains("failed to connect"));
        let msg = format_failure(Some(255), Some(&detail));
        assert!(msg.contains("Could not reach") || msg.contains("Connection failed"));
    }

    #[test]
    fn surfaces_unexpected_keyword_line() {
        let stderr = "\
[20:29:29:909] [1:1] [ERROR][com.winpr.commandline] - [CommandLineParseArgumentsA]: Failed at index 3 [+bitmap-cache]: Unexpected keyword
";
        let detail = useful_stderr_detail(stderr).expect("detail");
        assert!(detail.contains("Unexpected keyword"));
        assert!(detail.contains("bitmap-cache") || detail.contains("Failed at index"));
        let msg = format_failure(Some(255), Some(&detail));
        assert!(msg.contains("FreeRDP rejected") || msg.contains("Unexpected keyword"));
    }
}
