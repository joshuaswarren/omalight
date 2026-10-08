//! Blocking client for the engine channel.
//!
//! A reader thread owns the socket's read half so a preview can be superseded while an earlier
//! one is still on the wire. Writes stay on the calling thread.

use crate::pair::{self, decode_key, encode_key};
use omalight_protocol::{
    decode_image_payload, read_frame, write_frame, CommandReq, ErrorMsg, ExportReq, ExportResult,
    Frame, Hello, HelloOk, OpenReq, OpenResult, PairConfirm, PairOffer, PairResult, PairStart,
    PreviewMeta, PreviewReq, COMMAND, COMMAND_RESULT, ERROR, EXPORT, EXPORT_RESULT, HELLO,
    HELLO_OK, OPEN, OPEN_RESULT, PAIR_CONFIRM, PAIR_OFFER, PAIR_RESULT, PAIR_START, PING, PONG,
    PREVIEW, PREVIEW_IMAGE,
};
use serde::{de::DeserializeOwned, Serialize};
use std::collections::VecDeque;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use x25519_dalek::PublicKey;

pub struct Client {
    write: TcpStream,
    inbox: VecDeque<Frame>,
    frames: Receiver<Result<Frame, String>>,
    next_id: u64,
    reader: Option<JoinHandle<()>>,
    engine: String,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.write.shutdown(std::net::Shutdown::Both);
        if let Some(handle) = self.reader.take() {
            let _ = handle.join();
        }
    }
}

impl Client {
    pub fn connect(addr: &str, token: &str, timeout: Duration) -> Result<Self, String> {
        Self::handshake(
            addr,
            Hello {
                token: token.to_string(),
                client: "omalight".into(),
                local: false,
            },
            timeout,
        )
    }

    /// Loopback shortcut. The engine accepts this when the peer is the same user.
    pub fn connect_local(addr: &str, timeout: Duration) -> Result<Self, String> {
        Self::handshake(
            addr,
            Hello {
                token: String::new(),
                client: "omalight".into(),
                local: true,
            },
            timeout,
        )
    }

    pub fn engine(&self) -> &str {
        &self.engine
    }

    fn handshake(addr: &str, hello: Hello, timeout: Duration) -> Result<Self, String> {
        let mut stream = dial(addr, timeout)?;
        let frame = Frame::json(1, HELLO, &hello).map_err(|e| e.to_string())?;
        write_frame(&mut stream, &frame).map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .map_err(|e| e.to_string())?;
        let reply = read_frame(&mut stream).map_err(|e| e.to_string())?;
        stream.set_read_timeout(None).map_err(|e| e.to_string())?;
        if reply.kind == ERROR {
            let msg: ErrorMsg = reply.parse().unwrap_or(ErrorMsg {
                message: "authentication failed".into(),
            });
            return Err(msg.message);
        }
        if reply.kind != HELLO_OK {
            return Err(format!(
                "expected hello-ok, got {}",
                omalight_protocol::kind_name(reply.kind)
            ));
        }
        let ok: HelloOk = reply.parse().unwrap_or(HelloOk {
            version: 1,
            auth: String::new(),
            engine: String::new(),
        });
        Self::from_stream(stream, ok.engine)
    }

    pub fn ping(&mut self) -> Result<Duration, String> {
        let started = Instant::now();
        let _: serde_json::Value =
            self.call(PING, &serde_json::json!({}), Duration::from_secs(5))?;
        Ok(started.elapsed())
    }

    pub fn open(&mut self, path: &str) -> Result<OpenResult, String> {
        self.call(
            OPEN,
            &OpenReq {
                path: path.to_string(),
            },
            Duration::from_secs(60),
        )
    }

    pub fn command(
        &mut self,
        id: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        self.call(
            COMMAND,
            &CommandReq {
                id: id.to_string(),
                params,
            },
            Duration::from_secs(120),
        )
    }

    pub fn export(&mut self, req: &ExportReq) -> Result<ExportResult, String> {
        self.call(EXPORT, req, Duration::from_secs(180))
    }

    /// Send a preview and collect draft, then full (or cancelled). `rtt` is the time until the
    /// terminal frame.
    pub fn preview(&mut self, req: &PreviewReq) -> Result<PreviewRound, String> {
        let id = self.alloc();
        let frame = Frame::json(id, PREVIEW, req).map_err(|e| e.to_string())?;
        let started = Instant::now();
        write_frame(&mut self.write, &frame).map_err(|e| e.to_string())?;
        let mut round = PreviewRound {
            frames: Vec::new(),
            rtt: Duration::ZERO,
        };
        let deadline = Duration::from_secs(120);
        loop {
            let frame = self.recv_id(id, deadline)?;
            if frame.kind == ERROR {
                let msg: ErrorMsg = frame.parse().map_err(|e| e.to_string())?;
                return Err(msg.message);
            }
            if frame.kind != PREVIEW_IMAGE {
                continue;
            }
            let (meta_bytes, jpeg) =
                decode_image_payload(&frame.payload).map_err(|e| e.to_string())?;
            let meta: PreviewMeta =
                serde_json::from_slice(meta_bytes).map_err(|e| e.to_string())?;
            let terminal = meta.is_terminal();
            round.frames.push(PreviewFrame {
                meta,
                jpeg: jpeg.to_vec(),
            });
            if terminal {
                round.rtt = started.elapsed();
                return Ok(round);
            }
        }
    }

    fn call<T: Serialize, R: DeserializeOwned>(
        &mut self,
        kind: u8,
        body: &T,
        timeout: Duration,
    ) -> Result<R, String> {
        let id = self.alloc();
        let frame = Frame::json(id, kind, body).map_err(|e| e.to_string())?;
        write_frame(&mut self.write, &frame).map_err(|e| e.to_string())?;
        let frame = self.recv_id(id, timeout)?;
        if frame.kind == ERROR {
            let msg: ErrorMsg = frame.parse().map_err(|e| e.to_string())?;
            return Err(msg.message);
        }
        let expected = match kind {
            PING => PONG,
            OPEN => OPEN_RESULT,
            EXPORT => EXPORT_RESULT,
            COMMAND => COMMAND_RESULT,
            other => other,
        };
        if frame.kind != expected && kind != PING {
            return Err(format!(
                "reply kind {} for request {}",
                omalight_protocol::kind_name(frame.kind),
                omalight_protocol::kind_name(kind)
            ));
        }
        frame.parse().map_err(|e| e.to_string())
    }

    fn alloc(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }

    fn recv_id(&mut self, id: u64, timeout: Duration) -> Result<Frame, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = self.inbox.iter().position(|frame| frame.id == id) {
                return Ok(self.inbox.remove(pos).expect("position"));
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("timed out waiting for frame {id}"));
            }
            match self.frames.recv_timeout(left) {
                Ok(Ok(frame)) => self.inbox.push_back(frame),
                Ok(Err(err)) => return Err(err),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(format!("timed out waiting for frame {id}"))
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("engine connection closed".into())
                }
            }
        }
    }
}

pub struct PreviewFrame {
    pub meta: PreviewMeta,
    pub jpeg: Vec<u8>,
}

pub struct PreviewRound {
    pub frames: Vec<PreviewFrame>,
    pub rtt: Duration,
}

/// Kills the spawned engine when dropped.
pub struct ChildGuard(pub Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Compare-and-confirm pairing. `confirm` sees the six-digit code and returns whether it matches
/// the engine. The credential comes back only after the engine accepts.
pub fn pair(
    addr: &str,
    name: &str,
    timeout: Duration,
    confirm: impl FnOnce(&str) -> bool,
) -> Result<String, String> {
    let name = crate::identity::clean_name(name)?;
    let mut stream = dial(addr, timeout)?;
    let (secret, public) = pair::client_keypair();
    let start = Frame::json(
        1,
        PAIR_START,
        &PairStart {
            name,
            client_pub: encode_key(public.as_bytes()),
        },
    )
    .map_err(|e| e.to_string())?;
    write_frame(&mut stream, &start).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let offer_frame = read_frame(&mut stream).map_err(|e| e.to_string())?;
    if offer_frame.kind == ERROR {
        let msg: ErrorMsg = offer_frame.parse().unwrap_or(ErrorMsg {
            message: "pairing failed".into(),
        });
        return Err(msg.message);
    }
    if offer_frame.kind != PAIR_OFFER {
        return Err(format!(
            "expected pair-offer, got {}",
            omalight_protocol::kind_name(offer_frame.kind)
        ));
    }
    let offer: PairOffer = offer_frame.parse().map_err(|e| e.to_string())?;
    let server_pub = PublicKey::from(decode_key(&offer.server_pub)?);
    let code = pair::shared_code(secret, &public, &server_pub, true);
    let accept = confirm(&code);
    stream.set_read_timeout(None).map_err(|e| e.to_string())?;
    let confirm_frame =
        Frame::json(2, PAIR_CONFIRM, &PairConfirm { code, accept }).map_err(|e| e.to_string())?;
    write_frame(&mut stream, &confirm_frame)
        .map_err(|e| format!("pairing window expired ({e})"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|e| e.to_string())?;
    let result_frame = read_frame(&mut stream).map_err(|e| {
        if e.to_string().contains("timed out") || e.to_string().contains("expired") {
            format!("pairing window expired ({e})")
        } else {
            e.to_string()
        }
    })?;
    if result_frame.kind == ERROR {
        let msg: ErrorMsg = result_frame.parse().unwrap_or(ErrorMsg {
            message: "pairing failed".into(),
        });
        return Err(msg.message);
    }
    if result_frame.kind != PAIR_RESULT {
        return Err(format!(
            "expected pair-result, got {}",
            omalight_protocol::kind_name(result_frame.kind)
        ));
    }
    let result: PairResult = result_frame.parse().map_err(|e| e.to_string())?;
    if !result.ok || result.credential.len() < 16 {
        return Err(if result.message.is_empty() {
            "pairing failed".into()
        } else {
            result.message
        });
    }
    Ok(result.credential)
}

pub fn spawn_local(bin: &std::path::Path) -> Result<(Client, ChildGuard, String), String> {
    let dir = std::env::temp_dir().join(format!(
        "omalight-{}-{}",
        std::process::id(),
        crate::identity::random_hex(4)
    ));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ready = dir.join("ready");
    let _ = std::fs::remove_file(&ready);
    let mut child = Command::new(bin)
        .args(["--bind", "127.0.0.1:0", "--no-mdns", "--config-dir"])
        .arg(&dir)
        .arg("--ready-file")
        .arg(&ready)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn {}: {e}", bin.display()))?;
    let addr = wait_ready(&ready, &mut child, Duration::from_secs(30)).inspect_err(|_| {
        let _ = child.kill();
    })?;
    let client = connect_spawned(&addr, &dir).inspect_err(|_| {
        let _ = child.kill();
    })?;
    Ok((client, ChildGuard(child), addr))
}

fn connect_spawned(addr: &str, dir: &std::path::Path) -> Result<Client, String> {
    match Client::connect_local(addr, Duration::from_secs(10)) {
        Ok(client) => Ok(client),
        Err(_) => {
            let secret = crate::identity::read_secret(&dir.join("secret"))?;
            Client::connect(addr, &secret, Duration::from_secs(10))
        }
    }
}

fn dial(addr: &str, timeout: Duration) -> Result<TcpStream, String> {
    let mut targets = addr.to_socket_addrs().map_err(|e| format!("{addr}: {e}"))?;
    let target = targets
        .next()
        .ok_or_else(|| format!("{addr}: no address"))?;
    let stream =
        TcpStream::connect_timeout(&target, timeout).map_err(|e| format!("connect {addr}: {e}"))?;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    Ok(stream)
}

impl Client {
    fn from_stream(stream: TcpStream, engine: String) -> Result<Self, String> {
        let mut read = stream.try_clone().map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel();
        let reader = thread::spawn(move || loop {
            match read_frame(&mut read) {
                Ok(frame) => {
                    if tx.send(Ok(frame)).is_err() {
                        break;
                    }
                }
                Err(err) => {
                    let _ = tx.send(Err(err.to_string()));
                    break;
                }
            }
        });
        Ok(Self {
            write: stream,
            inbox: VecDeque::new(),
            frames: rx,
            next_id: 2,
            reader: Some(reader),
            engine,
        })
    }
}

fn wait_ready(
    path: &std::path::Path,
    child: &mut Child,
    timeout: Duration,
) -> Result<String, String> {
    let started = Instant::now();
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let line = text.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                return Ok(line.to_string());
            }
        }
        if let Some(status) = child.try_wait().map_err(|e| e.to_string())? {
            let mut err = String::new();
            if let Some(stderr) = child.stderr.as_mut() {
                let _ = stderr.read_to_string(&mut err);
            }
            return Err(format!("engine exited {status}: {err}"));
        }
        if started.elapsed() > timeout {
            return Err("engine did not report a port".into());
        }
        thread::sleep(Duration::from_millis(20));
    }
}
