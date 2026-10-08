//! Engine secret and per-device credentials.
//!
//! The secret is created on the first engine start and stored mode 0600. It authenticates a
//! loopback connection. It is never accepted from any other address, and it is never sent to a
//! client. Each paired device gets its own token in `devices.toml`.

use crate::config::{self, ensure_dir, write_private};
use omalight_protocol::tokens_eq;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Device {
    pub name: String,
    pub token: String,
    pub created: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct DevicesFile {
    #[serde(default)]
    device: Vec<Device>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairedEngine {
    pub host: String,
    pub name: String,
    pub token: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct PairedFile {
    #[serde(default)]
    engine: Vec<PairedEngine>,
}

pub struct Store {
    dir: PathBuf,
    secret: String,
    devices: Vec<Device>,
}

impl Store {
    pub fn open(dir: &Path) -> Result<Self, String> {
        ensure_dir(dir)?;
        let secret = load_or_create_secret(&dir.join("secret"))?;
        let mut store = Self {
            dir: dir.to_path_buf(),
            secret,
            devices: Vec::new(),
        };
        store.reload()?;
        Ok(store)
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn secret(&self) -> &str {
        &self.secret
    }

    pub fn devices(&mut self) -> Result<Vec<Device>, String> {
        self.reload()?;
        Ok(self.devices.clone())
    }

    /// True when `presented` is a device credential, or the engine secret on loopback.
    ///
    /// Every stored credential is compared. A mismatch does not return early.
    pub fn accepts(&mut self, presented: &str, loopback: bool) -> bool {
        let _ = self.reload();
        if presented.len() < 16 {
            return false;
        }
        let secret_match = tokens_eq(presented, &self.secret);
        let mut device_match = false;
        for device in &self.devices {
            if tokens_eq(presented, &device.token) {
                device_match = true;
            }
        }
        device_match || (loopback && secret_match)
    }

    /// Replace any credential with this name. The new token is 32 random bytes, hex encoded.
    pub fn issue(&mut self, name: &str) -> Result<String, String> {
        let name = clean_name(name)?;
        self.reload()?;
        let token = random_hex(32);
        let created = now_stamp();
        self.devices.retain(|device| device.name != name);
        self.devices.push(Device {
            name,
            token: token.clone(),
            created,
        });
        self.save()?;
        Ok(token)
    }

    pub fn revoke(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("revoke needs a device name".into());
        }
        self.reload()?;
        let before = self.devices.len();
        self.devices.retain(|device| device.name != name);
        if self.devices.len() == before {
            return Err(format!("no device named {name}"));
        }
        self.save()?;
        Ok(())
    }

    fn reload(&mut self) -> Result<(), String> {
        let path = self.dir.join("devices.toml");
        if !path.exists() {
            self.devices.clear();
            return Ok(());
        }
        let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
        let file: DevicesFile = toml::from_str(&text).map_err(|e| e.to_string())?;
        self.devices = file.device;
        Ok(())
    }

    fn save(&self) -> Result<(), String> {
        let file = DevicesFile {
            device: self.devices.clone(),
        };
        let text = toml::to_string_pretty(&file).map_err(|e| e.to_string())?;
        write_private(&self.dir.join("devices.toml"), text.trim_end())
    }
}

pub fn load_or_create_secret(path: &Path) -> Result<String, String> {
    if path.exists() {
        let secret = fs::read_to_string(path).map_err(|e| e.to_string())?;
        let secret = secret.trim().to_string();
        if secret.len() < 16 {
            return Err(format!("{} is shorter than 16 characters", path.display()));
        }
        return Ok(secret);
    }
    let secret = random_hex(32);
    write_private(path, &secret)?;
    Ok(secret)
}

pub fn read_secret(path: &Path) -> Result<String, String> {
    let secret = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let secret = secret.trim().to_string();
    if secret.len() < 16 {
        return Err("secret is shorter than 16 characters".into());
    }
    Ok(secret)
}

pub fn random_hex(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn clean_name(name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 64 {
        return Err("device name must be 1 to 64 characters".into());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        return Err("device name may contain letters, digits, '.', '_' and '-'".into());
    }
    Ok(name.to_string())
}

fn now_stamp() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".into())
}

pub fn save_paired(dir: &Path, host: &str, name: &str, token: &str) -> Result<(), String> {
    ensure_dir(dir)?;
    let host = config::with_port(host);
    let mut engines = load_paired(dir)?;
    engines.retain(|engine| engine.host != host);
    engines.push(PairedEngine {
        host,
        name: name.to_string(),
        token: token.to_string(),
    });
    let file = PairedFile { engine: engines };
    let text = toml::to_string_pretty(&file).map_err(|e| e.to_string())?;
    write_private(&dir.join("paired.toml"), text.trim_end())
}

pub fn load_paired(dir: &Path) -> Result<Vec<PairedEngine>, String> {
    let path = dir.join("paired.toml");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let text = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let file: PairedFile = toml::from_str(&text).map_err(|e| e.to_string())?;
    Ok(file.engine)
}

pub fn paired_token(dir: &Path, host: &str) -> Option<String> {
    let host = config::with_port(host);
    load_paired(dir)
        .ok()?
        .into_iter()
        .find(|engine| engine.host == host)
        .map(|engine| engine.token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "omalight-id-{}-{}",
            std::process::id(),
            random_hex(4)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn secret_is_created_once_and_private() {
        let dir = scratch();
        let store = Store::open(&dir).unwrap();
        let secret = store.secret().to_string();
        assert_eq!(secret.len(), 64);
        let again = Store::open(&dir).unwrap();
        assert_eq!(again.secret(), secret);
        #[cfg(unix)]
        {
            assert_eq!(crate::config::file_mode(&dir.join("secret")), Some(0o600));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn secret_is_refused_off_loopback_and_revoke_drops_a_device() {
        let dir = scratch();
        let mut store = Store::open(&dir).unwrap();
        let secret = store.secret().to_string();
        assert!(store.accepts(&secret, true));
        assert!(!store.accepts(&secret, false));
        assert!(!store.accepts("not-a-real-credential", true));
        assert!(!store.accepts("", true));

        let token = store.issue("laptop").unwrap();
        assert!(store.accepts(&token, false));
        assert!(store.accepts(&token, true));
        store.revoke("laptop").unwrap();
        assert!(!store.accepts(&token, false));
        assert!(store.revoke("laptop").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
