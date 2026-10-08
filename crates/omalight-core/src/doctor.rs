//! `omalight doctor` checks the pieces a first run depends on and prints a fix when one is missing.

use crate::client::Client;
use crate::config::{self, file_mode};
use crate::identity::{self, Store};
use omalight_protocol::ThemeWatch;
use std::net::ToSocketAddrs;
use std::path::Path;
use std::time::Duration;

pub struct Report {
    pub text: String,
    pub ok: bool,
}

pub fn report(dir: &Path, connect: Option<&str>) -> Report {
    let mut lines = Vec::new();
    let mut ok = true;

    lines.push(format!("config: {}", dir.display()));
    match config::ensure_dir(dir) {
        Ok(()) => {}
        Err(err) => {
            ok = false;
            lines.push(format!("fix: the config directory is not writable ({err})"));
        }
    }

    let secret_path = dir.join("secret");
    if secret_path.is_file() {
        match file_mode(&secret_path) {
            Some(0o600) => lines.push("secret: present, mode 0600".into()),
            Some(mode) => {
                ok = false;
                lines.push(format!("secret: present, mode {mode:o}"));
                lines.push(format!("fix: chmod 600 {}", secret_path.display()));
            }
            None => lines.push("secret: present".into()),
        }
        match Store::open(dir) {
            Ok(mut store) => match store.devices() {
                Ok(devices) if devices.is_empty() => lines.push("devices: none paired yet".into()),
                Ok(devices) => {
                    let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
                    lines.push(format!(
                        "devices: {} paired ({})",
                        devices.len(),
                        names.join(", ")
                    ));
                }
                Err(err) => {
                    ok = false;
                    lines.push(format!("devices: {err}"));
                    lines.push(
                        "fix: remove devices.toml in the config directory and pair again".into(),
                    );
                }
            },
            Err(err) => {
                ok = false;
                lines.push(format!("secret: {err}"));
            }
        }
    } else {
        lines.push(
            "secret: not on this machine. omalight-engine creates it the first time it starts."
                .into(),
        );
    }

    match identity::load_paired(dir) {
        Ok(engines) if engines.is_empty() => {
            lines.push("paired: this machine has not saved an engine yet".into())
        }
        Ok(engines) => {
            let hosts: Vec<_> = engines
                .iter()
                .map(|e| format!("{} ({})", e.host, e.name))
                .collect();
            lines.push(format!("paired: {}", hosts.join(", ")));
        }
        Err(err) => {
            ok = false;
            lines.push(format!("paired: {err}"));
            lines.push("fix: delete paired.toml and run `omalight pair HOST`".into());
        }
    }

    let theme = ThemeWatch::new();
    if theme.theme.source == "built-in" {
        lines.push("theme: built-in dark. No Omarchy colors.toml was found.".into());
    } else {
        lines.push(format!(
            "theme: {} ({})",
            theme.theme.name, theme.theme.source
        ));
    }

    lines.push(gpu_line());

    if let Some(host) = connect {
        let host = config::with_port(host);
        match probe(&host, dir) {
            Ok(detail) => lines.push(format!("connect {host}: {detail}")),
            Err(err) => {
                ok = false;
                lines.push(format!("connect {host}: {err}"));
                lines.push(
                    "fix: start omalight-engine on that machine and allow TCP port 7421, or run `omalight pair HOST`"
                        .into(),
                );
            }
        }
    }

    Report {
        text: lines.join("\n") + "\n",
        ok,
    }
}

fn gpu_line() -> String {
    match std::env::var("LIGHTCRAFT_GPU") {
        Ok(value) if matches!(value.trim(), "0" | "off" | "false" | "no") => {
            "gpu: LIGHTCRAFT_GPU is off, so rendering stays on the CPU. Unset it for Metal on macOS or Vulkan on Linux."
                .into()
        }
        Ok(value) => format!("gpu: LIGHTCRAFT_GPU={value}"),
        Err(_) if cfg!(target_os = "macos") => {
            "gpu: LIGHTCRAFT_GPU is unset. On macOS the engine uses Metal through wgpu.".into()
        }
        Err(_) => "gpu: LIGHTCRAFT_GPU is unset. On Linux the engine uses Vulkan when a GPU is present.".into(),
    }
}

fn probe(host: &str, dir: &Path) -> Result<String, String> {
    let mut addrs = host.to_socket_addrs().map_err(|e| format!("{e}"))?;
    if addrs.next().is_none() {
        return Err("that name did not resolve".into());
    }
    let token = identity::paired_token(dir, host);
    let mut client = if let Some(token) = token.as_deref() {
        Client::connect(host, token, Duration::from_secs(3))?
    } else if is_loopback(host) {
        match Client::connect_local(host, Duration::from_secs(3)) {
            Ok(client) => client,
            Err(_) => {
                let secret = identity::read_secret(&dir.join("secret"))
                    .map_err(|_| "not paired, and this machine has no engine secret".to_string())?;
                Client::connect(host, &secret, Duration::from_secs(3))?
            }
        }
    } else {
        return Err("not paired".into());
    };
    let ping = client.ping()?;
    Ok(format!(
        "ok ({}, ping {} µs)",
        client.engine(),
        ping.as_micros()
    ))
}

fn is_loopback(host: &str) -> bool {
    let name = host.rsplit_once(':').map(|(h, _)| h).unwrap_or(host);
    let name = name.trim_matches(|c| c == '[' || c == ']');
    name == "localhost" || name == "127.0.0.1" || name == "::1"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_names_a_missing_secret_and_the_gpu_switch() {
        let dir = std::env::temp_dir().join(format!("omalight-doctor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let report = report(&dir, None);
        assert!(report.ok, "{}", report.text);
        assert!(report.text.contains("secret: not on this machine"));
        assert!(report.text.contains("gpu:"));
        assert!(report.text.contains("theme:"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
