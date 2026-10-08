//! Optional smoke: spawn metis-secretsd and round-trip via `metis-secrets` / oo7.
//!
//! Requires a session bus. Skipped unless `METIS_SECRETS_SMOKE=1`.

#![cfg(test)]

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn target_bin(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop(); // metis-os-workspace
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    if let Ok(dir) = std::env::var("CARGO_TARGET_DIR") {
        return PathBuf::from(dir).join(profile).join(name);
    }
    path.join("target").join(profile).join(name)
}

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn oo7_store_get_delete_against_metis_secretsd() {
    if std::env::var_os("METIS_SECRETS_SMOKE").is_none() {
        eprintln!("skip: set METIS_SECRETS_SMOKE=1 to run");
        return;
    }
    let bin = target_bin("metis-secretsd");
    assert!(
        bin.is_file(),
        "missing {bin:?} — build metis-secretsd first"
    );

    let dir = std::env::temp_dir().join(format!(
        "metis-secretsd-smoke-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");

    // If something else owns the bus, this smoke cannot run.
    let child = Command::new(&bin)
        .env("METIS_SECRETS_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn secretsd");
    let _daemon = Daemon(child);
    tokio::time::sleep(Duration::from_millis(300)).await;

    let account = "smoke-test-account";
    let kind = metis_secrets::VIEWER_PASSWORD;
    metis_secrets::store(account, kind, "hunter2")
        .await
        .expect("store");
    let got = metis_secrets::get(account, kind).await.expect("get");
    assert_eq!(got.as_deref(), Some("hunter2"));
    metis_secrets::delete(account, kind).await.expect("delete");
    let gone = metis_secrets::get(account, kind).await.expect("get2");
    assert!(gone.is_none());

    let _ = std::fs::remove_dir_all(&dir);
}
