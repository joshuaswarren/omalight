//! stdio MCP server. The window and this process speak the same engine channel, so an agent
//! edits the photo the daemon already has open.

use omalight_core::client::{spawn_local, ChildGuard, Client};
use omalight_core::config::{self, ConnectMode};
use omalight_protocol::{plan_request, read_lsp, write_lsp, Endpoint, ExportReq, PreviewReq};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::{BufReader, Write};
use std::path::PathBuf;
use std::time::Duration;

struct Session {
    client: Client,
    _child: Option<ChildGuard>,
    photo: Option<u64>,
    ai: Option<Endpoint>,
}

pub fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        eprintln!(
            "\
omalight-mcp — engine tools over stdio (Content-Length JSON-RPC)

  --connect HOST:PORT   use a running engine
  --local               spawn omalight-engine
  --token TOKEN         device credential, if you already have one

stdout is the MCP stream. Diagnostics go to stderr. A remote engine must already be paired
(`omalight pair HOST`) unless you pass --token."
        );
        return Ok(());
    }
    let mut connect = None;
    let mut token = None;
    let mut local = false;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--connect" => {
                i += 1;
                connect = Some(args.get(i).cloned().ok_or("--connect needs HOST:PORT")?);
            }
            "--token" => {
                i += 1;
                token = Some(args.get(i).cloned().ok_or("--token needs a value")?);
            }
            "--local" => local = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
        i += 1;
    }
    let file = config::load_file()?;
    let ai = config::ai_from_toml(&file);
    let explicit = token.filter(|value| value.len() >= 16);
    let mode = if local {
        ConnectMode::Local
    } else if connect.is_some() {
        ConnectMode::Remote
    } else {
        config::toml_str(&file, "mode")
            .map(ConnectMode::parse)
            .transpose()?
            .unwrap_or(ConnectMode::Auto)
    };
    let host = connect.or_else(|| {
        config::toml_str(&file, "host").map(|h| {
            let port = config::toml_i64(&file, "port").unwrap_or(7421);
            if h.contains(':') {
                h.to_string()
            } else {
                format!("{h}:{port}")
            }
        })
    });

    let dir = config::config_dir();
    let (client, child) = match mode {
        ConnectMode::Remote => {
            let addr = host.ok_or("remote mode needs a host")?;
            (connect_remote(&addr, explicit.as_deref(), &dir)?, None)
        }
        ConnectMode::Local => {
            let (client, child, addr) = spawn_local(&engine_bin())?;
            eprintln!("omalight-mcp local engine {addr}");
            (client, Some(child))
        }
        ConnectMode::Auto => {
            if let Some(addr) = &host {
                if let Ok(client) = connect_remote(addr, explicit.as_deref(), &dir) {
                    eprintln!("omalight-mcp remote {addr}");
                    (client, None)
                } else {
                    let (client, child, addr) = spawn_local(&engine_bin())?;
                    eprintln!("omalight-mcp local engine {addr}");
                    (client, Some(child))
                }
            } else {
                let (client, child, addr) = spawn_local(&engine_bin())?;
                eprintln!("omalight-mcp local engine {addr}");
                (client, Some(child))
            }
        }
    };

    let mut session = Session {
        client,
        _child: child,
        photo: None,
        ai,
    };
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let mut output = std::io::stdout();
    loop {
        let body = match read_lsp(&mut input) {
            Ok(body) => body,
            Err(err) => {
                if err.to_string().contains("closed") {
                    return Ok(());
                }
                return Err(err.to_string());
            }
        };
        let msg: Value = serde_json::from_slice(&body).map_err(|e| e.to_string())?;
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        if msg.get("id").is_none() {
            continue;
        }
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let reply = match method {
            "initialize" => ok(
                id,
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "omalight", "version": env!("CARGO_PKG_VERSION") }
                }),
            ),
            "tools/list" => ok(id, json!({ "tools": tools() })),
            "tools/call" => {
                let params = msg.get("params").cloned().unwrap_or(json!({}));
                match call_tool(&mut session, &params) {
                    Ok(result) => ok(id, result),
                    Err(err) => ok(
                        id,
                        json!({
                            "content": [{ "type": "text", "text": err }],
                            "isError": true
                        }),
                    ),
                }
            }
            "ping" => ok(id, json!({})),
            other => err_rpc(id, -32601, format!("unknown method {other}")),
        };
        let bytes = serde_json::to_vec(&reply).map_err(|e| e.to_string())?;
        write_lsp(&mut output, &bytes).map_err(|e| e.to_string())?;
        output.flush().map_err(|e| e.to_string())?;
    }
}

fn ok(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn err_rpc(id: Value, code: i32, message: String) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn tools() -> Vec<Value> {
    vec![
        tool("status", "Ping the engine and report the open photo, if any.", json!({"type": "object", "properties": {}})),
        tool("open_photo", "Import a raw, DNG, or JPEG into the engine session and select it.", json!({
            "type": "object",
            "properties": { "path": { "type": "string" } },
            "required": ["path"]
        })),
        tool("develop_set", "Set one LightCraft develop control, for example light.exposure or wb.temp.", json!({
            "type": "object",
            "properties": {
                "control": { "type": "string" },
                "value": { "type": "number" },
                "values": { "type": "object" }
            }
        })),
        tool("preview", "Render a viewport-sized JPEG and write it to path. Returns server time in microseconds.", json!({
            "type": "object",
            "properties": {
                "path": { "type": "string" },
                "viewport_w": { "type": "integer" },
                "viewport_h": { "type": "integer" }
            },
            "required": ["path"]
        })),
        tool("export_jpeg", "Write a JPEG of the current develop settings. The path is on the engine host.", json!({
            "type": "object",
            "properties": {
                "dest": { "type": "string" },
                "long_edge": { "type": "integer" },
                "quality": { "type": "integer" }
            },
            "required": ["dest"]
        })),
        tool("execute", "Run any LightCraft command id with a JSON params object.", json!({
            "type": "object",
            "properties": {
                "id": { "type": "string" },
                "params": { "type": "object" }
            },
            "required": ["id"]
        })),
        tool("undo", "Undo the last develop interaction.", json!({"type": "object", "properties": {}})),
        tool("mask_add", "Add a mask. sky, subject, background, linear, and radial are implemented. object and prompt need SAM.", json!({
            "type": "object",
            "properties": { "kind": { "type": "string" } },
            "required": ["kind"]
        })),
        tool("plan", "Turn a sentence into engine steps. Pass apply=true to run them.", json!({
            "type": "object",
            "properties": {
                "text": { "type": "string" },
                "apply": { "type": "boolean" }
            },
            "required": ["text"]
        })),
        tool("heavy_task", "Run or describe an engine-host task: mask, denoise, upscale, or tag.", json!({
            "type": "object",
            "properties": {
                "kind": { "type": "string", "enum": ["mask", "denoise", "upscale", "tag"] },
                "mask": { "type": "string" },
                "strength": { "type": "number" },
                "scale": { "type": "integer" }
            },
            "required": ["kind"]
        })),
    ]
}

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": schema})
}

fn call_tool(session: &mut Session, params: &Value) -> Result<Value, String> {
    let name = params
        .get("name")
        .and_then(Value::as_str)
        .ok_or("tools/call needs a name")?;
    let args = params.get("arguments").cloned().unwrap_or(json!({}));
    let value = match name {
        "status" => {
            let ping = session.client.ping()?;
            json!({"ping_us": ping.as_micros() as u64, "photo": session.photo})
        }
        "open_photo" => {
            let path = args.get("path").and_then(Value::as_str).ok_or("path")?;
            let opened = session.client.open(path)?;
            session.photo = Some(opened.photo);
            serde_json::to_value(&opened).map_err(|e| e.to_string())?
        }
        "develop_set" => {
            let params = if args.get("values").is_some() {
                json!({"values": args["values"]})
            } else {
                json!({"control": args["control"], "value": args["value"]})
            };
            session.client.command("develop.set", params)?
        }
        "preview" => {
            let photo = session.photo.ok_or("open a photo first")?;
            let path = args.get("path").and_then(Value::as_str).ok_or("path")?;
            let mut values = BTreeMap::new();
            if let Some(map) = args.get("values").and_then(Value::as_object) {
                for (k, v) in map {
                    if let Some(n) = v.as_f64() {
                        values.insert(k.clone(), n);
                    }
                }
            }
            let round = session.client.preview(&PreviewReq {
                photo,
                generation: 1,
                viewport_w: args
                    .get("viewport_w")
                    .and_then(Value::as_u64)
                    .unwrap_or(1280) as u32,
                viewport_h: args
                    .get("viewport_h")
                    .and_then(Value::as_u64)
                    .unwrap_or(800) as u32,
                values,
                begin: false,
                end: false,
                commit: false,
            })?;
            let full = round
                .frames
                .iter()
                .rev()
                .find(|f| !f.jpeg.is_empty())
                .ok_or("no preview")?;
            std::fs::write(path, &full.jpeg).map_err(|e| e.to_string())?;
            json!({"path": path, "stage": full.meta.stage, "server_us": full.meta.server_us, "width": full.meta.width, "height": full.meta.height})
        }
        "export_jpeg" => {
            let photo = session.photo.ok_or("open a photo first")?;
            let dest = args.get("dest").and_then(Value::as_str).ok_or("dest")?;
            let exported = session.client.export(&ExportReq {
                photo,
                dest: dest.into(),
                long_edge: args
                    .get("long_edge")
                    .and_then(Value::as_u64)
                    .unwrap_or(2048) as u32,
                quality: args.get("quality").and_then(Value::as_u64).unwrap_or(90) as u8,
                format: "jpeg".into(),
            })?;
            serde_json::to_value(&exported).map_err(|e| e.to_string())?
        }
        "execute" => {
            let id = args.get("id").and_then(Value::as_str).ok_or("id")?;
            let params = args.get("params").cloned().unwrap_or(json!({}));
            session.client.command(id, params)?
        }
        "undo" => session.client.command("edit.undo", json!({}))?,
        "mask_add" => {
            let kind = args.get("kind").and_then(Value::as_str).ok_or("kind")?;
            session
                .client
                .command("omalight.task", json!({"kind": "mask", "mask": kind}))?
        }
        "plan" => {
            let text = args.get("text").and_then(Value::as_str).ok_or("text")?;
            let plan = plan_request(text, session.ai.as_ref());
            if args.get("apply").and_then(Value::as_bool).unwrap_or(false) {
                apply_plan(session, &plan)?;
            }
            serde_json::to_value(&plan).map_err(|e| e.to_string())?
        }
        "heavy_task" => {
            let params = args.clone();
            if params.get("kind").is_none() {
                return Err("kind".into());
            }
            session.client.command("omalight.task", params)?
        }
        other => return Err(format!("unknown tool {other}")),
    };
    Ok(json!({
        "content": [{ "type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string()) }],
        "isError": false
    }))
}

fn apply_plan(session: &mut Session, plan: &omalight_protocol::WorkPlan) -> Result<(), String> {
    use omalight_protocol::Step;
    for step in &plan.steps {
        match step {
            Step::Develop { control, value } => {
                session
                    .client
                    .command("develop.set", json!({"control": control, "value": value}))?;
            }
            Step::Mask { kind } => {
                session
                    .client
                    .command("omalight.task", json!({"kind": "mask", "mask": kind}))?;
            }
            Step::Export {
                long_edge,
                quality,
                format,
            } => {
                let photo = session.photo.ok_or("open a photo first")?;
                let dest = std::env::temp_dir().join(format!("omalight-mcp-{photo}.{format}"));
                session.client.export(&ExportReq {
                    photo,
                    dest: dest.display().to_string(),
                    long_edge: *long_edge,
                    quality: *quality,
                    format: format.clone(),
                })?;
            }
            Step::Undo => {
                session.client.command("edit.undo", json!({}))?;
            }
            Step::Command { id, params } => {
                session.client.command(id, params.clone())?;
            }
        }
    }
    Ok(())
}

fn connect_remote(
    addr: &str,
    token: Option<&str>,
    dir: &std::path::Path,
) -> Result<Client, String> {
    if let Some(token) = token {
        return Client::connect(addr, token, Duration::from_secs(5));
    }
    if let Some(saved) = omalight_core::identity::paired_token(dir, addr) {
        return Client::connect(addr, &saved, Duration::from_secs(5));
    }
    Err(format!("not paired with {addr}. Run: omalight pair {addr}"))
}

fn engine_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("omalight-engine"));
    path.pop();
    path.push("omalight-engine");
    if path.exists() {
        path
    } else {
        PathBuf::from("omalight-engine")
    }
}
