//! First-run secret, compare-and-confirm pairing, expiry, and revocation.
//! mDNS is attempted and skipped when this machine has no multicast.

use std::io::Write;
use std::net::TcpStream;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use lightcraft_engine::gpu;
use omalight_core::client::{self, Client};
use omalight_core::identity::Store;
use omalight_core::mdns;
use omalight_core::server::Server;
use omalight_protocol::{
    read_frame, write_frame, Frame, PairConfirm, PairOffer, PairStart, PAIR_CONFIRM, PAIR_OFFER,
    PAIR_START,
};

fn boot(ttl: Option<Duration>) -> (String, std::path::PathBuf) {
    gpu::set_enabled(false);
    unsafe { std::env::set_var("LIGHTCRAFT_GPU", "0") };
    let dir = std::env::temp_dir().join(format!(
        "omalight-pair-{}-{}",
        std::process::id(),
        omalight_core::identity::random_hex(4)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut server = Server::open(&dir).unwrap();
    if let Some(ttl) = ttl {
        server.set_pair_ttl(ttl);
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = Arc::new(server);
    thread::spawn(move || server.serve(listener));
    (addr, dir)
}

#[test]
fn approve_then_reconnect_and_revoke() {
    let (addr, dir) = boot(None);
    let cred = client::pair(&addr, "laptop", Duration::from_secs(5), |code| {
        assert_eq!(code.len(), 6, "{code}");
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        true
    })
    .unwrap();

    let mut client = Client::connect(&addr, &cred, Duration::from_secs(5)).unwrap();
    client.ping().unwrap();

    let bad = Client::connect(&addr, "not-a-valid-token", Duration::from_secs(5));
    assert!(bad.is_err(), "a bad credential must fail");

    let mut other = Store::open(&dir).unwrap();
    other.revoke("laptop").unwrap();
    let again = Client::connect(&addr, &cred, Duration::from_secs(5));
    assert!(again.is_err(), "revoked credential must fail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reject_creates_no_device() {
    let (addr, dir) = boot(None);
    let err = client::pair(&addr, "laptop", Duration::from_secs(5), |_| false).unwrap_err();
    assert!(err.contains("rejected"), "{err}");
    let mut store = Store::open(&dir).unwrap();
    assert!(store.devices().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pairing_window_expires() {
    let (addr, dir) = boot(Some(Duration::from_millis(250)));
    let err = client::pair(&addr, "laptop", Duration::from_secs(5), |_code| {
        thread::sleep(Duration::from_millis(800));
        true
    })
    .unwrap_err();
    assert!(err.contains("expired"), "{err}");
    let mut store = Store::open(&dir).unwrap();
    assert!(store.devices().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn offer_frame_has_no_code() {
    let (addr, dir) = boot(None);
    let mut stream = TcpStream::connect(&addr).unwrap();
    let (_secret, public) = omalight_core::pair::client_keypair();
    let start = Frame::json(
        1,
        PAIR_START,
        &PairStart {
            name: "laptop".into(),
            client_pub: omalight_core::pair::encode_key(public.as_bytes()),
        },
    )
    .unwrap();
    write_frame(&mut stream, &start).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let offer = read_frame(&mut stream).unwrap();
    assert_eq!(offer.kind, PAIR_OFFER);
    let parsed: PairOffer = offer.parse().unwrap();
    let value: serde_json::Value = serde_json::from_slice(&offer.payload).unwrap();
    let keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, vec!["serverPub".to_string()]);
    assert!(!parsed.server_pub.is_empty());
    let text = String::from_utf8(offer.payload.clone()).unwrap();
    assert!(!text.contains("\"code\""));
    let confirm = Frame::json(
        2,
        PAIR_CONFIRM,
        &PairConfirm {
            code: "000000".into(),
            accept: false,
        },
    )
    .unwrap();
    write_frame(&mut stream, &confirm).unwrap();
    let _ = read_frame(&mut stream);
    drop(stream);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pairing_is_rate_limited() {
    let (addr, dir) = boot(None);
    for _ in 0..8 {
        let err = client::pair(&addr, "laptop", Duration::from_secs(5), |_| false).unwrap_err();
        assert!(err.contains("rejected"), "{err}");
    }
    let err = client::pair(&addr, "laptop", Duration::from_secs(5), |_| true).unwrap_err();
    assert!(err.contains("too many"), "{err}");
    let mut store = Store::open(&dir).unwrap();
    assert!(store.devices().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mdns_advertise_and_browse() {
    match mdns::advertise("ci-engine", 7421) {
        Ok(advertiser) => match advertiser.browse(Duration::from_millis(800)) {
            Ok(found) if found.is_empty() => {
                eprintln!("mdns browse returned nothing; multicast may be blocked");
            }
            Ok(found) => eprintln!("mdns browse saw {} service(s)", found.len()),
            Err(err) => eprintln!("mdns browse skipped: {err}"),
        },
        Err(err) => eprintln!("mdns advertise skipped: {err}"),
    }
}

#[test]
fn ssh_script_returns_a_credential() {
    let dir = std::env::temp_dir().join(format!(
        "omalight-ssh-{}-{}",
        std::process::id(),
        omalight_core::identity::random_hex(3)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let script = dir.join("ssh");
    let mut file = std::fs::File::create(&script).unwrap();
    writeln!(
        file,
        "#!/bin/sh\necho credential 0123456789abcdef0123456789abcdef\n"
    )
    .unwrap();
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    let old = std::env::var_os("PATH");
    let path = std::env::join_paths(
        std::iter::once(dir.clone()).chain(std::env::split_paths(&old.clone().unwrap_or_default())),
    )
    .unwrap();
    let _restore = PathRestore(old);
    unsafe { std::env::set_var("PATH", &path) };
    let cred = omalight_core::setup::fetch_over_ssh("studio.local", "laptop");
    drop(_restore);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(cred.unwrap(), "0123456789abcdef0123456789abcdef");
}

struct PathRestore(Option<std::ffi::OsString>);

impl Drop for PathRestore {
    fn drop(&mut self) {
        match self.0.take() {
            Some(old) => unsafe { std::env::set_var("PATH", old) },
            None => unsafe { std::env::remove_var("PATH") },
        }
    }
}
