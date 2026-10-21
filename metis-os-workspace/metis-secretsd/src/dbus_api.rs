//! Minimal `org.freedesktop.Secret.*` server surface for oo7 / libsecret.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::{Mutex, Notify};
use zbus::fdo;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, interface};

use crate::vault::Vault;

pub const SERVICE_PATH: &str = "/org/freedesktop/secrets";
pub const COLLECTION_PATH: &str = "/org/freedesktop/secrets/collection/login";
const PROMPT_NONE: &str = "/";
const ALG_PLAIN: &str = "plain";
const ALG_DH: &str = "dh-ietf1024-sha256-aes128-cbc-pkcs7";

pub type SharedVault = Arc<Mutex<Vault>>;

#[derive(Clone)]
pub struct AppState {
    pub vault: SharedVault,
    pub connection: Connection,
    sessions: Arc<Mutex<HashMap<String, ()>>>,
    next_session: Arc<Mutex<u64>>,
    next_item: Arc<Mutex<u64>>,
    next_prompt: Arc<AtomicU64>,
    /// Wake waiters in `Prompt.Prompt` after unlock / dismiss / rewrap.
    pub unlock_notify: Arc<Notify>,
}

impl AppState {
    pub fn new(vault: Vault, connection: Connection) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            connection,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_session: Arc::new(Mutex::new(1)),
            next_item: Arc::new(Mutex::new(1)),
            next_prompt: Arc::new(AtomicU64::new(1)),
            unlock_notify: Arc::new(Notify::new()),
        }
    }

    fn alloc_prompt_id(&self) -> u64 {
        self.next_prompt.fetch_add(1, Ordering::Relaxed)
    }
}

fn prompt_path(id: u64) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/org/freedesktop/secrets/prompt/{id}"))
        .expect("valid prompt path")
}

fn empty_path() -> OwnedObjectPath {
    OwnedObjectPath::try_from(PROMPT_NONE).expect("valid /")
}

fn collection_path() -> OwnedObjectPath {
    OwnedObjectPath::try_from(COLLECTION_PATH).expect("valid collection path")
}

fn item_path(id: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("{COLLECTION_PATH}/{id}")).expect("valid item path")
}

fn session_path(id: &str) -> OwnedObjectPath {
    OwnedObjectPath::try_from(format!("/org/freedesktop/secrets/session/{id}"))
        .expect("valid session path")
}

fn label_from_props(props: &HashMap<String, Value<'_>>) -> String {
    const ITEM_LABEL: &str = "org.freedesktop.Secret.Item.Label";
    const COLL_LABEL: &str = "org.freedesktop.Secret.Collection.Label";
    for key in [ITEM_LABEL, COLL_LABEL] {
        if let Some(v) = props.get(key)
            && let Ok(s) = <&str>::try_from(v)
        {
            return s.to_string();
        }
    }
    String::new()
}

fn attrs_from_props(props: &HashMap<String, Value<'_>>) -> HashMap<String, String> {
    const ITEM_ATTRS: &str = "org.freedesktop.Secret.Item.Attributes";
    let Some(v) = props.get(ITEM_ATTRS) else {
        return HashMap::new();
    };
    HashMap::<String, String>::try_from(v.clone()).unwrap_or_default()
}

/// Wire secret: (session, parameters, value, content_type).
#[derive(Debug, zbus::zvariant::Type, serde::Serialize, serde::Deserialize)]
#[zvariant(signature = "(oayays)")]
pub struct WireSecret {
    pub session: OwnedObjectPath,
    #[serde(with = "serde_bytes")]
    pub parameters: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub value: Vec<u8>,
    pub content_type: String,
}

pub struct ServiceIface {
    pub state: AppState,
}

pub struct CollectionIface {
    pub state: AppState,
}

pub struct ItemIface {
    pub state: AppState,
    pub id: String,
}

pub struct SessionIface {
    pub state: AppState,
    pub id: String,
}

#[interface(name = "org.freedesktop.Secret.Service")]
impl ServiceIface {
    #[zbus(property)]
    async fn collections(&self) -> Vec<OwnedObjectPath> {
        vec![collection_path()]
    }

    async fn open_session(
        &self,
        algorithm: &str,
        _input: Value<'_>,
    ) -> fdo::Result<(OwnedValue, OwnedObjectPath)> {
        if algorithm == ALG_DH {
            return Err(fdo::Error::NotSupported(
                "metis-secretsd MVP: use plain sessions (oo7 falls back)".into(),
            ));
        }
        if algorithm != ALG_PLAIN {
            return Err(fdo::Error::NotSupported(format!(
                "unsupported algorithm {algorithm}"
            )));
        }
        let mut n = self.state.next_session.lock().await;
        let id = format!("{}", *n);
        *n += 1;
        drop(n);
        self.state.sessions.lock().await.insert(id.clone(), ());
        let path = session_path(&id);
        let iface = SessionIface {
            state: self.state.clone(),
            id: id.clone(),
        };
        self.state
            .connection
            .object_server()
            .at(path.clone(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let output = OwnedValue::try_from(Value::from(""))
            .map_err(|e| fdo::Error::Failed(format!("session output value: {e}")))?;
        Ok((output, path))
    }

    async fn create_collection(
        &self,
        properties: HashMap<String, Value<'_>>,
        alias: &str,
    ) -> fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        let _label = label_from_props(&properties);
        // Single default collection; aliases map via ReadAlias.
        let _ = alias;
        Ok((collection_path(), empty_path()))
    }

    async fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> fdo::Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>)> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Ok((Vec::new(), Vec::new()));
        }
        let unlocked = vault
            .search(&attributes)
            .into_iter()
            .map(|i| item_path(&i.id))
            .collect();
        Ok((unlocked, Vec::new()))
    }

    async fn unlock(
        &self,
        objects: Vec<OwnedObjectPath>,
    ) -> fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        let vault = self.state.vault.lock().await;
        if !vault.is_locked() {
            return Ok((objects, empty_path()));
        }
        drop(vault);
        let id = self.state.alloc_prompt_id();
        let path = prompt_path(id);
        let iface = PromptIface {
            state: self.state.clone(),
            id,
            pending_objects: objects,
        };
        self.state
            .connection
            .object_server()
            .at(path.clone(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        Ok((Vec::new(), path))
    }

    async fn lock(
        &self,
        _objects: Vec<OwnedObjectPath>,
    ) -> fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        self.state.vault.lock().await.lock_vault();
        self.state.unlock_notify.notify_waiters();
        Ok((Vec::new(), empty_path()))
    }

    async fn get_secrets(
        &self,
        items: Vec<OwnedObjectPath>,
        session: OwnedObjectPath,
    ) -> fdo::Result<HashMap<OwnedObjectPath, WireSecret>> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Err(fdo::Error::AccessDenied("vault is locked".into()));
        }
        let mut out = HashMap::new();
        for path in items {
            let id = path
                .as_str()
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .to_string();
            if let Some(item) = vault.get(&id) {
                out.insert(
                    path,
                    WireSecret {
                        session: session.clone(),
                        parameters: Vec::new(),
                        value: item.secret.as_bytes().to_vec(),
                        content_type: item.content_type.clone(),
                    },
                );
            }
        }
        Ok(out)
    }

    async fn read_alias(&self, name: &str) -> fdo::Result<OwnedObjectPath> {
        match name {
            "default" | "login" => Ok(collection_path()),
            _ => Ok(empty_path()),
        }
    }

    async fn set_alias(&self, _name: &str, _collection: OwnedObjectPath) -> fdo::Result<()> {
        Ok(())
    }
}

#[interface(name = "org.freedesktop.Secret.Collection")]
impl CollectionIface {
    #[zbus(property)]
    async fn items(&self) -> Vec<OwnedObjectPath> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Vec::new();
        }
        vault.items().map(|i| item_path(&i.id)).collect()
    }

    #[zbus(property)]
    async fn label(&self) -> String {
        "Default".into()
    }

    #[zbus(property)]
    async fn set_label(&self, _label: String) -> zbus::fdo::Result<()> {
        Ok(())
    }

    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.state.vault.lock().await.is_locked()
    }

    #[zbus(property)]
    async fn created(&self) -> u64 {
        self.state.vault.lock().await.collection_created()
    }

    #[zbus(property)]
    async fn modified(&self) -> u64 {
        self.state.vault.lock().await.collection_modified()
    }

    async fn delete(&self) -> fdo::Result<OwnedObjectPath> {
        Err(fdo::Error::NotSupported(
            "cannot delete the default Metis collection".into(),
        ))
    }

    async fn search_items(
        &self,
        attributes: HashMap<String, String>,
    ) -> fdo::Result<Vec<OwnedObjectPath>> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Err(fdo::Error::AccessDenied("vault is locked".into()));
        }
        Ok(vault
            .search(&attributes)
            .into_iter()
            .map(|i| item_path(&i.id))
            .collect())
    }

    async fn create_item(
        &mut self,
        properties: HashMap<String, Value<'_>>,
        secret: WireSecret,
        replace: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<(OwnedObjectPath, OwnedObjectPath)> {
        if self.state.vault.lock().await.is_locked() {
            return Err(fdo::Error::AccessDenied("vault is locked".into()));
        }
        let label = label_from_props(&properties);
        let attributes = attrs_from_props(&properties);
        let value = String::from_utf8_lossy(&secret.value).into_owned();
        let content_type = if secret.content_type.is_empty() {
            "text/plain".into()
        } else {
            secret.content_type
        };

        let mut n = self.state.next_item.lock().await;
        let id = format!("{}", *n);
        *n += 1;
        drop(n);

        let removed_ids = {
            let mut vault = self.state.vault.lock().await;
            let before: Vec<String> = if replace {
                vault
                    .search(&attributes)
                    .into_iter()
                    .map(|i| i.id.clone())
                    .collect()
            } else {
                Vec::new()
            };
            vault
                .upsert(id.clone(), label, attributes, value, content_type, replace)
                .map_err(|e| fdo::Error::Failed(e.to_string()))?;
            before
                .into_iter()
                .filter(|old| old != &id)
                .collect::<Vec<_>>()
        };
        for old in removed_ids {
            let _ = self
                .state
                .connection
                .object_server()
                .remove::<ItemIface, _>(item_path(&old).as_ref())
                .await;
        }

        let path = item_path(&id);
        let iface = ItemIface {
            state: self.state.clone(),
            id: id.clone(),
        };
        self.state
            .connection
            .object_server()
            .at(path.clone(), iface)
            .await
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;

        let _ = CollectionIface::item_created(&emitter, path.clone()).await;
        Ok((path, empty_path()))
    }

    #[zbus(signal)]
    async fn item_created(emitter: &SignalEmitter<'_>, item: OwnedObjectPath) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn item_deleted(emitter: &SignalEmitter<'_>, item: OwnedObjectPath) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn item_changed(emitter: &SignalEmitter<'_>, item: OwnedObjectPath) -> zbus::Result<()>;
}

#[interface(name = "org.freedesktop.Secret.Item")]
impl ItemIface {
    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.state.vault.lock().await.is_locked()
    }

    #[zbus(property)]
    async fn label(&self) -> fdo::Result<String> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Err(fdo::Error::AccessDenied("vault is locked".into()));
        }
        vault
            .get(&self.id)
            .map(|i| i.label.clone())
            .ok_or_else(|| fdo::Error::UnknownObject("item gone".into()))
    }

    #[zbus(property)]
    async fn set_label(&self, label: String) -> fdo::Result<()> {
        self.state
            .vault
            .lock()
            .await
            .set_label(&self.id, label)
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(property)]
    async fn attributes(&self) -> fdo::Result<HashMap<String, String>> {
        let vault = self.state.vault.lock().await;
        vault
            .get(&self.id)
            .map(|i| i.attributes.clone())
            .ok_or_else(|| fdo::Error::UnknownObject("item gone".into()))
    }

    #[zbus(property)]
    async fn set_attributes(&self, attributes: HashMap<String, String>) -> fdo::Result<()> {
        self.state
            .vault
            .lock()
            .await
            .set_attributes(&self.id, attributes)
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }

    #[zbus(property, name = "Type")]
    async fn item_type(&self) -> String {
        // Generic password schema used by libsecret / oo7 text secrets.
        "org.freedesktop.Secret.Generic".into()
    }

    #[zbus(property)]
    async fn created(&self) -> fdo::Result<u64> {
        let vault = self.state.vault.lock().await;
        vault
            .get(&self.id)
            .map(|i| i.created)
            .ok_or_else(|| fdo::Error::UnknownObject("item gone".into()))
    }

    #[zbus(property)]
    async fn modified(&self) -> fdo::Result<u64> {
        let vault = self.state.vault.lock().await;
        vault
            .get(&self.id)
            .map(|i| i.modified)
            .ok_or_else(|| fdo::Error::UnknownObject("item gone".into()))
    }

    async fn delete(&self) -> fdo::Result<OwnedObjectPath> {
        self.state
            .vault
            .lock()
            .await
            .remove(&self.id)
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let path = item_path(&self.id);
        let _ = self
            .state
            .connection
            .object_server()
            .remove::<ItemIface, _>(path.as_ref())
            .await;
        Ok(empty_path())
    }

    async fn get_secret(&self, session: OwnedObjectPath) -> fdo::Result<WireSecret> {
        let vault = self.state.vault.lock().await;
        if vault.is_locked() {
            return Err(fdo::Error::AccessDenied("vault is locked".into()));
        }
        let item = vault
            .get(&self.id)
            .ok_or_else(|| fdo::Error::UnknownObject("item gone".into()))?;
        Ok(WireSecret {
            session,
            parameters: Vec::new(),
            value: item.secret.as_bytes().to_vec(),
            content_type: item.content_type.clone(),
        })
    }

    async fn set_secret(&self, secret: WireSecret) -> fdo::Result<()> {
        let value = String::from_utf8_lossy(&secret.value).into_owned();
        let content_type = if secret.content_type.is_empty() {
            "text/plain".into()
        } else {
            secret.content_type
        };
        self.state
            .vault
            .lock()
            .await
            .set_secret(&self.id, value, content_type)
            .map_err(|e| match e {
                crate::vault::VaultError::Locked => fdo::Error::AccessDenied(e.to_string()),
                other => fdo::Error::Failed(other.to_string()),
            })
    }
}

/// Metis extension: password unlock / rewrap (complements Secret Service Prompt).
pub struct MetisSecretsIface {
    pub state: AppState,
}

#[interface(name = "org.metis.Secrets1")]
impl MetisSecretsIface {
    #[zbus(property)]
    async fn locked(&self) -> bool {
        self.state.vault.lock().await.is_locked()
    }

    /// Unlock with the login password (PAM) or initialize/migrate the wrap.
    async fn unlock_with_password(
        &self,
        password: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let mut vault = self.state.vault.lock().await;
        vault.unlock_with_password(password).map_err(|e| match e {
            crate::vault::VaultError::AuthFailed => {
                fdo::Error::AccessDenied("authentication failed".into())
            }
            other => fdo::Error::Failed(other.to_string()),
        })?;
        drop(vault);
        let _ = register_existing_items(&self.state).await;
        self.state.unlock_notify.notify_waiters();
        let _ = MetisSecretsIface::unlocked(&emitter).await;
        Ok(())
    }

    /// Rewrite `vault.wrap` while unlocked (after login password change).
    async fn rewrap_unlocked(&self, new_password: &str) -> fdo::Result<()> {
        self.state
            .vault
            .lock()
            .await
            .rewrap_unlocked(new_password)
            .map_err(map_vault_auth)
    }

    /// Unwrap with the old wrap password and re-wrap with the new login password.
    async fn rewrap_with_passwords(
        &self,
        old_password: &str,
        new_password: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        {
            let mut vault = self.state.vault.lock().await;
            vault
                .rewrap_with_passwords(old_password, new_password)
                .map_err(map_vault_auth)?;
        }
        let _ = register_existing_items(&self.state).await;
        self.state.unlock_notify.notify_waiters();
        let _ = MetisSecretsIface::unlocked(&emitter).await;
        Ok(())
    }

    async fn lock(&self) -> fdo::Result<()> {
        self.state.vault.lock().await.lock_vault();
        self.state.unlock_notify.notify_waiters();
        Ok(())
    }

    /// Emitted when a Secret Service Prompt needs the shell unlock UI.
    #[zbus(signal)]
    async fn unlock_prompt_needed(
        emitter: &SignalEmitter<'_>,
        prompt_path: OwnedObjectPath,
    ) -> zbus::Result<()>;

    /// Emitted after a successful unlock / rewrap that opens the vault.
    #[zbus(signal)]
    async fn unlocked(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

fn map_vault_auth(e: crate::vault::VaultError) -> fdo::Error {
    match e {
        crate::vault::VaultError::AuthFailed => {
            fdo::Error::AccessDenied("authentication failed".into())
        }
        crate::vault::VaultError::Locked => fdo::Error::AccessDenied(e.to_string()),
        other => fdo::Error::Failed(other.to_string()),
    }
}

pub struct PromptIface {
    state: AppState,
    id: u64,
    pending_objects: Vec<OwnedObjectPath>,
}

#[interface(name = "org.freedesktop.Secret.Prompt")]
impl PromptIface {
    /// Show / wait for the unlock UI. Shell listens for `UnlockPromptNeeded`.
    async fn prompt(
        &self,
        _window_id: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> fdo::Result<()> {
        let path = prompt_path(self.id);
        // Ask the Metis shell (or any listener) to show the unlock dialog.
        if let Ok(metis_emitter) = SignalEmitter::new(&self.state.connection, SERVICE_PATH) {
            let _ = MetisSecretsIface::unlock_prompt_needed(&metis_emitter, path.clone()).await;
        }

        // Wait until unlock succeeds or the vault stays locked after a notify
        // (dismiss / lock). Cap wait so a missing shell cannot hang clients forever.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(300);
        loop {
            if !self.state.vault.lock().await.is_locked() {
                let result = Value::from(self.pending_objects.clone())
                    .try_to_owned()
                    .map_err(|e| fdo::Error::Failed(e.to_string()))?;
                let _ = PromptIface::completed(&emitter, false, result).await;
                let _ = self
                    .state
                    .connection
                    .object_server()
                    .remove::<PromptIface, _>(path.as_ref())
                    .await;
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            let notified = self.state.unlock_notify.notified();
            if tokio::time::timeout(remaining, notified).await.is_err() {
                break;
            }
        }
        let empty = Value::from(Vec::<OwnedObjectPath>::new())
            .try_to_owned()
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let _ = PromptIface::completed(&emitter, true, empty).await;
        let _ = self
            .state
            .connection
            .object_server()
            .remove::<PromptIface, _>(path.as_ref())
            .await;
        Ok(())
    }

    async fn dismiss(&self, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> fdo::Result<()> {
        self.state.unlock_notify.notify_waiters();
        let path = prompt_path(self.id);
        let empty = Value::from(Vec::<OwnedObjectPath>::new())
            .try_to_owned()
            .map_err(|e| fdo::Error::Failed(e.to_string()))?;
        let _ = PromptIface::completed(&emitter, true, empty).await;
        let _ = self
            .state
            .connection
            .object_server()
            .remove::<PromptIface, _>(path.as_ref())
            .await;
        Ok(())
    }

    #[zbus(signal)]
    async fn completed(
        emitter: &SignalEmitter<'_>,
        dismissed: bool,
        result: OwnedValue,
    ) -> zbus::Result<()>;
}

#[interface(name = "org.freedesktop.Secret.Session")]
impl SessionIface {
    async fn close(&self) -> fdo::Result<()> {
        self.state.sessions.lock().await.remove(&self.id);
        let path = session_path(&self.id);
        let _ = self
            .state
            .connection
            .object_server()
            .remove::<SessionIface, _>(ObjectPath::from(path))
            .await;
        Ok(())
    }
}

pub async fn register_existing_items(state: &AppState) -> zbus::Result<()> {
    let ids: Vec<String> = {
        let vault = state.vault.lock().await;
        vault.items().map(|i| i.id.clone()).collect()
    };
    let mut max_id = 0u64;
    for id in ids {
        if let Ok(n) = id.parse::<u64>() {
            max_id = max_id.max(n);
        }
        let iface = ItemIface {
            state: state.clone(),
            id: id.clone(),
        };
        state
            .connection
            .object_server()
            .at(item_path(&id), iface)
            .await?;
    }
    *state.next_item.lock().await = max_id + 1;
    Ok(())
}
