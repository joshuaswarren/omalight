//! `omalight --headless` drives a running engine and prints one JSON object.

use omalight_core::client::Client;
use omalight_core::config;
use omalight_core::identity;
use omalight_protocol::{ExportReq, PreviewReq};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub struct HeadlessArgs {
    pub connect: String,
    pub token: Option<String>,
    pub config_dir: PathBuf,
    pub open: PathBuf,
    pub sets: BTreeMap<String, f64>,
    pub export: Option<PathBuf>,
    pub long_edge: u32,
    pub latency_json: Option<PathBuf>,
}

pub fn run(args: &HeadlessArgs) -> Result<(), String> {
    let mut client = connect(&args.connect, args.token.as_deref(), &args.config_dir)?;
    let ping = client.ping()?;
    let opened_at = Instant::now();
    let opened = client.open(args.open.to_str().ok_or("path is not utf-8")?)?;
    let open_us = opened_at.elapsed().as_micros() as u64;

    let mut values = args.sets.clone();
    if values.is_empty() {
        values.insert("light.exposure".into(), opened.exposure);
        values.insert("wb.temp".into(), opened.temp);
    }
    let round = client.preview(&PreviewReq {
        photo: opened.photo,
        generation: 1,
        viewport_w: 1280,
        viewport_h: 800,
        values,
        begin: true,
        end: true,
        commit: true,
    })?;
    let draft = round
        .frames
        .iter()
        .find(|f| f.meta.stage == "draft")
        .ok_or("engine sent no draft preview")?;
    let full = round
        .frames
        .iter()
        .find(|f| f.meta.stage == "full")
        .ok_or("engine sent no full preview")?;

    let mut export_us = 0;
    let mut export_server_us = 0;
    let mut export_path = String::new();
    if let Some(dest) = &args.export {
        let started = Instant::now();
        let exported = client.export(&ExportReq {
            photo: opened.photo,
            dest: dest.display().to_string(),
            long_edge: args.long_edge,
            quality: 90,
            format: "jpeg".into(),
        })?;
        export_us = started.elapsed().as_micros() as u64;
        export_server_us = exported.server_us;
        export_path = exported.path.clone();
        omalight_core::notify::notify(
            "Omalight export",
            &format!("{} ({} bytes)", exported.path, exported.bytes),
        );
    }

    let report = json!({
        "ping_us": ping.as_micros() as u64,
        "open_us": open_us,
        "draft_us": draft.meta.server_us,
        "full_us": full.meta.server_us,
        "preview_rtt_us": round.rtt.as_micros() as u64,
        "export_us": export_us,
        "export_server_us": export_server_us,
        "export_path": export_path,
        "photo": opened.photo,
        "width": opened.width,
        "height": opened.height,
        "draft_px": [draft.meta.width, draft.meta.height],
        "full_px": [full.meta.width, full.meta.height],
    });
    println!("{report}");
    if let Some(path) = &args.latency_json {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(
            path,
            serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn connect(addr: &str, token: Option<&str>, dir: &Path) -> Result<Client, String> {
    let addr = config::with_port(addr);
    if let Some(token) = token {
        if token.len() >= 16 {
            return Client::connect(&addr, token, Duration::from_secs(10));
        }
    }
    if let Some(token) = identity::paired_token(dir, &addr) {
        return Client::connect(&addr, &token, Duration::from_secs(10));
    }
    let host = addr
        .rsplit_once(':')
        .map(|(h, _)| h)
        .unwrap_or(addr.as_str());
    let host = host.trim_matches(|c| c == '[' || c == ']');
    if host == "localhost" || host == "127.0.0.1" || host == "::1" {
        if let Ok(client) = Client::connect_local(&addr, Duration::from_secs(10)) {
            return Ok(client);
        }
        if let Ok(secret) = identity::read_secret(&dir.join("secret")) {
            return Client::connect(&addr, &secret, Duration::from_secs(10));
        }
    }
    Err(format!("not paired with {addr}. Run: omalight pair {addr}"))
}
