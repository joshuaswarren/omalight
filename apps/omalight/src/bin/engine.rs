fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    if let Err(err) = omalight_core::engine::run() {
        eprintln!("omalight-engine: {err}");
        std::process::exit(1);
    }
}
