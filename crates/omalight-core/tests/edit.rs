//! Engine and client in one process, talking over a real TCP socket.
//! The separate-process check lives in the `omalight` package.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use lightcraft_engine::gpu;
use omalight_core::client::Client;
use omalight_core::fixture::write_fixture;
use omalight_core::identity;
use omalight_core::jpeg;
use omalight_core::server::Server;
use omalight_protocol::{ExportReq, PreviewReq};
use serde_json::json;

#[test]
fn exposure_white_balance_preview_and_export() {
    gpu::set_enabled(false);
    unsafe { std::env::set_var("LIGHTCRAFT_GPU", "0") };

    let dir = std::env::temp_dir().join(format!("omalight-edit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let dng = write_fixture(&dir, 128, 96).unwrap();

    let cfg = dir.join("engine");
    let server = Server::open(&cfg).unwrap();
    let secret = identity::read_secret(&cfg.join("secret")).unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server = Arc::new(server);
    thread::spawn(move || server.serve(listener));

    let bad = Client::connect(
        &addr.to_string(),
        "not-a-valid-token",
        Duration::from_secs(5),
    );
    assert!(bad.is_err(), "unauthenticated client must fail");

    let mut client = Client::connect(&addr.to_string(), &secret, Duration::from_secs(5)).unwrap();
    let ping = client.ping().unwrap();
    let opened = client.open(dng.to_str().unwrap()).unwrap();
    assert!(opened.width > 0 && opened.height > 0, "decoded size");

    let (luma_0, draft_us, full_us, rtt_0) =
        render(&mut client, opened.photo, &[("light.exposure", 0.0)], 1);
    let (luma_1, _, full_hi, rtt_1) =
        render(&mut client, opened.photo, &[("light.exposure", 1.5)], 2);
    assert!(
        luma_1 > luma_0 + 8.0,
        "exposure +1.5 EV should lift the preview ({luma_0} -> {luma_1})"
    );

    client.command("edit.undo", json!({})).unwrap();
    let (luma_back, _, _, _) = render(&mut client, opened.photo, &[], 3);
    assert!(
        luma_back < luma_1 - 4.0,
        "undo should bring the exposure back ({luma_back} vs {luma_1})"
    );

    let warm = channels(&mut client, opened.photo, &[("wb.temp", 3200.0)], 4);
    let cool = channels(&mut client, opened.photo, &[("wb.temp", 9000.0)], 5);
    let warm_ratio = warm.0 / warm.2.max(1.0);
    let cool_ratio = cool.0 / cool.2.max(1.0);
    assert!(
        (warm_ratio - cool_ratio).abs() > 0.02,
        "white balance should move red/blue ({warm_ratio} vs {cool_ratio})"
    );

    let mask = client.command("mask.add", json!({"kind": "sky"})).unwrap();
    assert!(mask.get("activeMask").is_some(), "sky mask: {mask}");

    let stub = client
        .command("omalight.task", json!({"kind": "denoise", "strength": 0.4}))
        .unwrap();
    assert_eq!(stub["status"], "unsupported");
    assert!(stub["interface"]
        .as_str()
        .unwrap()
        .contains("DenoiseRequest"));

    let dest = dir.join("out.jpg");
    let exported = client
        .export(&ExportReq {
            photo: opened.photo,
            dest: dest.display().to_string(),
            long_edge: 256,
            quality: 85,
            format: "jpeg".into(),
        })
        .unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert!(bytes.starts_with(&[0xff, 0xd8]), "jpeg export");
    assert_eq!(exported.bytes, bytes.len() as u64);

    eprintln!(
        "latency ping_us={} open_exposure_rtt_us={} draft_us={} full_us={} full_hi_us={} export_server_us={}",
        ping.as_micros(),
        rtt_0.as_micros(),
        draft_us,
        full_us,
        full_hi,
        exported.server_us
    );
    let _ = rtt_1;
}

fn render(
    client: &mut Client,
    photo: u64,
    values: &[(&str, f64)],
    generation: u64,
) -> (f64, u64, u64, Duration) {
    let mut map = BTreeMap::new();
    for (key, value) in values {
        map.insert((*key).to_string(), *value);
    }
    let round = client
        .preview(&PreviewReq {
            photo,
            generation,
            viewport_w: 640,
            viewport_h: 480,
            values: map,
            begin: true,
            end: true,
            commit: !values.is_empty(),
        })
        .unwrap_or_else(|err| panic!("preview gen {generation}: {err}"));
    let draft = round
        .frames
        .iter()
        .find(|f| f.meta.stage == "draft")
        .unwrap_or_else(|| panic!("no draft: {:?}", stages(&round)));
    let full = round
        .frames
        .iter()
        .find(|f| f.meta.stage == "full")
        .unwrap_or_else(|| panic!("no full: {:?}", stages(&round)));
    let (_, _, rgba) = jpeg::decode(&full.jpeg).unwrap();
    (
        jpeg::mean_luma(&rgba),
        draft.meta.server_us,
        full.meta.server_us,
        round.rtt,
    )
}

fn channels(
    client: &mut Client,
    photo: u64,
    values: &[(&str, f64)],
    generation: u64,
) -> (f64, f64, f64) {
    let mut map = BTreeMap::new();
    for (key, value) in values {
        map.insert((*key).to_string(), *value);
    }
    let round = client
        .preview(&PreviewReq {
            photo,
            generation,
            viewport_w: 320,
            viewport_h: 240,
            values: map,
            begin: true,
            end: true,
            commit: true,
        })
        .unwrap();
    let full = round
        .frames
        .iter()
        .find(|f| f.meta.stage == "full")
        .expect("full wb preview");
    let (_, _, rgba) = jpeg::decode(&full.jpeg).unwrap();
    jpeg::mean_channels(&rgba)
}

fn stages(round: &omalight_core::client::PreviewRound) -> Vec<&str> {
    round.frames.iter().map(|f| f.meta.stage.as_str()).collect()
}
