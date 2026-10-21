//! Encrypted on-disk vault under `$XDG_DATA_HOME/metis/secrets/`.
//!
//! At rest the DEK is wrapped with Argon2id(login password) in `vault.wrap`.
//! Legacy plaintext `vault.key` is migrated on first successful PAM unlock.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, ZeroizeOnDrop};

const KEY_FILE: &str = "vault.key";
const WRAP_FILE: &str = "vault.wrap";
const DATA_FILE: &str = "vault.bin";
const MAGIC: &[u8; 4] = b"MSK1";
const WRAP_MAGIC: &[u8; 4] = b"MSW1";
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 12;

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
    #[error("vault is locked")]
    Locked,
    #[error("authentication failed")]
    AuthFailed,
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
    /// `None` while locked — DEK must not sit on disk unwrapped.
    key: Option<MasterKey>,
    items: HashMap<String, StoredItem>,
    collection_created: u64,
    collection_modified: u64,
}

impl Vault {
    /// Open the vault directory. Starts **locked** (no DEK in memory).
    pub fn open_default() -> Result<Self, VaultError> {
        let dir = data_dir()?;
        fs::create_dir_all(&dir)?;
        let mut perms = fs::metadata(&dir)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&dir, perms)?;

        let now = unix_now();
        Ok(Self {
            dir,
            key: None,
            items: HashMap::new(),
            collection_created: now,
            collection_modified: now,
        })
    }

    pub fn is_locked(&self) -> bool {
        self.key.is_none()
    }

    /// PAM-check password, then unwrap / migrate / initialize the DEK.
    ///
    /// CI may set `METIS_SECRETS_TEST_UNLOCK` to a passphrase that skips PAM
    /// (never set in production sessions).
    pub fn unlock_with_password(&mut self, password: &str) -> Result<(), VaultError> {
        if password.is_empty() {
            return Err(VaultError::AuthFailed);
        }
        Self::pam_check_password(password)?;
        self.unlock_crypto(password)
    }

    /// Unit-test helper: unwrap without PAM.
    #[cfg(test)]
    pub fn unlock_for_test(&mut self, password: &str) -> Result<(), VaultError> {
        self.unlock_crypto(password)
    }

    fn unlock_crypto(&mut self, password: &str) -> Result<(), VaultError> {
        if self.key.is_some() {
            return Ok(());
        }

        let wrap_path = self.dir.join(WRAP_FILE);
        let key_path = self.dir.join(KEY_FILE);
        let data_path = self.dir.join(DATA_FILE);

        let key = if wrap_path.is_file() {
            unwrap_dek(&wrap_path, password)?
        } else if key_path.is_file() {
            let legacy = load_legacy_key(&key_path)?;
            write_wrap(&wrap_path, password, &legacy)?;
            let _ = fs::remove_file(&key_path);
            tracing::info!("migrated legacy vault.key to password-wrapped vault.wrap");
            legacy
        } else {
            let mut key = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut key);
            let dek = MasterKey(key);
            write_wrap(&wrap_path, password, &dek)?;
            dek
        };

        let (items, created, modified) = if data_path.exists() {
            let file = decrypt_file(&data_path, &key)?;
            let now = unix_now();
            let modified = file.items.iter().map(|i| i.modified).max().unwrap_or(now);
            let created = file.items.iter().map(|i| i.created).min().unwrap_or(now);
            let map = file.items.into_iter().map(|i| (i.id.clone(), i)).collect();
            (map, created, modified)
        } else {
            let now = unix_now();
            (HashMap::new(), now, now)
        };

        self.key = Some(key);
        self.items = items;
        self.collection_created = created;
        self.collection_modified = modified;
        Ok(())
    }

    /// Drop the DEK and clear secrets from memory.
    pub fn lock_vault(&mut self) {
        self.key = None;
        self.items.clear();
    }

    /// Rewrite `vault.wrap` with a new login password while the DEK is already
    /// held in memory (vault unlocked). PAM-checks `new_password`.
    pub fn rewrap_unlocked(&mut self, new_password: &str) -> Result<(), VaultError> {
        if new_password.is_empty() {
            return Err(VaultError::AuthFailed);
        }
        self.require_unlocked()?;
        Self::pam_check_password(new_password)?;
        let wrap_path = self.dir.join(WRAP_FILE);
        let dek = self.key.as_ref().ok_or(VaultError::Locked)?;
        write_wrap(&wrap_path, new_password, dek)
    }

    /// Unlock-or-rewrap after a login password change: unwrap with `old_password`,
    /// PAM-check `new_password`, rewrite wrap. Loads the vault if it was locked.
    pub fn rewrap_with_passwords(
        &mut self,
        old_password: &str,
        new_password: &str,
    ) -> Result<(), VaultError> {
        if old_password.is_empty() || new_password.is_empty() {
            return Err(VaultError::AuthFailed);
        }
        Self::pam_check_password(new_password)?;
        let wrap_path = self.dir.join(WRAP_FILE);
        if !wrap_path.is_file() {
            // No wrap yet — initialize with the new password.
            return self.unlock_with_password(new_password);
        }
        let dek = if let Some(ref key) = self.key {
            // Already unlocked: verify old still unwraps (optional integrity),
            // then rewrite with new. Wrong old → AuthFailed so Settings can
            // prompt again.
            let check = unwrap_dek(&wrap_path, old_password)?;
            if check.0 != key.0 {
                return Err(VaultError::AuthFailed);
            }
            key.0
        } else {
            let opened = unwrap_dek(&wrap_path, old_password)?;
            // Load items under the recovered DEK, then rewrap.
            let data_path = self.dir.join(DATA_FILE);
            let (items, created, modified) = if data_path.exists() {
                let file = decrypt_file(&data_path, &opened)?;
                let now = unix_now();
                let modified = file.items.iter().map(|i| i.modified).max().unwrap_or(now);
                let created = file.items.iter().map(|i| i.created).min().unwrap_or(now);
                let map = file.items.into_iter().map(|i| (i.id.clone(), i)).collect();
                (map, created, modified)
            } else {
                let now = unix_now();
                (HashMap::new(), now, now)
            };
            let raw = opened.0;
            self.key = Some(opened);
            self.items = items;
            self.collection_created = created;
            self.collection_modified = modified;
            raw
        };
        write_wrap(&wrap_path, new_password, &MasterKey(dek))
    }

    fn pam_check_password(password: &str) -> Result<(), VaultError> {
        let pam_ok = match std::env::var("METIS_SECRETS_TEST_UNLOCK") {
            Ok(expected) if !expected.is_empty() && expected == password => true,
            _ => {
                let user = crate::pam_auth::current_username().ok_or(VaultError::AuthFailed)?;
                let service = crate::pam_auth::pam_service();
                crate::pam_auth::pam_check(&service, &user, password)
            }
        };
        if pam_ok {
            Ok(())
        } else {
            Err(VaultError::AuthFailed)
        }
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
        self.require_unlocked()?;
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
        self.require_unlocked()?;
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
        self.require_unlocked()?;
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
        self.require_unlocked()?;
        let Some(item) = self.items.get_mut(id) else {
            return Err(VaultError::Corrupt);
        };
        item.attributes = attributes;
        item.modified = unix_now();
        self.collection_modified = item.modified;
        self.persist()
    }

    pub fn remove(&mut self, id: &str) -> Result<bool, VaultError> {
        self.require_unlocked()?;
        let removed = self.items.remove(id).is_some();
        if removed {
            self.collection_modified = unix_now();
            self.persist()?;
        }
        Ok(removed)
    }

    fn require_unlocked(&self) -> Result<(), VaultError> {
        if self.key.is_none() {
            Err(VaultError::Locked)
        } else {
            Ok(())
        }
    }

    fn persist(&self) -> Result<(), VaultError> {
        let key = self.key.as_ref().ok_or(VaultError::Locked)?;
        let file = VaultFile {
            items: self.items.values().cloned().collect(),
        };
        let plain = serde_json::to_vec(&file)?;
        encrypt_file(&self.dir.join(DATA_FILE), key, &plain)
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

fn load_legacy_key(path: &Path) -> Result<MasterKey, VaultError> {
    let bytes = fs::read(path)?;
    if bytes.len() != 32 {
        return Err(VaultError::Corrupt);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&bytes);
    Ok(MasterKey(key))
}

fn derive_kek(password: &str, salt: &[u8]) -> Result<[u8; 32], VaultError> {
    let params = Params::new(19_456, 2, 1, Some(32)).map_err(|_| VaultError::Crypto)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    argon
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .map_err(|_| VaultError::Crypto)?;
    Ok(out)
}

fn write_wrap(path: &Path, password: &str, dek: &MasterKey) -> Result<(), VaultError> {
    let mut salt = [0u8; SALT_LEN];
    rand::thread_rng().fill_bytes(&mut salt);
    let kek = derive_kek(password, &salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&kek).map_err(|_| VaultError::Crypto)?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, dek.0.as_ref())
        .map_err(|_| VaultError::Crypto)?;
    let mut out = Vec::with_capacity(4 + SALT_LEN + NONCE_LEN + ciphertext.len());
    out.extend_from_slice(WRAP_MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    atomic_write_0600(path, &out)
}

fn unwrap_dek(path: &Path, password: &str) -> Result<MasterKey, VaultError> {
    let data = fs::read(path)?;
    if data.len() < 4 + SALT_LEN + NONCE_LEN + 16 || &data[..4] != WRAP_MAGIC {
        return Err(VaultError::Corrupt);
    }
    let salt = &data[4..4 + SALT_LEN];
    let nonce = Nonce::from_slice(&data[4 + SALT_LEN..4 + SALT_LEN + NONCE_LEN]);
    let ct = &data[4 + SALT_LEN + NONCE_LEN..];
    let kek = derive_kek(password, salt)?;
    let cipher = ChaCha20Poly1305::new_from_slice(&kek).map_err(|_| VaultError::Crypto)?;
    let plain = cipher
        .decrypt(nonce, ct)
        .map_err(|_| VaultError::AuthFailed)?;
    if plain.len() != 32 {
        return Err(VaultError::Corrupt);
    }
    let mut key = [0u8; 32];
    key.copy_from_slice(&plain);
    Ok(MasterKey(key))
}

fn atomic_write_0600(path: &Path, bytes: &[u8]) -> Result<(), VaultError> {
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true).mode(0o600);
        let mut f = opts.open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
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
    atomic_write_0600(path, &out)
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

    fn test_dir(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "metis-secretsd-vault-{tag}-{}-{}",
            std::process::id(),
            unix_now()
        ))
    }

    #[test]
    fn wrap_round_trip_without_pam() {
        let dir = test_dir("wrap");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut dek = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut dek);
        let key = MasterKey(dek);
        let wrap = dir.join(WRAP_FILE);
        write_wrap(&wrap, "test-passphrase-not-pam", &key).unwrap();
        let opened = unwrap_dek(&wrap, "test-passphrase-not-pam").unwrap();
        assert_eq!(opened.0, key.0);
        assert!(unwrap_dek(&wrap, "wrong").is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrap_unlocked_rewrites_wrap() {
        let dir = test_dir("rewrap");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut vault = Vault {
            dir: dir.clone(),
            key: None,
            items: HashMap::new(),
            collection_created: unix_now(),
            collection_modified: unix_now(),
        };
        vault.unlock_for_test("old-pass").unwrap();
        // Unit path: rewrite wrap without PAM (production uses rewrap_unlocked).
        let dek = vault.key.as_ref().unwrap();
        write_wrap(&dir.join(WRAP_FILE), "new-pass", dek).unwrap();
        vault.lock_vault();
        assert!(vault.unlock_for_test("old-pass").is_err());
        assert!(vault.unlock_for_test("new-pass").is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rewrap_with_passwords_from_locked() {
        let dir = test_dir("rewrap-locked");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut vault = Vault {
            dir: dir.clone(),
            key: None,
            items: HashMap::new(),
            collection_created: unix_now(),
            collection_modified: unix_now(),
        };
        vault.unlock_for_test("legacy").unwrap();
        vault.lock_vault();
        // Simulate rewrap crypto without PAM race across parallel tests.
        let dek = unwrap_dek(&dir.join(WRAP_FILE), "legacy").unwrap();
        write_wrap(&dir.join(WRAP_FILE), "brand-new", &dek).unwrap();
        assert!(vault.unlock_for_test("brand-new").is_ok());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn locked_until_unlock_for_test() {
        let dir = test_dir("locked");
        let _ = fs::remove_dir_all(&dir);
        unsafe { std::env::set_var("METIS_SECRETS_DIR", &dir) };
        let mut vault = Vault::open_default().expect("open");
        assert!(vault.is_locked());
        assert!(matches!(vault.require_unlocked(), Err(VaultError::Locked)));
        vault.unlock_for_test("unit-test-pass").expect("unlock");
        assert!(!vault.is_locked());
        let mut attrs = HashMap::new();
        attrs.insert("app".into(), "metis".into());
        vault
            .upsert(
                "1".into(),
                "t".into(),
                attrs.clone(),
                "s".into(),
                "text/plain".into(),
                true,
            )
            .expect("upsert");
        vault.lock_vault();
        assert!(vault.is_locked());
        vault.unlock_for_test("unit-test-pass").expect("reopen");
        assert_eq!(vault.search(&attrs)[0].secret, "s");
        unsafe { std::env::remove_var("METIS_SECRETS_DIR") };
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn legacy_key_migrate_via_wrap_helpers() {
        let dir = test_dir("legacy");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let mut raw = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut raw);
        let key_path = dir.join(KEY_FILE);
        {
            let mut opts = fs::OpenOptions::new();
            opts.write(true).create_new(true).mode(0o600);
            let mut f = opts.open(&key_path).unwrap();
            f.write_all(&raw).unwrap();
        }
        let legacy = load_legacy_key(&key_path).unwrap();
        let wrap = dir.join(WRAP_FILE);
        write_wrap(&wrap, "migrate-pass", &legacy).unwrap();
        let _ = fs::remove_file(&key_path);
        let opened = unwrap_dek(&wrap, "migrate-pass").unwrap();
        assert_eq!(opened.0, raw);
        let _ = fs::remove_dir_all(&dir);
    }
}
