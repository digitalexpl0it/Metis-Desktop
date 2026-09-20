//! Metis PolicyKit authentication agent — GTK4 dialogs for `pkexec` / polkitd.

mod agent;
mod authority;
mod helper;
mod session;
mod theme;
mod ui;

use std::process::ExitCode;
use std::time::Duration;

use gtk::prelude::*;

use crate::agent::{AgentEvent, UiResponse};
use crate::authority::{AuthorityProxy, Subject};
use crate::session::resolve_session_id;

const OBJECT_PATH: &str = "/org/metis/PolicyKit1/AuthenticationAgent";
const APP_ID: &str = "org.metis.PolkitAgent";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "metis_polkit_agent=info,warn".into()),
        )
        .init();

    if unsafe { libc::geteuid() } == 0 {
        eprintln!("metis-polkit-agent: refuse to run as root");
        return ExitCode::from(1);
    }

    // Avoid AT-SPI round-trips that stall password entry on Wayland sessions.
    // Also prefer Cairo so GSK/GL init cannot hitch the dialog.
    unsafe {
        if std::env::var_os("GTK_A11Y").is_none() {
            std::env::set_var("GTK_A11Y", "none");
        }
        if std::env::var_os("NO_AT_BRIDGE").is_none() {
            std::env::set_var("NO_AT_BRIDGE", "1");
        }
        if std::env::var_os("GSK_RENDERER").is_none() {
            std::env::set_var("GSK_RENDERER", "cairo");
        }
    }

    if let Err(err) = gtk::init() {
        eprintln!("metis-polkit-agent: gtk init failed: {err}");
        return ExitCode::from(1);
    }
    theme::install();

    let Some(helper) = helper::locate_helper() else {
        eprintln!(
            "metis-polkit-agent: polkit-agent-helper-1 not found \
             (install polkitd / policykit-1)"
        );
        return ExitCode::from(1);
    };
    if let Err(err) = helper::helper_backend_ready(&helper) {
        eprintln!("metis-polkit-agent: {err}");
        return ExitCode::from(1);
    }
    tracing::info!(path = %helper.display(), "using polkit agent helper");
    let session_id = match resolve_session_id() {
        Ok(id) => id,
        Err(err) => {
            eprintln!("metis-polkit-agent: {err}");
            return ExitCode::from(1);
        }
    };
    tracing::info!(%session_id, "registering for unix-session");

    let (to_ui_tx, to_ui_rx) = async_channel::unbounded::<AgentEvent>();
    let (from_ui_tx, from_ui_rx) = async_channel::unbounded::<UiResponse>();

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("metis-polkit-dbus")
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("metis-polkit-agent: tokio runtime: {err}");
            return ExitCode::from(1);
        }
    };

    // Register on the system bus *before* entering the GTK loop so a failure
    // exits cleanly (and the compositor watchdog can retry after clearing
    // third-party agents).
    let connection = match rt.block_on(register_agent(
        helper.clone(),
        session_id.clone(),
        to_ui_tx,
        from_ui_rx,
    )) {
        Ok(conn) => conn,
        Err(err) => {
            eprintln!("metis-polkit-agent: {err}");
            return ExitCode::from(1);
        }
    };

    // Keep the connection alive on the tokio runtime for the process lifetime.
    rt.spawn(async move {
        let _conn = connection;
        std::future::pending::<()>().await;
    });

    let app = gtk::Application::builder().application_id(APP_ID).build();
    let _hold = app.hold();
    app.connect_activate(|_| {});

    let app_weak = app.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(event) = to_ui_rx.recv().await {
            let Some(app) = app_weak.upgrade() else {
                break;
            };
            match event {
                AgentEvent::Begin {
                    cookie,
                    message,
                    action_id,
                    usernames,
                } => {
                    ui::present_auth_dialog(
                        &app,
                        cookie,
                        message,
                        action_id,
                        usernames,
                        from_ui_tx.clone(),
                    );
                }
                AgentEvent::Cancel { cookie } => {
                    let _ = from_ui_tx
                        .send(UiResponse::Cancel {
                            cookie: cookie.clone(),
                        })
                        .await;
                    ui::cancel_dialog(&cookie);
                }
                AgentEvent::Retry {
                    cookie,
                    retry_message,
                } => {
                    ui::show_retry(&cookie, retry_message);
                }
                AgentEvent::Succeeded { cookie } => {
                    ui::close_dialog(&cookie);
                }
            }
        }
    });

    let code = app.run();
    if code == glib::ExitCode::SUCCESS {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

async fn register_agent(
    helper: std::path::PathBuf,
    session_id: String,
    to_ui: async_channel::Sender<AgentEvent>,
    from_ui: async_channel::Receiver<UiResponse>,
) -> Result<zbus::Connection, String> {
    let agent = agent::AuthenticationAgent::new(helper, to_ui, from_ui);
    let connection = zbus::connection::Builder::system()
        .map_err(|e| format!("system bus: {e}"))?
        .serve_at(OBJECT_PATH, agent)
        .map_err(|e| format!("serve agent: {e}"))?
        .build()
        .await
        .map_err(|e| format!("connect system bus: {e}"))?;

    let locale = glib::language_names()
        .first()
        .map(|s| s.as_str().to_string())
        .unwrap_or_else(|| "C".into());

    let mut details = std::collections::HashMap::new();
    details.insert(
        "session-id".to_string(),
        zbus::zvariant::OwnedValue::try_from(zbus::zvariant::Value::new(session_id.clone()))
            .map_err(|e| format!("session-id value: {e}"))?,
    );
    let subject = Subject {
        kind: "unix-session".into(),
        details,
    };

    let proxy = AuthorityProxy::new(&connection)
        .await
        .map_err(|e| format!("Authority proxy: {e}"))?;

    match proxy
        .register_authentication_agent(&subject, &locale, OBJECT_PATH)
        .await
    {
        Ok(()) => {
            tracing::info!("registered as Metis PolicyKit authentication agent");
            Ok(connection)
        }
        Err(err) => {
            let msg = err.to_string();
            if msg.contains("already exists") {
                // Brief wait in case a dying third-party agent is unregistering.
                tokio::time::sleep(Duration::from_millis(500)).await;
                proxy
                    .register_authentication_agent(&subject, &locale, OBJECT_PATH)
                    .await
                    .map_err(|e| {
                        format!(
                            "RegisterAuthenticationAgent (retry): {e} — stop other \
                             PolicyKit agents in this session and retry"
                        )
                    })?;
                tracing::info!("registered as Metis PolicyKit authentication agent (retry)");
                Ok(connection)
            } else {
                Err(format!("RegisterAuthenticationAgent: {msg}"))
            }
        }
    }
}
