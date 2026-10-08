//! `omalight pair`, `omalight doctor`, and `omalight-engine install-service`.

use crate::client;
use crate::config::{self, hostname, ssh_host, with_port};
use crate::doctor;
use crate::identity::{self, Store};
use crate::pair::parse_ssh_credential;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

pub fn pair_cli(args: &[String]) -> Result<(), String> {
    let mut connect = None;
    let mut name = None;
    let mut config = None;
    let mut yes = false;
    let mut code_only = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" => {
                i += 1;
                connect = Some(args.get(i).cloned().ok_or("--connect needs a host")?);
            }
            "--name" => {
                i += 1;
                name = Some(args.get(i).cloned().ok_or("--name needs a value")?);
            }
            "--config-dir" => {
                i += 1;
                config = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--config-dir needs a path")?,
                ));
            }
            "--yes" => yes = true,
            "--code-only" => code_only = true,
            "--help" | "-h" => {
                eprintln!(
                    "\
omalight pair HOST [--name NAME] [--yes] [--code-only] [--config-dir DIR]

Asks the engine for a device credential. If ssh to that host works, the credential comes
back over ssh. Otherwise both sides show a six-digit code and you confirm they match."
                );
                return Ok(());
            }
            other if other.starts_with('-') => return Err(format!("unknown argument `{other}`")),
            other if connect.is_none() => connect = Some(other.to_string()),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    let host = with_port(&connect.ok_or("omalight pair needs a host")?);
    let name = name.unwrap_or_else(hostname);
    let dir = config.unwrap_or_else(config::config_dir);
    let token = if !code_only {
        match fetch_over_ssh(&host, &name) {
            Ok(token) => {
                identity::save_paired(&dir, &host, &name, &token)?;
                println!("paired {name} at {host} over ssh");
                return Ok(());
            }
            Err(err) => {
                eprintln!("ssh: {err}");
                eprintln!("falling back to the pairing code");
                None
            }
        }
    } else {
        None
    };
    let token = match token {
        Some(token) => token,
        None => client::pair(&host, &name, Duration::from_secs(8), |code| {
            println!("code {code}");
            if yes {
                return true;
            }
            eprint!("Does this match the code on the engine? [yes/no] ");
            let _ = std::io::Write::flush(&mut std::io::stderr());
            let mut line = String::new();
            let _ = std::io::stdin().read_line(&mut line);
            matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes")
        })?,
    };
    identity::save_paired(&dir, &host, &name, &token)?;
    println!("paired {name} at {host}");
    Ok(())
}

pub fn fetch_over_ssh(host: &str, name: &str) -> Result<String, String> {
    let name = identity::clean_name(name)?;
    let remote = ssh_host(host);
    if remote.is_empty() {
        return Err("missing ssh host".into());
    }
    let command =
        format!("export PATH=\"$HOME/.local/bin:$PATH\"; omalight-engine pair-ssh {name}");
    let output = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            &remote,
            &command,
        ])
        .output()
        .map_err(|e| format!("{e}"))?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        let err = err.trim();
        if err.is_empty() {
            return Err(format!("ssh exited {}", output.status));
        }
        return Err(err.to_string());
    }
    parse_ssh_credential(&String::from_utf8_lossy(&output.stdout))
}

pub fn doctor_cli(args: &[String]) -> Result<(), String> {
    let mut connect = None;
    let mut config = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" => {
                i += 1;
                connect = Some(args.get(i).cloned().ok_or("--connect needs a host")?);
            }
            "--config-dir" => {
                i += 1;
                config = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--config-dir needs a path")?,
                ));
            }
            "--help" | "-h" => {
                eprintln!("omalight doctor [--connect HOST] [--config-dir DIR]");
                return Ok(());
            }
            other if other.starts_with('-') => return Err(format!("unknown argument `{other}`")),
            other if connect.is_none() => connect = Some(other.to_string()),
            other => return Err(format!("unexpected argument `{other}`")),
        }
        i += 1;
    }
    let dir = config.unwrap_or_else(config::config_dir);
    let report = doctor::report(&dir, connect.as_deref());
    print!("{}", report.text);
    if report.ok {
        Ok(())
    } else {
        Err("checks failed".into())
    }
}

pub fn install_service() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    {
        install_launchd(&exe)
    }
    #[cfg(not(target_os = "macos"))]
    {
        install_systemd(&exe)
    }
}

#[cfg(target_os = "macos")]
fn install_launchd(exe: &Path) -> Result<(), String> {
    let dir = config::home().join("Library/LaunchAgents");
    config::ensure_dir(&dir)?;
    let path = dir.join("com.omalight.engine.plist");
    let body = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>com.omalight.engine</string>
  <key>ProgramArguments</key>
  <array>
    <string>{}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>KeepAlive</key>
  <true/>
</dict>
</plist>
"#,
        xml_escape(&exe.display().to_string())
    );
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let status = Command::new("launchctl")
        .args(["bootstrap", &domain, path.to_str().unwrap_or("")])
        .status();
    match status {
        Ok(status) if status.success() => println!("launchd started com.omalight.engine"),
        Ok(status) => {
            let _ = Command::new("launchctl")
                .args(["bootout", &domain, path.to_str().unwrap_or("")])
                .status();
            let again = Command::new("launchctl")
                .args(["bootstrap", &domain, path.to_str().unwrap_or("")])
                .status();
            if again.as_ref().map(|s| s.success()).unwrap_or(false) {
                println!("launchd started com.omalight.engine");
            } else {
                println!("launchctl bootstrap exited {status}. The plist is in place; load it after logging in again.");
            }
        }
        Err(err) => println!("launchctl is not available ({err}). The plist is in place."),
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_systemd(exe: &Path) -> Result<(), String> {
    let dir = config::home().join(".config/systemd/user");
    config::ensure_dir(&dir)?;
    let path = dir.join("omalight-engine.service");
    let body = format!(
        "[Unit]\n\
Description=Omalight LightCraft engine\n\
After=network-online.target\n\
\n\
[Service]\n\
ExecStart={}\n\
Restart=on-failure\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        exe.display()
    );
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    println!("wrote {}", path.display());
    match Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .status()
    {
        Ok(status) if status.success() => {
            match Command::new("systemctl").args(["--user", "enable", "--now", "omalight-engine.service"]).status() {
                Ok(status) if status.success() => println!("enabled omalight-engine.service"),
                Ok(status) => println!(
                    "systemctl enable exited {status}. The unit file is in place. Start it with: systemctl --user enable --now omalight-engine.service"
                ),
                Err(err) => println!("systemctl is not available ({err}). The unit file is in place."),
            }
        }
        Ok(status) => {
            println!("systemctl daemon-reload exited {status}. The unit file is in place.")
        }
        Err(err) => println!("systemctl is not available ({err}). The unit file is in place."),
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub fn list_devices(dir: &Path) -> Result<(), String> {
    let mut store = Store::open(dir)?;
    let devices = store.devices()?;
    if devices.is_empty() {
        println!("no paired devices");
        return Ok(());
    }
    for device in devices {
        println!("{}  {}", device.name, device.created);
    }
    Ok(())
}

pub fn revoke_device(dir: &Path, name: &str) -> Result<(), String> {
    let mut store = Store::open(dir)?;
    store.revoke(name)?;
    println!("revoked {name}");
    Ok(())
}

pub fn pair_ssh(dir: &Path, name: &str) -> Result<(), String> {
    let mut store = Store::open(dir)?;
    let token = store.issue(name)?;
    println!("credential {token}");
    Ok(())
}
