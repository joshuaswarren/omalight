//! UI thread ↔ engine thread. Slider values are coalesced here so a drag sends the latest
//! generation, not every intermediate tick.

use omalight_core::client::{spawn_local, Client};
use omalight_core::config::{self, ConnectMode};
use omalight_core::identity;
use omalight_core::jpeg;
use omalight_core::mdns;
use omalight_protocol::{plan_request, Endpoint, ExportReq, PreviewReq, Step};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::net::ToSocketAddrs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::Duration;

pub struct Link {
    pub tx: Sender<ToEngine>,
    pub rx: Receiver<FromEngine>,
    pub pair_tx: Sender<bool>,
    pub pick_tx: Sender<String>,
}

pub struct LinkOpts {
    pub mode: ConnectMode,
    pub remote: Option<String>,
    pub ai: Option<Endpoint>,
    pub config_dir: std::path::PathBuf,
    /// Device credential from `--token`, when a caller already has one.
    pub credential: Option<String>,
}

pub enum ToEngine {
    Open(PathBuf),
    Edit {
        exposure: f64,
        temp: f64,
        begin: bool,
        end: bool,
        commit: bool,
        viewport: [u32; 2],
    },
    Export {
        dest: PathBuf,
        long_edge: u32,
    },
    Command {
        id: String,
        params: Value,
    },
    Plan {
        text: String,
    },
    Shutdown,
}

pub enum FromEngine {
    Status(String),
    Opened {
        photo: u64,
        name: String,
        width: u32,
        height: u32,
        exposure: f64,
        temp: f64,
    },
    Image {
        generation: u64,
        stage: String,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
        server_us: u64,
        rtt_us: u64,
        exposure: f64,
        temp: f64,
    },
    Exported {
        path: String,
        bytes: u64,
    },
    Note(String),
    Failed(String),
    PairCode(String),
    Discovered(Vec<(String, String)>),
}

pub fn start(opts: LinkOpts) -> Link {
    let (to_tx, to_rx) = mpsc::channel();
    let (from_tx, from_rx) = mpsc::channel();
    let (pair_tx, pair_rx) = mpsc::channel();
    let (pick_tx, pick_rx) = mpsc::channel();
    thread::spawn(move || {
        if let Err(err) = run(opts, to_rx, from_tx.clone(), pair_rx, pick_rx) {
            let _ = from_tx.send(FromEngine::Failed(err));
        }
    });
    Link {
        tx: to_tx,
        rx: from_rx,
        pair_tx,
        pick_tx,
    }
}

fn run(
    opts: LinkOpts,
    rx: Receiver<ToEngine>,
    tx: Sender<FromEngine>,
    pair_rx: Receiver<bool>,
    pick_rx: Receiver<String>,
) -> Result<(), String> {
    let (mut client, _child, which) = connect(&opts, &tx, &pair_rx, &pick_rx)?;
    let _ = tx.send(FromEngine::Status(which));
    let mut photo: Option<u64> = None;
    let mut generation = 1u64;
    while let Ok(msg) = rx.recv() {
        match msg {
            ToEngine::Shutdown => break,
            ToEngine::Open(path) => match client.open(&path.display().to_string()) {
                Ok(opened) => {
                    photo = Some(opened.photo);
                    let _ = tx.send(FromEngine::Opened {
                        photo: opened.photo,
                        name: opened.name,
                        width: opened.width,
                        height: opened.height,
                        exposure: opened.exposure,
                        temp: opened.temp,
                    });
                }
                Err(err) => {
                    let _ = tx.send(FromEngine::Failed(err));
                }
            },
            ToEngine::Edit {
                exposure,
                temp,
                begin,
                end,
                commit,
                viewport,
            } => {
                let Some(id) = photo else { continue };
                generation += 1;
                let gen = generation;
                let mut values = BTreeMap::new();
                values.insert("light.exposure".into(), exposure);
                values.insert("wb.temp".into(), temp);
                match client.preview(&PreviewReq {
                    photo: id,
                    generation: gen,
                    viewport_w: viewport[0],
                    viewport_h: viewport[1],
                    values,
                    begin,
                    end,
                    commit,
                }) {
                    Ok(round) => {
                        for frame in round.frames {
                            if frame.jpeg.is_empty() {
                                continue;
                            }
                            match jpeg::decode(&frame.jpeg) {
                                Ok((w, h, rgba)) => {
                                    let _ = tx.send(FromEngine::Image {
                                        generation: gen,
                                        stage: frame.meta.stage,
                                        width: w,
                                        height: h,
                                        rgba,
                                        server_us: frame.meta.server_us,
                                        rtt_us: round.rtt.as_micros() as u64,
                                        exposure,
                                        temp,
                                    });
                                }
                                Err(err) => {
                                    let _ = tx.send(FromEngine::Failed(err));
                                }
                            }
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(FromEngine::Failed(err));
                    }
                }
            }
            ToEngine::Export { dest, long_edge } => {
                let Some(id) = photo else { continue };
                match client.export(&ExportReq {
                    photo: id,
                    dest: dest.display().to_string(),
                    long_edge,
                    quality: 90,
                    format: "jpeg".into(),
                }) {
                    Ok(exported) => {
                        omalight_core::notify::notify(
                            "Omalight export",
                            &format!("{} ({} bytes)", exported.path, exported.bytes),
                        );
                        let _ = tx.send(FromEngine::Exported {
                            path: exported.path,
                            bytes: exported.bytes,
                        });
                    }
                    Err(err) => {
                        let _ = tx.send(FromEngine::Failed(err));
                    }
                }
            }
            ToEngine::Command { id, params } => match client.command(&id, params) {
                Ok(value) => {
                    let _ = tx.send(FromEngine::Note(value.to_string()));
                }
                Err(err) => {
                    let _ = tx.send(FromEngine::Failed(err));
                }
            },
            ToEngine::Plan { text } => {
                let plan = plan_request(&text, opts.ai.as_ref());
                let _ = tx.send(FromEngine::Note(format!(
                    "{} [{}]",
                    plan.summary, plan.source
                )));
                let Some(id) = photo else { continue };
                let mut exposure = None;
                let mut temp = None;
                for step in &plan.steps {
                    match step {
                        Step::Develop { control, value } if control == "light.exposure" => {
                            exposure = Some(*value)
                        }
                        Step::Develop { control, value } if control == "wb.temp" => {
                            temp = Some(*value)
                        }
                        Step::Develop { control, value } => {
                            let _ = client.command(
                                "develop.set",
                                json!({"control": control, "value": value}),
                            );
                        }
                        Step::Mask { kind } => match client
                            .command("omalight.task", json!({"kind": "mask", "mask": kind}))
                        {
                            Ok(value) => {
                                let _ = tx.send(FromEngine::Note(format!("mask: {value}")));
                            }
                            Err(err) => {
                                let _ = tx.send(FromEngine::Failed(err));
                            }
                        },
                        Step::Export {
                            long_edge,
                            quality,
                            format,
                        } => {
                            let dest =
                                std::env::temp_dir().join(format!("omalight-export-{id}.{format}"));
                            match client.export(&ExportReq {
                                photo: id,
                                dest: dest.display().to_string(),
                                long_edge: *long_edge,
                                quality: *quality,
                                format: format.clone(),
                            }) {
                                Ok(exported) => {
                                    omalight_core::notify::notify(
                                        "Omalight export",
                                        &exported.path,
                                    );
                                    let _ = tx.send(FromEngine::Exported {
                                        path: exported.path,
                                        bytes: exported.bytes,
                                    });
                                }
                                Err(err) => {
                                    let _ = tx.send(FromEngine::Failed(err));
                                }
                            }
                        }
                        Step::Undo => {
                            let _ = client.command("edit.undo", json!({}));
                        }
                        Step::Command {
                            id: command,
                            params,
                        } => {
                            let _ = client.command(command, params.clone());
                        }
                    }
                }
                if exposure.is_some() || temp.is_some() {
                    // The UI still owns the slider positions. Report the plan's numbers by a note
                    // the window parses: "sliders <exposure> <temp>".
                    let _ = tx.send(FromEngine::Note(format!(
                        "sliders {} {}",
                        exposure
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "-".into()),
                        temp.map(|v| v.to_string()).unwrap_or_else(|| "-".into())
                    )));
                    let _ = id;
                }
            }
        }
    }
    Ok(())
}

fn connect(
    opts: &LinkOpts,
    tx: &Sender<FromEngine>,
    pair_rx: &Receiver<bool>,
    pick_rx: &Receiver<String>,
) -> Result<(Client, Option<omalight_core::client::ChildGuard>, String), String> {
    match opts.mode {
        ConnectMode::Remote => {
            let addr = config::with_port(opts.remote.as_deref().ok_or("remote mode needs a host")?);
            let client = connect_remote(opts, &addr, tx, pair_rx)?;
            Ok((client, None, format!("remote {addr}")))
        }
        ConnectMode::Local => {
            let (client, child, addr) = spawn_local(&engine_bin())?;
            Ok((client, Some(child), format!("local {addr}")))
        }
        ConnectMode::Auto => connect_auto(opts, tx, pair_rx, pick_rx),
    }
}

fn connect_auto(
    opts: &LinkOpts,
    tx: &Sender<FromEngine>,
    pair_rx: &Receiver<bool>,
    pick_rx: &Receiver<String>,
) -> Result<(Client, Option<omalight_core::client::ChildGuard>, String), String> {
    if let Some(addr) = opts.remote.as_deref() {
        let addr = config::with_port(addr);
        if let Ok(client) = connect_saved(opts, &addr, Duration::from_millis(800)) {
            return Ok((client, None, format!("remote {addr}")));
        }
        if tcp_open(&addr) {
            let client = connect_remote(opts, &addr, tx, pair_rx)?;
            return Ok((client, None, format!("remote {addr}")));
        }
    }

    let found = mdns::browse(Duration::from_millis(700)).unwrap_or_default();
    for engine in &found {
        let addr = config::with_port(&engine.host);
        if let Ok(client) = connect_saved(opts, &addr, Duration::from_millis(800)) {
            return Ok((client, None, format!("remote {addr}")));
        }
    }
    if found.is_empty() {
        if let Ok(saved) = identity::load_paired(&opts.config_dir) {
            for engine in saved {
                if let Ok(client) =
                    Client::connect(&engine.host, &engine.token, Duration::from_millis(800))
                {
                    return Ok((client, None, format!("remote {}", engine.host)));
                }
            }
        }
        let (client, child, addr) = spawn_local(&engine_bin())?;
        return Ok((client, Some(child), format!("local {addr}")));
    }

    let choices: Vec<(String, String)> = found
        .iter()
        .map(|engine| {
            (
                format!("{} ({})", engine.name, engine.host),
                config::with_port(&engine.host),
            )
        })
        .collect();
    let _ = tx.send(FromEngine::Discovered(choices));
    let pick = pick_rx
        .recv()
        .map_err(|_| "closed before an engine was picked".to_string())?;
    if pick == "local" {
        let (client, child, addr) = spawn_local(&engine_bin())?;
        return Ok((client, Some(child), format!("local {addr}")));
    }
    let client = connect_remote(opts, &pick, tx, pair_rx)?;
    Ok((client, None, format!("remote {pick}")))
}

fn connect_remote(
    opts: &LinkOpts,
    addr: &str,
    tx: &Sender<FromEngine>,
    pair_rx: &Receiver<bool>,
) -> Result<Client, String> {
    if let Ok(client) = connect_saved(opts, addr, Duration::from_secs(3)) {
        return Ok(client);
    }
    let name = identity::clean_name(&config::hostname()).unwrap_or_else(|_| "omalight".into());
    let token = omalight_core::client::pair(addr, &name, Duration::from_secs(8), |code| {
        let _ = tx.send(FromEngine::PairCode(code.to_string()));
        pair_rx.recv().unwrap_or(false)
    })?;
    identity::save_paired(&opts.config_dir, addr, &name, &token)?;
    Client::connect(addr, &token, Duration::from_secs(5))
}

fn connect_saved(opts: &LinkOpts, addr: &str, timeout: Duration) -> Result<Client, String> {
    if let Some(token) = opts.credential.as_deref() {
        if token.len() >= 16 {
            return Client::connect(addr, token, timeout);
        }
    }
    let token = identity::paired_token(&opts.config_dir, addr)
        .ok_or_else(|| format!("not paired with {addr}"))?;
    Client::connect(addr, &token, timeout)
}

fn tcp_open(addr: &str) -> bool {
    let Ok(mut targets) = addr.to_socket_addrs() else {
        return false;
    };
    let Some(target) = targets.next() else {
        return false;
    };
    std::net::TcpStream::connect_timeout(&target, Duration::from_millis(400)).is_ok()
}

fn engine_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omalight-engine"));
    path.pop();
    path.push("omalight-engine");
    if !path.exists() {
        PathBuf::from("omalight-engine")
    } else {
        path
    }
}

pub fn engine_path() -> &'static Path {
    Path::new("omalight-engine")
}
