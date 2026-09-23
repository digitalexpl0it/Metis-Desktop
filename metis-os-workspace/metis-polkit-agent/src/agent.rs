//! org.freedesktop.PolicyKit1.AuthenticationAgent implementation.

use std::collections::HashMap;
use std::path::PathBuf;

use zbus::interface;
use zbus::zvariant::OwnedValue;
use zeroize::Zeroize;

use crate::authority::{Identity, PolkitError};
use crate::helper::{self, HelperOutcome};

/// Events pushed to the GTK thread.
pub enum AgentEvent {
    Begin {
        cookie: String,
        message: String,
        action_id: String,
        usernames: Vec<String>,
    },
    Cancel {
        cookie: String,
    },
    Retry {
        cookie: String,
        retry_message: Option<String>,
    },
    Succeeded {
        cookie: String,
    },
}

/// User actions from the GTK dialog.
pub enum UiResponse {
    Authenticate {
        cookie: String,
        username: String,
        password: String,
    },
    Cancel {
        cookie: String,
    },
}

pub struct AuthenticationAgent {
    helper: PathBuf,
    to_ui: async_channel::Sender<AgentEvent>,
    from_ui: async_channel::Receiver<UiResponse>,
}

impl AuthenticationAgent {
    pub fn new(
        helper: PathBuf,
        to_ui: async_channel::Sender<AgentEvent>,
        from_ui: async_channel::Receiver<UiResponse>,
    ) -> Self {
        Self {
            helper,
            to_ui,
            from_ui,
        }
    }
}

#[interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl AuthenticationAgent {
    async fn cancel_authentication(&self, cookie: &str) {
        tracing::debug!(%cookie, "CancelAuthentication");
        let _ = self
            .to_ui
            .send(AgentEvent::Cancel {
                cookie: cookie.to_string(),
            })
            .await;
    }

    async fn begin_authentication(
        &mut self,
        action_id: &str,
        message: &str,
        icon_name: &str,
        details: HashMap<String, String>,
        cookie: &str,
        identities: Vec<Identity>,
    ) -> Result<(), PolkitError> {
        let _ = (icon_name, &details);
        tracing::info!(%action_id, %cookie, %message, "BeginAuthentication");

        let usernames = identities_to_usernames(&identities);
        if usernames.is_empty() {
            return Err(PolkitError::Failed(
                "no unix-user identities available for authentication".into(),
            ));
        }

        self.to_ui
            .send(AgentEvent::Begin {
                cookie: cookie.to_string(),
                message: message.to_string(),
                action_id: action_id.to_string(),
                usernames,
            })
            .await
            .map_err(|_| PolkitError::Failed("UI channel closed".into()))?;

        loop {
            let response = self
                .from_ui
                .recv()
                .await
                .map_err(|_| PolkitError::Failed("UI reply channel closed".into()))?;

            match response {
                UiResponse::Cancel { cookie: c } if c == cookie => {
                    return Err(PolkitError::Cancelled(
                        "user cancelled authentication".into(),
                    ));
                }
                UiResponse::Cancel { .. } => continue,
                UiResponse::Authenticate {
                    cookie: c,
                    username,
                    mut password,
                } if c == cookie => {
                    let outcome =
                        helper::authenticate(&self.helper, &username, cookie, password.clone())
                            .await;
                    password.zeroize();
                    match outcome {
                        HelperOutcome::Success => {
                            let _ = self
                                .to_ui
                                .send(AgentEvent::Succeeded {
                                    cookie: cookie.to_string(),
                                })
                                .await;
                            return Ok(());
                        }
                        HelperOutcome::Failure { message } => {
                            let _ = self
                                .to_ui
                                .send(AgentEvent::Retry {
                                    cookie: cookie.to_string(),
                                    retry_message: message.or_else(|| {
                                        Some("Authentication failed. Please try again.".into())
                                    }),
                                })
                                .await;
                            continue;
                        }
                        HelperOutcome::Error(err) => {
                            return Err(PolkitError::Failed(err));
                        }
                    }
                }
                UiResponse::Authenticate { .. } => continue,
            }
        }
    }
}

fn identities_to_usernames(identities: &[Identity]) -> Vec<String> {
    let mut names = Vec::new();
    for identity in identities {
        if identity.kind != "unix-user" {
            continue;
        }
        let Some(uid_val) = identity.details.get("uid") else {
            continue;
        };
        let Some(uid) = owned_to_u32(uid_val) else {
            continue;
        };
        if let Some(name) = username_from_uid(uid) {
            names.push(name);
        }
    }
    if let Ok(me) = std::env::var("USER")
        && let Some(pos) = names.iter().position(|n| n == &me)
    {
        let mine = names.remove(pos);
        names.insert(0, mine);
    }
    names
}

fn owned_to_u32(val: &OwnedValue) -> Option<u32> {
    if let Ok(u) = u32::try_from(val) {
        return Some(u);
    }
    if let Ok(u) = u64::try_from(val) {
        return Some(u as u32);
    }
    None
}

fn username_from_uid(uid: u32) -> Option<String> {
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return None;
        }
        let cstr = std::ffi::CStr::from_ptr((*pw).pw_name);
        cstr.to_str().ok().map(str::to_string)
    }
}
