//! Engine daemon and client as two processes, over localhost TCP.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn separate_processes_open_edit_preview_export() {
    let engine = env!("CARGO_BIN_EXE_omalight-engine");
    let client = env!("CARGO_BIN_EXE_omalight");
    let dir = std::env::temp_dir().join(format!("omalight-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let engine_dir = dir.join("engine");
    let client_dir = dir.join("client");
    let dng = omalight_core::fixture::write_fixture(&dir, 128, 96).unwrap();
    let ready = dir.join("ready");

    let mut engine_proc = Command::new(engine)
        .args(["--bind", "127.0.0.1:0", "--no-mdns", "--config-dir"])
        .arg(&engine_dir)
        .arg("--ready-file")
        .arg(&ready)
        .env("LIGHTCRAFT_GPU", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn engine");

    let addr = wait_ready(&ready, &mut engine_proc);

    let mut pair = Command::new(client)
        .args([
            "pair",
            "--connect",
            &addr,
            "--yes",
            "--name",
            "ci",
            "--code-only",
            "--config-dir",
        ])
        .arg(&client_dir)
        .env("LIGHTCRAFT_GPU", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn pair");
    let paired = wait_exit(&mut pair, Duration::from_secs(30));
    let pair_out = read_pipe(pair.stdout.take());
    let pair_err = read_pipe(pair.stderr.take());
    assert!(
        paired,
        "pair timed out\nstdout:{pair_out}\nstderr:{pair_err}"
    );
    let pair_status = pair.wait().expect("wait pair");
    assert!(
        pair_status.success(),
        "pair exit {pair_status}\nstdout:{pair_out}\nstderr:{pair_err}"
    );
    assert!(pair_out.contains("paired"), "{pair_out}");
    assert!(
        !pair_out.contains("credential"),
        "the client must not print the credential\n{pair_out}"
    );
    assert!(
        client_dir.join("paired.toml").is_file(),
        "paired.toml was not written"
    );

    let export = dir.join("out.jpg");
    let latency = dir.join("latency.json");
    let mut child = Command::new(client)
        .args(["--headless", "--connect", &addr, "--config-dir"])
        .arg(&client_dir)
        .args([
            "--open",
            dng.to_str().unwrap(),
            "--set",
            "light.exposure=0.8",
            "--set",
            "wb.temp=5200",
            "--export",
            export.to_str().unwrap(),
            "--long-edge",
            "256",
            "--latency-json",
            latency.to_str().unwrap(),
        ])
        .env("LIGHTCRAFT_GPU", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn client");

    let finished = wait_exit(&mut child, Duration::from_secs(120));
    let stdout = read_pipe(child.stdout.take());
    let stderr = read_pipe(child.stderr.take());
    let _ = engine_proc.kill();
    let _ = engine_proc.wait();
    let engine_err = read_pipe(engine_proc.stderr.take());

    assert!(
        finished,
        "headless client timed out\nstdout:{stdout}\nstderr:{stderr}\nengine:{engine_err}"
    );
    let status = child.wait().expect("wait client");
    assert!(
        status.success(),
        "client exit {status}\nstdout:{stdout}\nstderr:{stderr}\nengine:{engine_err}"
    );

    let jpeg = std::fs::read(&export).expect("export file");
    assert!(jpeg.starts_with(&[0xff, 0xd8]), "export is a jpeg");

    let report: serde_json::Value =
        serde_json::from_str(std::fs::read_to_string(&latency).unwrap().trim()).unwrap();
    for key in [
        "ping_us",
        "open_us",
        "draft_us",
        "full_us",
        "preview_rtt_us",
        "export_us",
        "export_server_us",
    ] {
        let n = report[key].as_u64().unwrap_or(0);
        assert!(n > 0, "{key} should be a measured duration, got {report}");
    }
    assert!(
        report["ping_us"].as_u64().unwrap() < 2_000_000,
        "ping {}",
        report["ping_us"]
    );
    assert!(
        report["full_us"].as_u64().unwrap() < 60_000_000,
        "full render {}",
        report["full_us"]
    );
    assert!(report["draft_px"]
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n.as_u64().unwrap() > 0));
    eprintln!("e2e {report}");
}

fn wait_ready(path: &std::path::Path, child: &mut std::process::Child) -> String {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            let line = text.lines().next().unwrap_or("").trim();
            if !line.is_empty() {
                return line.to_string();
            }
        }
        if let Some(status) = child.try_wait().ok().flatten() {
            panic!("engine exited {status} before ready");
        }
        if Instant::now() > deadline {
            panic!("engine did not write a ready file");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_exit(child: &mut std::process::Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    loop {
        if child.try_wait().ok().flatten().is_some() {
            return true;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn read_pipe(pipe: Option<impl std::io::Read>) -> String {
    let mut out = String::new();
    if let Some(mut pipe) = pipe {
        let _ = std::io::Read::read_to_string(&mut pipe, &mut out);
    }
    out
}
