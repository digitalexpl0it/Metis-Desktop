//! Minimal `org.freedesktop.Secret.*` server surface for oo7 / libsecret.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
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
}

impl AppState {
    pub fn new(vault: Vault, connection: Connection) -> Self {
        Self {
            vault: Arc::new(Mutex::new(vault)),
            connection,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            next_session: Arc::new(Mutex::new(1)),
            next_item: Arc::new(Mutex::new(1)),
        }
    }
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
        // Vault is session-unlocked while the daemon runs.
        Ok((objects, empty_path()))
    }

    async fn lock(
        &self,
        _objects: Vec<OwnedObjectPath>,
    ) -> fdo::Result<(Vec<OwnedObjectPath>, OwnedObjectPath)> {
        Ok((Vec::new(), empty_path()))
    }

    async fn get_secrets(
        &self,
        items: Vec<OwnedObjectPath>,
        session: OwnedObjectPath,
    ) -> fdo::Result<HashMap<OwnedObjectPath, WireSecret>> {
        let vault = self.state.vault.lock().await;
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
        false
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
        false
    }

    #[zbus(property)]
    async fn label(&self) -> fdo::Result<String> {
        let vault = self.state.vault.lock().await;
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
            .map_err(|e| fdo::Error::Failed(e.to_string()))
    }
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
