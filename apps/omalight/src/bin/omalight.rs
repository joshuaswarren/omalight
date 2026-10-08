fn main() {
    if let Err(err) = omalight_app::ui::entry() {
        eprintln!("omalight: {err}");
        std::process::exit(1);
    }
}
