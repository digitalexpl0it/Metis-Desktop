//! Locate and drive `polkit-agent-helper-1` for PAM authentication.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use zeroize::Zeroize;

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

#[derive(Debug)]
pub enum HelperOutcome {
    Success,
    Failure { message: Option<String> },
    Error(String),
}

/// Run the setuid helper: `helper <username>`, cookie then password on stdin.
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

    let mut stdin = match child.stdin.take() {
        Some(s) => s,
        None => return HelperOutcome::Error("helper stdin missing".into()),
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return HelperOutcome::Error("helper stdout missing".into()),
    };

    if let Err(e) = stdin.write_all(cookie.as_bytes()).await {
        return HelperOutcome::Error(format!("write cookie: {e}"));
    }
    if let Err(e) = stdin.write_all(b"\n").await {
        return HelperOutcome::Error(format!("write cookie newline: {e}"));
    }

    let mut lines = BufReader::new(stdout).lines();
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
            // Common prompts: "Password:", localized variants — send once.
            if !password_sent {
                if let Err(e) = stdin.write_all(password.as_bytes()).await {
                    return HelperOutcome::Error(format!("write password: {e}"));
                }
                if let Err(e) = stdin.write_all(b"\n").await {
                    return HelperOutcome::Error(format!("write password newline: {e}"));
                }
                let _ = stdin.flush().await;
                password_sent = true;
                tracing::debug!(%prompt, "sent password to helper");
            }
        } else if let Some(info) = line.strip_prefix("PAM_TEXT_INFO") {
            last_info = Some(info.trim().to_string());
        } else if let Some(err) = line.strip_prefix("PAM_ERROR_MSG") {
            last_info = Some(err.trim().to_string());
        } else if line.starts_with("SUCCESS") {
            let _ = child.wait().await;
            return HelperOutcome::Success;
        } else if line.starts_with("FAILURE") {
            let _ = child.wait().await;
            return HelperOutcome::Failure {
                message: last_info.take(),
            };
        }
    }

    match child.wait().await {
        Ok(status) if status.success() && password_sent => HelperOutcome::Success,
        Ok(status) => HelperOutcome::Failure {
            message: Some(format!(
                "authentication failed ({status}){}",
                last_info
                    .map(|m| format!(": {m}"))
                    .unwrap_or_default()
            )),
        },
        Err(e) => HelperOutcome::Error(format!("helper wait: {e}")),
    }
}
