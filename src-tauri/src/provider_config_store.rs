use crate::error::GlossError;
use aes_gcm::aead::{Aead, Generate, KeyInit, Nonce as AeadNonce};
use aes_gcm::Aes256Gcm;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};

const SECRET_KEY_FILENAME: &str = "secret-store.key";
const SECRET_DATA_FILENAME: &str = "secret-store.enc";

#[derive(Debug, Clone)]
pub struct SecretStore {
    dir: PathBuf,
    gate: Arc<Mutex<()>>,
}

#[derive(Debug, Serialize, Deserialize)]
struct EncryptedSecrets {
    nonce_b64: String,
    ciphertext_b64: String,
}

impl SecretStore {
    pub fn new(data_dir: &Path) -> Result<Self, GlossError> {
        let dir = data_dir.join("secrets");
        fs::create_dir_all(&dir)?;
        set_owner_only_dir_permissions(&dir)?;
        let dir = fs::canonicalize(dir)?;
        let gate = secret_store_gate(&dir)?;
        Ok(Self { dir, gate })
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, GlossError> {
        let _guard = self.lock()?;
        let secrets = self.load_all()?;
        Ok(secrets.get(key).cloned())
    }

    pub fn contains(&self, key: &str) -> Result<bool, GlossError> {
        Ok(self.get(key)?.is_some())
    }

    pub fn set(&self, key: &str, value: Option<&str>) -> Result<(), GlossError> {
        let _guard = self.lock()?;
        let mut secrets = self.load_all()?;
        match value.map(str::trim) {
            Some(value) if !value.is_empty() => {
                secrets.insert(key.to_string(), value.to_string());
            }
            _ => {
                secrets.remove(key);
            }
        }
        self.save_all(&secrets)
    }

    fn lock(&self) -> Result<MutexGuard<'_, ()>, GlossError> {
        self.gate
            .lock()
            .map_err(|_| GlossError::Other("Secret-store operation lock is poisoned".into()))
    }

    fn load_all(&self) -> Result<HashMap<String, String>, GlossError> {
        let data_path = self.data_path();
        if !data_path.exists() {
            return Ok(HashMap::new());
        }
        set_owner_only_permissions(&data_path)?;

        let encrypted: EncryptedSecrets =
            serde_json::from_slice(&fs::read(&data_path)?).map_err(GlossError::JsonParse)?;
        let nonce = base64::engine::general_purpose::STANDARD
            .decode(encrypted.nonce_b64)
            .map_err(|e| GlossError::Other(format!("Invalid secret nonce encoding: {e}")))?;
        let ciphertext = base64::engine::general_purpose::STANDARD
            .decode(encrypted.ciphertext_b64)
            .map_err(|e| GlossError::Other(format!("Invalid secret ciphertext encoding: {e}")))?;

        if nonce.len() != 12 {
            return Err(GlossError::Other(
                "Secret store nonce has invalid length".into(),
            ));
        }

        let key = self.load_or_create_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| GlossError::Other(format!("Failed to initialize secret cipher: {e}")))?;
        let nonce: AeadNonce<Aes256Gcm> = nonce
            .as_slice()
            .try_into()
            .map_err(|_| GlossError::Other("Secret store nonce has invalid length".into()))?;
        let plaintext = cipher
            .decrypt(&nonce, ciphertext.as_ref())
            .map_err(|e| GlossError::Other(format!("Failed to decrypt local secret store: {e}")))?;

        serde_json::from_slice(&plaintext).map_err(GlossError::JsonParse)
    }

    fn save_all(&self, secrets: &HashMap<String, String>) -> Result<(), GlossError> {
        let key = self.load_or_create_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| GlossError::Other(format!("Failed to initialize secret cipher: {e}")))?;

        let nonce = AeadNonce::<Aes256Gcm>::generate();

        let plaintext = serde_json::to_vec(secrets)?;
        let ciphertext = cipher
            .encrypt(&nonce, plaintext.as_ref())
            .map_err(|e| GlossError::Other(format!("Failed to encrypt local secret store: {e}")))?;

        let payload = EncryptedSecrets {
            nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce.as_slice()),
            ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(ciphertext),
        };

        let tmp_path = self.dir.join(format!(
            ".{SECRET_DATA_FILENAME}.{}.tmp",
            uuid::Uuid::new_v4()
        ));
        let mut pending = write_owner_only_file(&tmp_path, &serde_json::to_vec(&payload)?)?;
        fs::rename(&pending.path, self.data_path())?;
        pending.committed = true;
        Ok(())
    }

    fn load_or_create_key(&self) -> Result<[u8; 32], GlossError> {
        let key_path = self.key_path();
        if key_path.exists() {
            set_owner_only_permissions(&key_path)?;
            let bytes = fs::read(&key_path)?;
            return bytes
                .try_into()
                .map_err(|_| GlossError::Other("Secret-store key has invalid length".into()));
        }

        let key: [u8; 32] = <[u8; 32] as Generate>::generate();

        let mut pending = write_owner_only_file(&key_path, &key)?;
        pending.committed = true;

        Ok(key)
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join(SECRET_KEY_FILENAME)
    }

    fn data_path(&self) -> PathBuf {
        self.dir.join(SECRET_DATA_FILENAME)
    }
}

fn secret_store_gate(dir: &Path) -> Result<Arc<Mutex<()>>, GlossError> {
    // Separate instances and clones must serialize the entire read/modify/commit
    // operation. This registry coordinates this process only, not other processes.
    static GATES: OnceLock<Mutex<HashMap<PathBuf, Weak<Mutex<()>>>>> = OnceLock::new();
    let mut gates = GATES
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| GlossError::Other("Secret-store lock registry is poisoned".into()))?;
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(dir).and_then(Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(Mutex::new(()));
    gates.insert(dir.to_path_buf(), Arc::downgrade(&gate));
    Ok(gate)
}

struct PendingSecretFile {
    path: PathBuf,
    committed: bool,
}

impl Drop for PendingSecretFile {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[cfg(unix)]
fn set_owner_only_dir_permissions(path: &Path) -> Result<(), GlossError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_dir_permissions(path: &Path) -> Result<(), GlossError> {
    let path_str = match path.to_str() {
        Some(s) => s,
        None => return Ok(()),
    };
    // Remove inherited permissions and grant current user only
    let username = std::env::var("USERNAME").unwrap_or_else(|_| "*S-1-1-0".to_string());
    let output = std::process::Command::new("icacls")
        .arg(path_str)
        .arg("/inheritance:r")
        .arg(format!("/grant:r:{}:(R)", username))
        .output();
    match output {
        Ok(out) if out.status.success() => Ok(()),
        Ok(out) => {
            tracing::warn!(
                "Failed to set restrictive permissions on {:?}: icacls exit {:?}, stderr: {}",
                path,
                out.status,
                String::from_utf8_lossy(&out.stderr)
            );
            Ok(())
        }
        Err(e) => {
            tracing::warn!("Failed to run icacls on {:?}: {}", path, e);
            Ok(())
        }
    }
}

#[cfg(unix)]
fn set_owner_only_permissions(path: &Path) -> Result<(), GlossError> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<(), GlossError> {
    Ok(())
}

fn write_owner_only_file(path: &Path, bytes: &[u8]) -> Result<PendingSecretFile, GlossError> {
    let mut file = open_owner_only_create_new(path)?;
    // Register cleanup only after exclusive creation has established ownership.
    let pending = PendingSecretFile {
        path: path.to_path_buf(),
        committed: false,
    };
    let result = (|| {
        file.write_all(bytes)?;
        file.sync_all()?;
        set_owner_only_permissions(path)
    })();
    // Close before cleanup or rename, including on Windows.
    drop(file);
    result?;
    Ok(pending)
}

#[cfg(unix)]
fn open_owner_only_create_new(path: &Path) -> Result<std::fs::File, GlossError> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(not(unix))]
fn open_owner_only_create_new(path: &Path) -> Result<std::fs::File, GlossError> {
    Ok(OpenOptions::new().write(true).create_new(true).open(path)?)
}

#[cfg(test)]
mod tests {
    use super::{write_owner_only_file, SecretStore, SECRET_DATA_FILENAME, SECRET_KEY_FILENAME};
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use tempfile::tempdir;

    #[test]
    fn test_secret_store_round_trip() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();

        store.set("openai_api_key", Some("sk-test")).unwrap();
        assert_eq!(
            store.get("openai_api_key").unwrap(),
            Some("sk-test".to_string())
        );

        store.set("openai_api_key", Some("")).unwrap();
        assert_eq!(store.get("openai_api_key").unwrap(), None);
    }

    #[test]
    fn barrier_regression_preserves_concurrent_store_updates() {
        const WORKERS: usize = 8;
        const ROUNDS: usize = 8;

        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();
        store
            .set("synthetic-baseline", Some("synthetic-value"))
            .unwrap();
        let barrier = Arc::new(Barrier::new(WORKERS));
        let mut threads = Vec::new();
        for worker in 0..WORKERS {
            let store = if worker % 2 == 0 {
                store.clone()
            } else {
                // Independently constructed stores and lexical aliases must
                // share the same read/modify/commit gate.
                SecretStore::new(&dir.path().join(".")).unwrap()
            };
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                let mut errors = Vec::new();
                for round in 0..ROUNDS {
                    barrier.wait();
                    let key = format!("synthetic-secret-{worker}-{round}");
                    if let Err(error) = store.set(&key, Some("synthetic-value")) {
                        // Finish every barrier even after an error so a failed
                        // regression produces a result instead of hanging.
                        errors.push(error.to_string());
                    }
                }
                errors
            }));
        }
        for thread in threads {
            assert!(thread.join().unwrap().is_empty());
        }
        assert_eq!(
            store.get("synthetic-baseline").unwrap().as_deref(),
            Some("synthetic-value")
        );
        for worker in 0..WORKERS {
            for round in 0..ROUNDS {
                assert_eq!(
                    store
                        .get(&format!("synthetic-secret-{worker}-{round}"))
                        .unwrap()
                        .as_deref(),
                    Some("synthetic-value")
                );
            }
        }
    }

    #[test]
    fn concurrent_first_writes_share_one_key() {
        const WORKERS: usize = 8;

        let dir = tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(WORKERS));
        let mut threads = Vec::new();
        for worker in 0..WORKERS {
            let store = SecretStore::new(dir.path()).unwrap();
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                barrier.wait();
                store.set(
                    &format!("synthetic-secret-{worker}"),
                    Some("synthetic-value"),
                )
            }));
        }
        for thread in threads {
            thread.join().unwrap().unwrap();
        }
        let store = SecretStore::new(dir.path()).unwrap();
        for worker in 0..WORKERS {
            assert!(store
                .contains(&format!("synthetic-secret-{worker}"))
                .unwrap());
        }
        assert_eq!(fs::read(store.key_path()).unwrap().len(), 32);
    }

    #[test]
    fn failed_commit_preserves_target_and_removes_staging_file() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();
        let data_path = store.data_path();
        fs::create_dir(&data_path).unwrap();
        let marker = data_path.join("synthetic-marker");
        fs::write(&marker, b"synthetic-unchanged").unwrap();
        let secrets = HashMap::from([(
            "synthetic-secret".to_string(),
            "synthetic-value".to_string(),
        )]);

        assert!(store.save_all(&secrets).is_err());
        assert_eq!(fs::read(marker).unwrap(), b"synthetic-unchanged");
        let mut filenames: Vec<_> = fs::read_dir(&store.dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        filenames.sort();
        assert_eq!(
            filenames,
            [SECRET_DATA_FILENAME, SECRET_KEY_FILENAME].map(std::ffi::OsString::from)
        );
    }

    #[test]
    fn pending_write_cleanup_only_removes_owned_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("synthetic-stage");
        fs::write(&path, b"synthetic-existing").unwrap();
        assert!(write_owner_only_file(&path, b"synthetic-replacement").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"synthetic-existing");
        fs::remove_file(&path).unwrap();

        let pending = write_owner_only_file(&path, b"synthetic-new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"synthetic-new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        drop(pending);
        assert!(!path.exists());
    }

    #[test]
    fn commits_do_not_touch_legacy_fixed_temporary_path() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();
        let legacy_tmp = store.dir.join(format!("{SECRET_DATA_FILENAME}.tmp"));
        fs::write(&legacy_tmp, b"synthetic-existing").unwrap();

        store
            .set("synthetic-secret", Some("synthetic-value"))
            .unwrap();
        assert_eq!(fs::read(legacy_tmp).unwrap(), b"synthetic-existing");
        assert!(store.contains("synthetic-secret").unwrap());
    }

    #[test]
    fn poisoned_operation_lock_fails_closed() {
        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();
        store
            .set("synthetic-secret", Some("synthetic-value"))
            .unwrap();
        let original = fs::read(store.data_path()).unwrap();
        let gate = store.gate.clone();
        assert!(thread::spawn(move || {
            let _guard = gate.lock().unwrap();
            panic!("synthetic lock poisoning");
        })
        .join()
        .is_err());

        assert!(store.get("synthetic-secret").is_err());
        assert!(store
            .set("synthetic-other", Some("synthetic-value"))
            .is_err());
        assert_eq!(fs::read(store.data_path()).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn secret_store_repairs_owner_only_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempdir().unwrap();
        let store = SecretStore::new(dir.path()).unwrap();
        store.set("openai_api_key", Some("sk-test")).unwrap();

        let secret_dir = dir.path().join("secrets");
        let key_path = secret_dir.join("secret-store.key");
        let data_path = secret_dir.join("secret-store.enc");
        assert_eq!(
            std::fs::metadata(&secret_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&data_path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::set_permissions(&data_path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(
            store.get("openai_api_key").unwrap().as_deref(),
            Some("sk-test")
        );
        store
            .set("anthropic_api_key", Some("sk-anthropic"))
            .unwrap();

        assert_eq!(
            std::fs::metadata(&key_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&data_path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
