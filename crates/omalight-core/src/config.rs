use omalight_protocol::Endpoint;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConnectMode {
    Auto,
    Local,
    Remote,
}

impl ConnectMode {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim() {
            "auto" => Ok(Self::Auto),
            "local" => Ok(Self::Local),
            "remote" => Ok(Self::Remote),
            other => Err(format!("unknown mode `{other}` (auto|local|remote)")),
        }
    }
}

/// Config directory for the engine secret, paired devices, and `config.toml`.
///
/// `OMALIGHT_CONFIG_DIR` wins. macOS uses `~/Library/Application Support/omalight`.
/// Everywhere else uses `$XDG_CONFIG_HOME/omalight` or `~/.config/omalight`.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("OMALIGHT_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    {
        home().join("Library/Application Support/omalight")
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".config"))
            .join("omalight")
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `host` or `host:port`. A bare name gets port 7421.
pub fn with_port(host: &str) -> String {
    let host = host.trim();
    if host.is_empty() {
        return "127.0.0.1:7421".into();
    }
    if host.starts_with('[') {
        if host.contains("]:") {
            host.to_string()
        } else {
            format!("{host}:7421")
        }
    } else if host.contains(':') {
        host.to_string()
    } else {
        format!("{host}:7421")
    }
}

/// Host without a port, for SSH.
pub fn ssh_host(host: &str) -> String {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(host).to_string();
    }
    match host.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => name.to_string(),
        _ => host.to_string(),
    }
}

pub fn hostname() -> String {
    let mut buf = [0u8; 256];
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, buf.len()) };
    if rc != 0 {
        return "omalight".into();
    }
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    if name.is_empty() {
        "omalight".into()
    } else {
        name
    }
}

/// Load `config.toml` if it exists. Missing keys stay at the defaults.
pub fn load_file() -> Result<toml::Value, String> {
    load_file_from(&config_path())
}

pub fn load_file_from(path: &Path) -> Result<toml::Value, String> {
    if !path.exists() {
        return Ok(toml::Value::Table(Default::default()));
    }
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.to_string())
}

pub fn toml_str<'a>(value: &'a toml::Value, key: &str) -> Option<&'a str> {
    value.get(key).and_then(toml::Value::as_str)
}

pub fn toml_i64(value: &toml::Value, key: &str) -> Option<i64> {
    value.get(key).and_then(toml::Value::as_integer)
}

pub fn ai_from_toml(value: &toml::Value) -> Option<Endpoint> {
    let ai = value.get("ai")?;
    let base = ai
        .get("base_url")
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if base.is_empty() {
        return None;
    }
    let model = ai
        .get("model")
        .and_then(toml::Value::as_str)
        .unwrap_or("local")
        .to_string();
    let api_key = ai
        .get("api_key_env")
        .and_then(toml::Value::as_str)
        .and_then(|name| std::env::var(name).ok())
        .or_else(|| {
            ai.get("api_key")
                .and_then(toml::Value::as_str)
                .map(str::to_string)
        });
    Some(Endpoint {
        base_url: base,
        model,
        api_key,
    })
}

pub fn ensure_dir(path: &Path) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|e| format!("{}: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn write_private(path: &Path, text: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| e.to_string())?;
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.write_all(b"\n").map_err(|e| e.to_string())?;
        file.sync_all().ok();
    }
    #[cfg(not(unix))]
    {
        fs::write(&tmp, format!("{text}\n")).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

pub fn file_mode(path: &Path) -> Option<u32> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path)
            .ok()
            .map(|meta| meta.permissions().mode() & 0o777)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}
