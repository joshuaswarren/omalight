fn main() {
    if let Err(err) = omalight_app::mcp::run() {
        eprintln!("omalight-mcp: {err}");
        std::process::exit(1);
    }
}
