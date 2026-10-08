//! Encrypted on-disk vault under `$XDG_DATA_HOME/metis/secrets/`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const KEY_FILE: &str = "vault.key";
const DATA_FILE: &str = "vault.bin";
const MAGIC: &[u8; 4] = b"MSK1";

#[derive(Debug, thiserror::Error)]
pub enum VaultError {
    #[error("io: {0}")]
    Io(#[from] io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("crypto failure")]
    Crypto,
    #[error("corrupt vault")]
    Corrupt,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredItem {
    pub id: String,
    pub label: String,
    pub attributes: HashMap<String, String>,
    /// UTF-8 secret payload (MVP stores text secrets).
    pub secret: String,
    pub content_type: String,
    pub created: u64,
    pub modified: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VaultFile {
    items: Vec<StoredItem>,
}

#[derive(Zeroize, ZeroizeOnDrop)]
struct MasterKey([u8; 32]);

pub struct Vault {
    dir: PathBuf,
    key: MasterKey,
    items: HashMap<String, StoredItem>,
    collection_created: u64,
    collection_modified: u64,
}

impl Vault {
    pub fn open_default() -> Result<Self, VaultError> {
        let dir = data_dir()?;
        fs::create_dir_all(&dir)?;
        let mut perms = fs::metadata(&dir)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms)?;

        let key = load_or_create_key(&dir.join(KEY_FILE))?;
        let path = dir.join(DATA_FILE);
        let (items, created, modified) = if path.exists() {
            let file = decrypt_file(&path, &key)?;
            let now = unix_now();
            let modified = file.items.iter().map(|i| i.modified).max().unwrap_or(now);
            let created = file.items.iter().map(|i| i.created).min().unwrap_or(now);
            let map = file.items.into_iter().map(|i| (i.id.clone(), i)).collect();
            (map, created, modified)
        } else {
            let now = unix_now();
            (HashMap::new(), now, now)
        };

        Ok(Self {
            dir,
            key,
            items,
            collection_created: created,
            collection_modified: modified,
        })
    }

    pub fn collection_created(&self) -> u64 {
        self.collection_created
    }

    pub fn collection_modified(&self) -> u64 {
        self.collection_modified
    }

    pub fn items(&self) -> impl Iterator<Item = &StoredItem> {
        self.items.values()
    }

    pub fn get(&self, id: &str) -> Option<&StoredItem> {
        self.items.get(id)
    }

    pub fn search(&self, attrs: &HashMap<String, String>) -> Vec<&StoredItem> {
        self.items
            .values()
            .filter(|item| {
                attrs
                    .iter()
                    .all(|(k, v)| item.attributes.get(k).map(|x| x == v).unwrap_or(false))
            })
            .collect()
    }

    pub fn upsert(
        &mut self,
        id: String,
        label: String,
        attributes: HashMap<String, String>,
        secret: String,
        content_type: String,
        replace: bool,
    ) -> Result<StoredItem, VaultError> {
        if replace {
            let matching: Vec<String> = self
                .search(&attributes)
                .into_iter()
                .map(|i| i.id.clone())
                .collect();
            for mid in matching {
                self.items.remove(&mid);
            }
        }
        let now = unix_now();
        let created = self.items.get(&id).map(|i| i.created).unwrap_or(now);
        let item = StoredItem {
            id: id.clone(),
            label,
            attributes,
            secret,
            content_type,
            created,
            modified: now,
        };
        self.items.insert(id, item.clone());
        self.collection_modified = now;
        self.persist()?;
        Ok(item)
    }

    pub fn set_secret(
        &mut self,
        id: &str,
        secret: String,
        content_type: String,
    ) -> Result<(), VaultError> {
        let Some(item) = self.items.get_mut(id) else {
            return Err(VaultError::Corrupt);
        };
        item.secret = secret;
        item.content_type = content_type;
        item.modified = unix_now();
        self.collection_modified = item.modified;
        self.persist()
    }

    pub fn set_label(&mut self, id: &str, label: String) -> Result<(), VaultError> {
        let Some(item) = self.items.get_mut(id) else {
            return Err(VaultError::Corrupt);
        };
        item.label = label;
        item.modified = unix_now();
        self.collection_modified = item.modified;
        self.persist()
    }

    pub fn set_attributes(
        &mut self,
        id: &str,
        attributes: HashMap<String, String>,
    ) -> Result<(), VaultError> {
        let Some(item) = self.items.get_mut(id) else {
            return Err(VaultError::Corrupt);
        };
        item.attributes = attributes;
        item.modified = unix_now();
        self.collection_modified = item.modified;
        self.persist()
    }

    pub fn remove(&mut self, id: &str) -> Result<bool, VaultError> {
        let removed = self.items.remove(id).is_some();
        if removed {
            self.collection_modified = unix_now();
            self.persist()?;
        }
        Ok(removed)
    }

    fn persist(&self) -> Result<(), VaultError> {
        let file = VaultFile {
            items: self.items.values().cloned().collect(),
        };
        let plain = serde_json::to_vec(&file)?;
        encrypt_file(&self.dir.join(DATA_FILE), &self.key, &plain)
    }
}

fn data_dir() -> Result<PathBuf, VaultError> {
    if let Ok(dir) = std::env::var("METIS_SECRETS_DIR") {
        return Ok(PathBuf::from(dir));
    }
    Ok(directories::ProjectDirs::from("com", "metis", "metis")
        .map(|d| d.data_dir().join("secrets"))
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var("HOME")
                    .map(|h| format!("{h}/.local/share/metis/secrets"))
                    .unwrap_or_else(|_| ".local/share/metis/secrets".into()),
            )
        }))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn load_or_create_key(path: &Path) -> Result<MasterKey, VaultError> {
    if path.exists() {
        let bytes = fs::read(path)?;
        if bytes.len() != 32 {
            return Err(VaultError::Corrupt);
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(MasterKey(key));
    }
    let mut key = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut key);
    let mut opts = fs::OpenOptions::new();
    opts.write(true).create_new(true).mode(0o600);
    let mut f = opts.open(path)?;
    f.write_all(&key)?;
    f.sync_all()?;
    Ok(MasterKey(key))
}

fn encrypt_file(path: &Path, key: &MasterKey, plain: &[u8]) -> Result<(), VaultError> {
    let cipher = ChaCha20Poly1305::new_from_slice(&key.0).map_err(|_| VaultError::Crypto)?;
    let mut nonce_bytes = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plain)
        .map_err(|_| VaultError::Crypto)?;
    let mut out = Vec::with_capacity(4 + 12 + ciphertext.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    let tmp = path.with_extension("bin.tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(&out)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

fn decrypt_file(path: &Path, key: &MasterKey) -> Result<VaultFile, VaultError> {
    let data = fs::read(path)?;
    if data.len() < 4 + 12 + 16 || &data[..4] != MAGIC {
        return Err(VaultError::Corrupt);
    }
    let nonce = Nonce::from_slice(&data[4..16]);
    let cipher = ChaCha20Poly1305::new_from_slice(&key.0).map_err(|_| VaultError::Crypto)?;
    let plain = cipher
        .decrypt(nonce, &data[16..])
        .map_err(|_| VaultError::Crypto)?;
    Ok(serde_json::from_slice(&plain)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_upsert_search() {
        let dir = std::env::temp_dir().join(format!(
            "metis-secretsd-vault-{}-{}",
            std::process::id(),
            unix_now()
        ));
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("METIS_SECRETS_DIR", &dir) };

        let mut vault = Vault::open_default().expect("open");
        let mut attrs = HashMap::new();
        attrs.insert("app".into(), "metis".into());
        attrs.insert("account".into(), "a1".into());
        attrs.insert("kind".into(), "viewer_password".into());
        vault
            .upsert(
                "item1".into(),
                "test".into(),
                attrs.clone(),
                "s3cret".into(),
                "text/plain".into(),
                true,
            )
            .expect("upsert");

        let vault2 = Vault::open_default().expect("reopen");
        let found = vault2.search(&attrs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].secret, "s3cret");

        unsafe { std::env::remove_var("METIS_SECRETS_DIR") };
        let _ = fs::remove_dir_all(&dir);
    }
}
