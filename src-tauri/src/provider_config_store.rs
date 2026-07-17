use crate::error::GlossError;
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

const SECRET_KEY_FILENAME: &str = "secret-store.key";
const SECRET_DATA_FILENAME: &str = "secret-store.enc";

#[derive(Debug, Clone)]
pub struct SecretStore {
    dir: PathBuf,
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
        Ok(Self { dir })
    }

    pub fn get(&self, key: &str) -> Result<Option<String>, GlossError> {
        let secrets = self.load_all()?;
        Ok(secrets.get(key).cloned())
    }

    pub fn contains(&self, key: &str) -> Result<bool, GlossError> {
        Ok(self.get(key)?.is_some())
    }

    pub fn set(&self, key: &str, value: Option<&str>) -> Result<(), GlossError> {
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
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&nonce), ciphertext.as_ref())
            .map_err(|e| GlossError::Other(format!("Failed to decrypt local secret store: {e}")))?;

        serde_json::from_slice(&plaintext).map_err(GlossError::JsonParse)
    }

    fn save_all(&self, secrets: &HashMap<String, String>) -> Result<(), GlossError> {
        let key = self.load_or_create_key()?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| GlossError::Other(format!("Failed to initialize secret cipher: {e}")))?;

        let mut nonce = [0u8; 12];
        aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut nonce);

        let plaintext = serde_json::to_vec(secrets)?;
        let ciphertext = cipher
            .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
            .map_err(|e| GlossError::Other(format!("Failed to encrypt local secret store: {e}")))?;

        let payload = EncryptedSecrets {
            nonce_b64: base64::engine::general_purpose::STANDARD.encode(nonce),
            ciphertext_b64: base64::engine::general_purpose::STANDARD.encode(ciphertext),
        };

        let tmp_path = self.dir.join(format!("{SECRET_DATA_FILENAME}.tmp"));
        write_owner_only_file(&tmp_path, &serde_json::to_vec(&payload)?)?;
        fs::rename(tmp_path, self.data_path())?;
        set_owner_only_permissions(&self.data_path())?;
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

        let mut key = [0u8; 32];
        aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut key);

        let mut file = open_owner_only_create_new(&key_path)?;
        file.write_all(&key)?;
        file.sync_all()?;
        set_owner_only_permissions(&key_path)?;

        Ok(key)
    }

    fn key_path(&self) -> PathBuf {
        self.dir.join(SECRET_KEY_FILENAME)
    }

    fn data_path(&self) -> PathBuf {
        self.dir.join(SECRET_DATA_FILENAME)
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

fn write_owner_only_file(path: &Path, bytes: &[u8]) -> Result<(), GlossError> {
    let mut file = open_owner_only_truncate(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    set_owner_only_permissions(path)?;
    Ok(())
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

#[cfg(unix)]
fn open_owner_only_truncate(path: &Path) -> Result<std::fs::File, GlossError> {
    use std::os::unix::fs::OpenOptionsExt;

    Ok(OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?)
}

#[cfg(not(unix))]
fn open_owner_only_truncate(path: &Path) -> Result<std::fs::File, GlossError> {
    Ok(OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?)
}

#[cfg(test)]
mod tests {
    use super::SecretStore;
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
