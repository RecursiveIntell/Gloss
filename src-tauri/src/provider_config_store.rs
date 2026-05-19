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
        fs::write(&tmp_path, serde_json::to_vec(&payload)?)?;
        fs::rename(tmp_path, self.data_path())?;
        Ok(())
    }

    fn load_or_create_key(&self) -> Result<[u8; 32], GlossError> {
        let key_path = self.key_path();
        if key_path.exists() {
            let bytes = fs::read(&key_path)?;
            return bytes
                .try_into()
                .map_err(|_| GlossError::Other("Secret-store key has invalid length".into()));
        }

        let mut key = [0u8; 32];
        aes_gcm::aead::rand_core::RngCore::fill_bytes(&mut OsRng, &mut key);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&key_path)?;
        file.write_all(&key)?;
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
fn set_owner_only_permissions(path: &Path) -> Result<(), GlossError> {
    use std::os::unix::fs::PermissionsExt;

    let permissions = fs::Permissions::from_mode(0o600);
    fs::set_permissions(path, permissions)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &Path) -> Result<(), GlossError> {
    Ok(())
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
}
