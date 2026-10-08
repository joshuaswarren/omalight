use crate::config::config_dir;
use crate::mdns;
use crate::server::{engine_name, Server};
use crate::setup;
use std::fs;
use std::io::Write;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    run_args(&args)
}

pub fn run_args(args: &[String]) -> Result<(), String> {
    let mut bind = "0.0.0.0:7421".to_string();
    let mut config: Option<PathBuf> = None;
    let mut ready_file: Option<PathBuf> = None;
    let mut no_mdns = false;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "--bind" => bind = need(args, &mut i, "--bind")?,
            "--config-dir" => config = Some(PathBuf::from(need(args, &mut i, "--config-dir")?)),
            "--ready-file" => ready_file = Some(PathBuf::from(need(args, &mut i, "--ready-file")?)),
            "--no-mdns" => no_mdns = true,
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            "--auth" | "--token" | "--token-file" => {
                return Err("the engine creates its own secret. Pair a device instead of passing --auth or --token.".into());
            }
            other if other.starts_with('-') => return Err(format!("unknown argument `{other}`")),
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    let dir = config.unwrap_or_else(config_dir);
    match positional.first().map(String::as_str) {
        Some("devices") => return setup::list_devices(&dir),
        Some("revoke") => {
            let name = positional.get(1).ok_or("revoke needs a device name")?;
            return setup::revoke_device(&dir, name);
        }
        Some("install-service") => return setup::install_service(),
        Some("pair-ssh") => {
            let name = positional.get(1).ok_or("pair-ssh needs a device name")?;
            return setup::pair_ssh(&dir, name);
        }
        Some(other) => return Err(format!("unknown command `{other}`")),
        None => {}
    }

    let listener = TcpListener::bind(&bind).map_err(|e| format!("bind {bind}: {e}"))?;
    let addr = listener.local_addr().map_err(|e| e.to_string())?;
    if let Some(path) = &ready_file {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
        writeln!(file, "{addr}").map_err(|e| e.to_string())?;
    }
    let server = Server::open(&dir)?;
    let advertise = !no_mdns && !addr.ip().is_loopback();
    let _mdns = if advertise {
        match mdns::advertise(&crate::config::hostname(), addr.port()) {
            Ok(advertiser) => Some(advertiser),
            Err(err) => {
                eprintln!("mdns: {err}");
                None
            }
        }
    } else {
        None
    };
    eprintln!(
        "omalight-engine listening on {addr} engine={}",
        engine_name()
    );
    let server = Arc::new(server);
    server.serve(listener);
    Ok(())
}

fn need(args: &[String], i: &mut usize, flag: &str) -> Result<String, String> {
    *i += 1;
    args.get(*i)
        .cloned()
        .ok_or_else(|| format!("{flag} needs a value"))
}

fn print_help() {
    eprintln!(
        "\
omalight-engine — LightCraft develop session for Omalight

  omalight-engine [--bind ADDR] [--config-dir DIR] [--ready-file PATH] [--no-mdns]
  omalight-engine devices
  omalight-engine revoke NAME
  omalight-engine install-service
  omalight-engine pair-ssh NAME

The first start writes a secret into the config directory (mode 0600) and listens on
port 7421. Devices pair with a six-digit code. The secret is not a network password.

Set LIGHTCRAFT_GPU=0 to stay on the CPU pipeline. Leave it unset for Metal or Vulkan."
    );
}
