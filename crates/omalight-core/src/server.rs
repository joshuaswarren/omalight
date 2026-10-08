//! One LightCraft session, many connections, one render at a time.
//!
//! The reader thread answers pings immediately and queues everything else. Preview requests
//! collapse to the newest generation. A full-resolution pass that is no longer the newest
//! generation is not started.

use crate::ai::{outcome_json, parse_task, prepare, HeavyOutcome};
use crate::identity::Store;
use crate::jpeg;
use crate::pair::{self, decode_key, encode_key};
use lightcraft_engine::catalog::PhotoId;
use lightcraft_engine::Session;
use omalight_protocol::{
    kind_name, read_frame, tokens_eq, write_frame, CancelReq, CommandReq, ErrorMsg, ExportReq,
    ExportResult, Frame, Hello, HelloOk, OpenReq, OpenResult, PairConfirm, PairOffer, PairResult,
    PairStart, PreviewMeta, PreviewReq, CANCEL, COMMAND, COMMAND_RESULT, ERROR, EXPORT,
    EXPORT_RESULT, HELLO, HELLO_OK, OPEN, OPEN_RESULT, PAIR_CONFIRM, PAIR_OFFER, PAIR_RESULT,
    PAIR_START, PING, PONG, PREVIEW, PREVIEW_IMAGE,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::net::{IpAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use x25519_dalek::PublicKey;

pub struct Server {
    session: Arc<Mutex<Session>>,
    store: Mutex<Store>,
    engine_name: String,
    pair_ttl: Duration,
    attempts: Mutex<HashMap<IpAddr, Attempts>>,
}

struct Attempts {
    started: Instant,
    starts: u32,
    failures: u32,
}

const PAIR_LIMIT_WINDOW: Duration = Duration::from_secs(120);
const PAIR_MAX_STARTS: u32 = 8;
const PAIR_MAX_FAILURES: u32 = 5;

enum Job {
    Open(u64, OpenReq),
    Preview(u64, PreviewReq),
    Export(u64, ExportReq),
    Command(u64, CommandReq),
}

pub fn engine_name() -> String {
    match std::env::var("LIGHTCRAFT_GPU") {
        Ok(v) if matches!(v.trim(), "0" | "off" | "false" | "no") => "lightcraft-cpu".into(),
        _ => "lightcraft".into(),
    }
}

impl Server {
    pub fn open(dir: &Path) -> Result<Self, String> {
        let store = Store::open(dir)?;
        Ok(Self {
            session: Arc::new(Mutex::new(Session::new().with_fs())),
            store: Mutex::new(store),
            engine_name: engine_name(),
            pair_ttl: Duration::from_secs(120),
            attempts: Mutex::new(HashMap::new()),
        })
    }

    pub fn set_pair_ttl(&mut self, ttl: Duration) {
        self.pair_ttl = ttl;
    }

    pub fn serve(self: &Arc<Self>, listener: TcpListener) {
        for incoming in listener.incoming() {
            match incoming {
                Ok(stream) => {
                    let server = Arc::clone(self);
                    thread::spawn(move || {
                        if let Err(err) = server.connection(stream) {
                            log::info!("connection ended: {err}");
                        }
                    });
                }
                Err(err) => {
                    log::warn!("accept: {err}");
                    break;
                }
            }
        }
    }

    fn connection(&self, mut stream: TcpStream) -> Result<(), String> {
        let _ = stream.set_nodelay(true);
        let first = read_frame(&mut stream).map_err(|e| e.to_string())?;
        match first.kind {
            HELLO => self.session_after_hello(stream, first),
            PAIR_START => self.pair_connection(stream, first),
            _ => {
                let _ = write_frame(
                    &mut stream,
                    &error_frame(first.id, "first frame was not hello"),
                );
                Err("first frame was not hello".into())
            }
        }
    }

    fn session_after_hello(&self, mut stream: TcpStream, hello_frame: Frame) -> Result<(), String> {
        let hello: Hello = hello_frame.parse().map_err(|e| e.to_string())?;
        let peer = stream.peer_addr().map_err(|e| e.to_string())?.ip();
        let auth = match self.authorize(&stream, &hello, peer) {
            Ok(auth) => auth,
            Err(err) => {
                let _ = write_frame(&mut stream, &error_frame(hello_frame.id, &err));
                return Err(err);
            }
        };
        let ok = Frame::json(
            hello_frame.id,
            HELLO_OK,
            &HelloOk {
                version: 1,
                auth: auth.into(),
                engine: self.engine_name.clone(),
            },
        )
        .map_err(|e| e.to_string())?;
        write_frame(&mut stream, &ok).map_err(|e| e.to_string())?;

        let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
        let write = Arc::new(Mutex::new(stream));
        let (tx, rx): (Sender<Job>, Receiver<Job>) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let session = Arc::clone(&self.session);
        let write_worker = Arc::clone(&write);
        let latest_worker = Arc::clone(&latest);
        thread::spawn(move || worker(rx, session, write_worker, latest_worker));

        loop {
            let frame = read_frame(&mut reader).map_err(|e| e.to_string())?;
            match frame.kind {
                PING => send(
                    &write,
                    &Frame {
                        id: frame.id,
                        kind: PONG,
                        payload: b"{}".to_vec(),
                    },
                )?,
                CANCEL => {
                    if let Ok(cancel) = frame.parse::<CancelReq>() {
                        let current = latest.load(Ordering::SeqCst);
                        if cancel.generation >= current {
                            latest.store(0, Ordering::SeqCst);
                        }
                    }
                }
                PREVIEW => match frame.parse::<PreviewReq>() {
                    Ok(req) => {
                        latest.store(req.generation, Ordering::SeqCst);
                        tx.send(Job::Preview(frame.id, req))
                            .map_err(|_| "worker stopped".to_string())?;
                    }
                    Err(err) => send(&write, &error_frame(frame.id, err.to_string()))?,
                },
                OPEN => enqueue(&tx, &write, frame, |id, req: OpenReq| Job::Open(id, req))?,
                EXPORT => enqueue(&tx, &write, frame, |id, req: ExportReq| {
                    Job::Export(id, req)
                })?,
                COMMAND => enqueue(&tx, &write, frame, |id, req: CommandReq| {
                    Job::Command(id, req)
                })?,
                other => send(
                    &write,
                    &error_frame(frame.id, format!("unexpected {}", kind_name(other))),
                )?,
            }
        }
    }
}

fn enqueue<T, F>(
    tx: &Sender<Job>,
    write: &Mutex<TcpStream>,
    frame: Frame,
    build: F,
) -> Result<(), String>
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(u64, T) -> Job,
{
    match frame.parse::<T>() {
        Ok(body) => tx
            .send(build(frame.id, body))
            .map_err(|_| "worker stopped".to_string()),
        Err(err) => send(write, &error_frame(frame.id, err.to_string())),
    }
}

impl Server {
    fn authorize(
        &self,
        stream: &TcpStream,
        hello: &Hello,
        peer: IpAddr,
    ) -> Result<&'static str, String> {
        let loopback = peer.is_loopback();
        if loopback && hello.local && same_user(stream) {
            return Ok("local");
        }
        let mut store = self.store.lock().unwrap_or_else(|err| err.into_inner());
        if store.accepts(&hello.token, loopback) {
            let secret = store.secret().to_string();
            if loopback && tokens_eq(&hello.token, &secret) {
                return Ok("secret");
            }
            return Ok("device");
        }
        Err("authentication failed".into())
    }

    fn allow_pair(&self, ip: IpAddr) -> bool {
        let mut map = self.attempts.lock().unwrap_or_else(|err| err.into_inner());
        let now = Instant::now();
        let entry = map.entry(ip).or_insert(Attempts {
            started: now,
            starts: 0,
            failures: 0,
        });
        if now.duration_since(entry.started) > PAIR_LIMIT_WINDOW {
            *entry = Attempts {
                started: now,
                starts: 0,
                failures: 0,
            };
        }
        if entry.failures >= PAIR_MAX_FAILURES || entry.starts >= PAIR_MAX_STARTS {
            return false;
        }
        entry.starts += 1;
        true
    }

    fn note_failure(&self, ip: IpAddr) {
        let mut map = self.attempts.lock().unwrap_or_else(|err| err.into_inner());
        if let Some(entry) = map.get_mut(&ip) {
            entry.failures += 1;
        }
    }

    fn pair_connection(&self, mut stream: TcpStream, first: Frame) -> Result<(), String> {
        let peer = stream
            .peer_addr()
            .map(|addr| addr.ip())
            .unwrap_or(IpAddr::from([0, 0, 0, 0]));
        if !self.allow_pair(peer) {
            let _ = write_frame(
                &mut stream,
                &error_frame(first.id, "too many pairing attempts"),
            );
            return Err("too many pairing attempts".into());
        }
        let start: PairStart = match first.parse() {
            Ok(start) => start,
            Err(err) => {
                let _ = write_frame(&mut stream, &error_frame(first.id, err.to_string()));
                return Err(err.to_string());
            }
        };
        let name = match crate::identity::clean_name(&start.name) {
            Ok(name) => name,
            Err(err) => {
                let _ = write_frame(&mut stream, &error_frame(first.id, &err));
                return Err(err);
            }
        };
        let client_bytes = match decode_key(&start.client_pub) {
            Ok(bytes) => bytes,
            Err(err) => {
                let _ = write_frame(&mut stream, &error_frame(first.id, &err));
                return Err(err);
            }
        };
        let client_pub = PublicKey::from(client_bytes);
        let (server_secret, server_pub) = pair::server_keypair();
        let offer = Frame::json(
            first.id,
            PAIR_OFFER,
            &PairOffer {
                server_pub: encode_key(server_pub.as_bytes()),
            },
        )
        .map_err(|e| e.to_string())?;
        write_frame(&mut stream, &offer).map_err(|e| e.to_string())?;
        let code = pair::shared_code(server_secret, &server_pub, &client_pub, false);
        eprintln!("omalight pairing {name}: {code}");
        eprintln!(
            "Confirm this code on the other machine. It expires in {} seconds.",
            self.pair_ttl.as_secs().max(1)
        );
        crate::notify::notify("Omalight pairing", &format!("{code}  {name}"));

        stream
            .set_read_timeout(Some(self.pair_ttl))
            .map_err(|e| e.to_string())?;
        let confirm_frame = match read_frame(&mut stream) {
            Ok(frame) => frame,
            Err(err) if err_is_timeout(&err) => {
                let _ = write_frame(
                    &mut stream,
                    &error_frame(first.id, "pairing window expired"),
                );
                return Err("pairing window expired".into());
            }
            Err(err) => return Err(err.to_string()),
        };
        stream.set_read_timeout(None).ok();
        if confirm_frame.kind != PAIR_CONFIRM {
            let _ = write_frame(
                &mut stream,
                &error_frame(confirm_frame.id, "expected pair-confirm"),
            );
            return Err("expected pair-confirm".into());
        }
        let confirm: PairConfirm = confirm_frame.parse().map_err(|e| e.to_string())?;
        if !confirm.accept {
            let _ = write_frame(
                &mut stream,
                &error_frame(confirm_frame.id, "pairing rejected"),
            );
            return Err("pairing rejected".into());
        }
        if !tokens_eq(&confirm.code, &code) {
            self.note_failure(peer);
            let _ = write_frame(
                &mut stream,
                &error_frame(confirm_frame.id, "pairing code did not match"),
            );
            return Err("pairing code did not match".into());
        }
        let credential = {
            let mut store = self.store.lock().unwrap_or_else(|err| err.into_inner());
            store.issue(&name)?
        };
        let result = Frame::json(
            confirm_frame.id,
            PAIR_RESULT,
            &PairResult {
                ok: true,
                credential,
                name,
                message: String::new(),
            },
        )
        .map_err(|e| e.to_string())?;
        write_frame(&mut stream, &result).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn err_is_timeout(err: &omalight_protocol::ProtoError) -> bool {
    match err {
        omalight_protocol::ProtoError::Io(io) => {
            io.kind() == ErrorKind::TimedOut || io.kind() == ErrorKind::WouldBlock
        }
        _ => false,
    }
}

#[cfg(target_os = "linux")]
fn same_user(stream: &TcpStream) -> bool {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut libc::ucred as *mut libc::c_void,
            &mut len,
        )
    };
    rc == 0 && cred.uid == unsafe { libc::getuid() }
}

#[cfg(not(target_os = "linux"))]
fn same_user(_stream: &TcpStream) -> bool {
    false
}

fn error_frame(id: u64, message: impl Into<String>) -> Frame {
    Frame::json(
        id,
        ERROR,
        &ErrorMsg {
            message: message.into(),
        },
    )
    .unwrap_or(Frame {
        id,
        kind: ERROR,
        payload: br#"{"message":"error"}"#.to_vec(),
    })
}

fn send(write: &Mutex<TcpStream>, frame: &Frame) -> Result<(), String> {
    let mut guard = write.lock().unwrap_or_else(|err| err.into_inner());
    write_frame(&mut *guard, frame).map_err(|e| e.to_string())
}

fn send_json(
    write: &Mutex<TcpStream>,
    id: u64,
    kind: u8,
    body: &impl Serialize,
) -> Result<(), String> {
    let frame = Frame::json(id, kind, body).map_err(|e| e.to_string())?;
    send(write, &frame)
}

fn worker(
    rx: Receiver<Job>,
    session: Arc<Mutex<Session>>,
    write: Arc<Mutex<TcpStream>>,
    latest: Arc<AtomicU64>,
) {
    while let Ok(first) = rx.recv() {
        for job in coalesce_jobs(first, &rx) {
            if let Err(err) = run_job(job, &session, &write, &latest) {
                log::warn!("job: {err}");
            }
        }
    }
}

/// Non-preview jobs keep their order. Only the newest preview in the burst is kept, and it runs last.
fn coalesce_jobs(first: Job, rx: &Receiver<Job>) -> Vec<Job> {
    let mut jobs = vec![first];
    while let Ok(next) = rx.try_recv() {
        jobs.push(next);
    }
    let mut others = Vec::new();
    let mut preview: Option<Job> = None;
    for job in jobs {
        match job {
            Job::Preview(_, ref req) => {
                let replace = match &preview {
                    Some(Job::Preview(_, cur)) => req.generation >= cur.generation,
                    _ => true,
                };
                if replace {
                    preview = Some(job);
                }
            }
            other => others.push(other),
        }
    }
    if let Some(preview) = preview {
        others.push(preview);
    }
    others
}

fn run_job(
    job: Job,
    session: &Mutex<Session>,
    write: &Mutex<TcpStream>,
    latest: &AtomicU64,
) -> Result<(), String> {
    match job {
        Job::Open(id, req) => {
            let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
            match open_photo(&mut guard, &req.path) {
                Ok(result) => send_json(write, id, OPEN_RESULT, &result),
                Err(err) => send(write, &error_frame(id, err)),
            }
        }
        Job::Export(id, req) => {
            let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
            match export_one(&mut guard, &req) {
                Ok(result) => send_json(write, id, EXPORT_RESULT, &result),
                Err(err) => send(write, &error_frame(id, err)),
            }
        }
        Job::Command(id, req) => {
            let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
            match run_command(&mut guard, &req) {
                Ok(result) => send_json(write, id, COMMAND_RESULT, &result),
                Err(err) => send(write, &error_frame(id, err)),
            }
        }
        Job::Preview(id, req) => render_preview(session, write, latest, id, req),
    }
}

fn render_preview(
    session: &Mutex<Session>,
    write: &Mutex<TcpStream>,
    latest: &AtomicU64,
    id: u64,
    req: PreviewReq,
) -> Result<(), String> {
    if !current(latest, req.generation) {
        return send_cancelled(write, id, &req);
    }
    {
        let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
        if let Err(err) = apply_edits(&mut guard, &req) {
            return send(write, &error_frame(id, err));
        }
        match render_stage(&mut guard, &req, true) {
            Ok((image, us)) => {
                drop(guard);
                send_image(write, id, &req, "draft", &image, us)?;
            }
            Err(err) => return send(write, &error_frame(id, err)),
        }
    }
    if !current(latest, req.generation) {
        finish_interaction(session, &req);
        return send_cancelled(write, id, &req);
    }
    let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
    let full = render_stage(&mut guard, &req, false);
    if req.commit && req.end {
        let _ = guard.execute("develop.endInteraction", &json!({}));
    }
    drop(guard);
    match full {
        Ok((image, us)) => send_image(write, id, &req, "full", &image, us),
        Err(err) => send(write, &error_frame(id, err)),
    }
}

fn current(latest: &AtomicU64, generation: u64) -> bool {
    latest.load(Ordering::SeqCst) == generation
}

fn finish_interaction(session: &Mutex<Session>, req: &PreviewReq) {
    if req.commit && req.end {
        let mut guard = session.lock().unwrap_or_else(|err| err.into_inner());
        let _ = guard.execute("develop.endInteraction", &json!({}));
    }
}

fn send_cancelled(write: &Mutex<TcpStream>, id: u64, req: &PreviewReq) -> Result<(), String> {
    let meta = PreviewMeta::cancelled(req.photo, req.generation);
    let json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let payload = omalight_protocol::encode_image_payload(&json, &[]).map_err(|e| e.to_string())?;
    send(
        write,
        &Frame {
            id,
            kind: PREVIEW_IMAGE,
            payload,
        },
    )
}

fn send_image(
    write: &Mutex<TcpStream>,
    id: u64,
    req: &PreviewReq,
    stage: &str,
    image: &lightcraft_raster::Rgba8,
    server_us: u64,
) -> Result<(), String> {
    let quality = if stage == "draft" { 55 } else { 82 };
    let jpeg_bytes = jpeg::encode(image, quality)?;
    let meta = PreviewMeta {
        photo: req.photo,
        generation: req.generation,
        stage: stage.into(),
        width: image.width as u32,
        height: image.height as u32,
        server_us,
    };
    log::info!(
        "preview {stage} gen {} {server_us} µs {}x{}",
        req.generation,
        image.width,
        image.height
    );
    let json = serde_json::to_vec(&meta).map_err(|e| e.to_string())?;
    let payload =
        omalight_protocol::encode_image_payload(&json, &jpeg_bytes).map_err(|e| e.to_string())?;
    send(
        write,
        &Frame {
            id,
            kind: PREVIEW_IMAGE,
            payload,
        },
    )
}

fn apply_edits(session: &mut Session, req: &PreviewReq) -> Result<(), String> {
    if !req.commit {
        return Ok(());
    }
    if req.begin || (session.interaction.is_none() && !req.values.is_empty()) {
        session
            .execute("develop.beginInteraction", &json!({"label": "Develop"}))
            .map_err(|e| e.to_string())?;
    }
    if req.values.is_empty() {
        return Ok(());
    }
    let mut values = serde_json::Map::new();
    for (key, value) in &req.values {
        values.insert(key.clone(), json!(value));
    }
    session
        .execute("develop.set", &json!({"values": Value::Object(values)}))
        .map_err(|e| e.to_string())?;
    Ok(())
}

fn render_stage(
    session: &mut Session,
    req: &PreviewReq,
    draft: bool,
) -> Result<(lightcraft_raster::Rgba8, u64), String> {
    let (w, h) = req.fit(draft);
    let id = PhotoId(req.photo);
    let started = Instant::now();
    let job = if req.commit {
        session
            .render_job(id, w as usize, h as usize, false, true)
            .ok_or_else(|| "no such photo".to_string())?
    } else {
        let current = session
            .develop_of(id)
            .ok_or_else(|| "no such photo".to_string())?;
        let mut settings = (*current).clone();
        for (key, value) in &req.values {
            lightcraft_engine::develop::controls::set(&mut settings, key, *value);
        }
        session
            .preview_job(id, w as usize, h as usize, true, &settings)
            .ok_or_else(|| "no such photo".to_string())?
    };
    let job = if draft { job.draft() } else { job };
    let result = job.run();
    session.accept(&result);
    let rendered = result.rendered?;
    Ok((rendered.image, started.elapsed().as_micros() as u64))
}

fn open_photo(session: &mut Session, path: &str) -> Result<OpenResult, String> {
    let path = std::fs::canonicalize(path).map_err(|e| e.to_string())?;
    let path = path.to_string_lossy().to_string();
    let report = session
        .execute("library.import", &json!({"paths": [path]}))
        .map_err(|e| e.to_string())?;
    let imported = ids_of(&report["imported"]);
    let existing = report["duplicates"].as_array().and_then(|rows| {
        rows.iter()
            .find_map(|row| row.get("existing").and_then(Value::as_u64))
    });
    let id = imported
        .first()
        .copied()
        .or(existing)
        .ok_or_else(|| format!("import failed: {}", report["failed"]))?;
    session
        .execute("library.select", &json!({"ids": [id], "active": id}))
        .map_err(|e| e.to_string())?;
    let photo = session
        .catalog
        .photo(PhotoId(id))
        .ok_or("photo missing after import")?;
    let exposure =
        lightcraft_engine::develop::controls::get(&photo.develop, "light.exposure").unwrap_or(0.0);
    let temp =
        lightcraft_engine::develop::controls::get(&photo.develop, "wb.temp").unwrap_or(6500.0);
    Ok(OpenResult {
        photo: id,
        width: photo.width,
        height: photo.height,
        name: photo.file_name.clone(),
        exposure,
        temp,
    })
}

fn ids_of(value: &Value) -> Vec<u64> {
    value
        .as_array()
        .map(|rows| rows.iter().filter_map(Value::as_u64).collect())
        .unwrap_or_default()
}

fn export_one(session: &mut Session, req: &ExportReq) -> Result<ExportResult, String> {
    let started = Instant::now();
    let params = json!({
        "format": req.format,
        "quality": req.quality.max(1),
        "longEdge": req.long_edge,
        "colorSpace": "srgb",
    });
    let opts = lightcraft_engine::export::ExportOptions::from_json(&params);
    let exported = lightcraft_engine::export::export_photo(session, PhotoId(req.photo), &opts, 1)?;
    if let Some(parent) = std::path::Path::new(&req.dest).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
    }
    std::fs::write(&req.dest, &exported.bytes).map_err(|e| e.to_string())?;
    Ok(ExportResult {
        path: req.dest.clone(),
        width: exported.width as u32,
        height: exported.height as u32,
        bytes: exported.bytes.len() as u64,
        server_us: started.elapsed().as_micros() as u64,
    })
}

fn run_command(session: &mut Session, req: &CommandReq) -> Result<Value, String> {
    if req.id == "omalight.task" {
        let kind = req.params.get("kind").and_then(Value::as_str).unwrap_or("");
        let task = parse_task(kind, &req.params)?;
        let outcome = prepare(&task);
        return match &outcome {
            HeavyOutcome::Ready { command, params } => {
                let result = session
                    .execute(command, params)
                    .map_err(|e| e.to_string())?;
                Ok(json!({"task": outcome_json(&outcome), "result": result}))
            }
            HeavyOutcome::Stub { .. } => Ok(outcome_json(&outcome)),
        };
    }
    let params = if req.params.is_null() {
        json!({})
    } else {
        req.params.clone()
    };
    session.execute(&req.id, &params).map_err(|e| e.to_string())
}
