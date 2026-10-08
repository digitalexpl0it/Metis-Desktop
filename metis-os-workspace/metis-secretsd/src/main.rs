//! Metis Secret Service daemon — owns `org.freedesktop.secrets` when no other
//! provider is already on the session bus.

mod dbus_api;
mod vault;

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;
use zbus::Connection;

use crate::dbus_api::{
    AppState, COLLECTION_PATH, CollectionIface, SERVICE_PATH, ServiceIface, register_existing_items,
};
use crate::vault::Vault;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("metis_secretsd=info,warn")),
        )
        .init();

    if libc_geteuid() == 0 {
        eprintln!("metis-secretsd: refuse to run as root");
        return ExitCode::from(1);
    }

    let vault = match Vault::open_default() {
        Ok(v) => v,
        Err(err) => {
            tracing::error!(%err, "failed to open vault");
            return ExitCode::from(1);
        }
    };

    let connection = match Connection::session().await {
        Ok(c) => c,
        Err(err) => {
            tracing::error!(%err, "session bus connect failed");
            return ExitCode::from(1);
        }
    };

    match connection.request_name("org.freedesktop.secrets").await {
        Ok(_) => {}
        Err(err) => {
            tracing::error!(
                %err,
                "could not claim org.freedesktop.secrets — another provider is running"
            );
            return ExitCode::from(1);
        }
    }

    let state = AppState::new(vault, connection.clone());

    if let Err(err) = connection
        .object_server()
        .at(
            SERVICE_PATH,
            ServiceIface {
                state: state.clone(),
            },
        )
        .await
    {
        tracing::error!(%err, "serve Service failed");
        return ExitCode::from(1);
    }

    if let Err(err) = connection
        .object_server()
        .at(
            COLLECTION_PATH,
            CollectionIface {
                state: state.clone(),
            },
        )
        .await
    {
        tracing::error!(%err, "serve Collection failed");
        return ExitCode::from(1);
    }

    if let Err(err) = register_existing_items(&state).await {
        tracing::error!(%err, "register items failed");
        return ExitCode::from(1);
    }

    tracing::info!("metis-secretsd listening on org.freedesktop.secrets");

    // Stay alive until SIGTERM/SIGINT.
    tokio::signal::ctrl_c().await.ok();
    tracing::info!("shutting down");
    ExitCode::SUCCESS
}

fn libc_geteuid() -> u32 {
    // Avoid a libc crate dep for a single call.
    #[link(name = "c")]
    unsafe extern "C" {
        fn geteuid() -> u32;
    }
    unsafe { geteuid() }
}
