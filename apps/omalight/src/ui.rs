//! egui window. Immediate mode keeps the develop surface small, and eframe's winit backend is a
//! native Wayland client on Hyprland (the app id is `omalight`, matching the desktop file).
//! LightCraft itself uses this stack, so the UI and the engine agree on how a frame is presented.

use crate::headless::{self, HeadlessArgs};
use crate::link::{self, FromEngine, Link, LinkOpts, ToEngine};
use omalight_core::config::{self, ConnectMode};
use omalight_core::proxy;
use omalight_protocol::{plan_rules, Color, Theme, ThemeWatch};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

pub fn entry() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("skill") {
        return link_skill(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("pair") {
        return omalight_core::setup::pair_cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("doctor") {
        return omalight_core::setup::doctor_cli(&args[1..]);
    }
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--headless") {
        return headless::run(&parse_headless(&args)?);
    }
    let opts = parse_gui(&args)?;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Omalight")
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([720.0, 480.0])
            .with_app_id("omalight"),
        ..Default::default()
    };
    eframe::run_native(
        "omalight",
        native,
        Box::new(move |cc| Ok(Box::new(App::new(cc, opts)))),
    )
    .map_err(|e| e.to_string())
}

struct GuiOpts {
    link: LinkOpts,
    open: Option<PathBuf>,
    screenshot_dir: Option<PathBuf>,
    fixture: Option<(usize, usize)>,
}

struct App {
    link: Link,
    theme_watch: ThemeWatch,
    status: String,
    note: String,
    exposure: f64,
    temp: f64,
    photo: Option<u64>,
    name: String,
    viewport: [u32; 2],
    dirty: bool,
    begin: bool,
    end: bool,
    commit: bool,
    last_sent: Instant,
    texture: Option<egui::TextureHandle>,
    tex_key: u64,
    base: Option<ProxyBase>,
    show_proxy: bool,
    help: bool,
    palette: bool,
    palette_text: String,
    palette_focus: bool,
    before: Option<(f64, f64)>,
    seeing_before: bool,
    screenshot_dir: Option<PathBuf>,
    shot: Shot,
    generation: u64,
    pair_code: Option<String>,
    engines: Vec<(String, String)>,
}

struct ProxyBase {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
    exposure: f64,
    temp: f64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Shot {
    Off,
    WaitImage,
    GrabDevelop,
    SaveDevelop,
    GrabPalette,
    SavePalette,
    Done,
}

impl App {
    fn new(cc: &eframe::CreationContext<'_>, opts: GuiOpts) -> Self {
        let theme_watch = ThemeWatch::new();
        apply_theme(&cc.egui_ctx, &theme_watch.theme);
        let shot = if opts.screenshot_dir.is_some() {
            Shot::WaitImage
        } else {
            Shot::Off
        };
        let app = Self {
            link: link::start(opts.link),
            theme_watch,
            status: "Connecting…".into(),
            note: String::new(),
            exposure: 0.0,
            temp: 6500.0,
            photo: None,
            name: String::new(),
            viewport: [1280, 800],
            dirty: false,
            begin: false,
            end: false,
            commit: true,
            last_sent: Instant::now(),
            texture: None,
            tex_key: 0,
            base: None,
            show_proxy: false,
            help: false,
            palette: false,
            palette_text: String::new(),
            palette_focus: false,
            before: None,
            seeing_before: false,
            screenshot_dir: opts.screenshot_dir,
            shot,
            generation: 0,
            pair_code: None,
            engines: Vec::new(),
        };
        if let Some(path) = opts.open {
            let _ = app.link.tx.send(ToEngine::Open(path));
        } else if let Some((w, h)) = opts.fixture {
            if let Ok(path) = write_shot_fixture(w, h) {
                let _ = app.link.tx.send(ToEngine::Open(path));
            }
        }
        app
    }

    fn pump(&mut self, ctx: &egui::Context) {
        if self.theme_watch.poll() {
            apply_theme(ctx, &self.theme_watch.theme);
        }
        while let Ok(msg) = self.link.rx.try_recv() {
            match msg {
                FromEngine::Status(text) => self.status = text,
                FromEngine::Failed(text) => self.note = text,
                FromEngine::Note(text) => {
                    if let Some(rest) = text.strip_prefix("sliders ") {
                        let mut parts = rest.split_whitespace();
                        if let Some(ev) =
                            parts
                                .next()
                                .and_then(|s| if s == "-" { None } else { s.parse().ok() })
                        {
                            self.exposure = ev;
                        }
                        if let Some(k) =
                            parts
                                .next()
                                .and_then(|s| if s == "-" { None } else { s.parse().ok() })
                        {
                            self.temp = k;
                        }
                        self.dirty = true;
                        self.begin = true;
                        self.end = true;
                        self.commit = true;
                    }
                    self.note = text;
                }
                FromEngine::Opened {
                    photo,
                    name,
                    exposure,
                    temp,
                    ..
                } => {
                    self.photo = Some(photo);
                    self.name = name;
                    self.exposure = exposure;
                    self.temp = temp;
                    self.before = Some((exposure, temp));
                    self.dirty = true;
                    self.begin = false;
                    self.end = false;
                    self.commit = true;
                }
                FromEngine::Image {
                    generation,
                    stage,
                    width,
                    height,
                    rgba,
                    server_us,
                    rtt_us,
                    exposure,
                    temp,
                    ..
                } => {
                    if generation < self.generation && self.generation != 0 {
                        continue;
                    }
                    self.base = Some(ProxyBase {
                        rgba: rgba.clone(),
                        width,
                        height,
                        exposure,
                        temp,
                    });
                    self.show_proxy = false;
                    self.upload(ctx, &rgba, width, height, generation);
                    self.note = format!("{stage}  render {server_us} µs  round trip {rtt_us} µs");
                    if stage == "full" && self.shot == Shot::WaitImage {
                        self.shot = Shot::GrabDevelop;
                    }
                }
                FromEngine::Exported { path, bytes } => {
                    self.note = format!("exported {path} ({bytes} bytes)");
                }
                FromEngine::PairCode(code) => {
                    self.pair_code = Some(code);
                    self.status = "Confirm the pairing code".into();
                }
                FromEngine::Discovered(list) => {
                    self.engines = list;
                    self.status = "Pick an engine".into();
                }
            }
        }
        if self.dirty
            && self.photo.is_some()
            && self.last_sent.elapsed() > Duration::from_millis(16)
        {
            self.generation = self.generation.wrapping_add(1);
            let _ = self.link.tx.send(ToEngine::Edit {
                exposure: self.exposure,
                temp: self.temp,
                begin: self.begin,
                end: self.end,
                commit: self.commit,
                viewport: self.viewport,
            });
            self.dirty = false;
            self.begin = false;
            self.end = false;
            self.commit = true;
            self.last_sent = Instant::now();
        }
    }

    fn upload(&mut self, ctx: &egui::Context, rgba: &[u8], width: u32, height: u32, key: u64) {
        let image =
            egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], rgba);
        self.texture = Some(ctx.load_texture("preview", image, egui::TextureOptions::LINEAR));
        self.tex_key = key;
    }

    fn show_local_proxy(&mut self, ctx: &egui::Context) {
        let Some(base) = &self.base else { return };
        let rgba = proxy::apply(
            &base.rgba,
            base.exposure,
            self.exposure,
            base.temp,
            self.temp,
        );
        self.upload(
            ctx,
            &rgba,
            base.width,
            base.height,
            self.tex_key.wrapping_add(1),
        );
        self.show_proxy = true;
    }

    fn nudge(&mut self, exposure: f64, temp: f64) {
        self.exposure = (self.exposure + exposure).clamp(-5.0, 5.0);
        self.temp = (self.temp + temp).clamp(2000.0, 50000.0);
        self.dirty = true;
        self.begin = true;
        self.end = true;
        self.commit = true;
        self.last_sent = Instant::now() - Duration::from_millis(20);
    }

    fn keys(&mut self, ctx: &egui::Context) {
        if self.palette || ctx.wants_keyboard_input() {
            if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.palette = false;
                self.help = false;
            }
            return;
        }
        let pressed = |key| ctx.input(|i| i.key_pressed(key));
        let cmd = ctx.input(|i| i.modifiers.command);
        let shift = ctx.input(|i| i.modifiers.shift);
        if pressed(egui::Key::Escape) {
            self.help = false;
            self.palette = false;
        }
        if pressed(egui::Key::Slash) && !shift {
            self.palette = true;
            self.palette_focus = true;
            return;
        }
        // `?` is shift-slash on a US layout.
        if pressed(egui::Key::Slash) && shift {
            self.help = !self.help;
        }
        if pressed(egui::Key::Q) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if cmd && pressed(egui::Key::Z) && !shift {
            let _ = self.link.tx.send(ToEngine::Command {
                id: "edit.undo".into(),
                params: serde_json::json!({}),
            });
            self.dirty = true;
        }
        if cmd && pressed(egui::Key::Z) && shift {
            let _ = self.link.tx.send(ToEngine::Command {
                id: "edit.redo".into(),
                params: serde_json::json!({}),
            });
            self.dirty = true;
        }
        if pressed(egui::Key::Plus) || pressed(egui::Key::Equals) {
            self.nudge(0.1, 0.0);
        }
        if pressed(egui::Key::Minus) {
            self.nudge(-0.1, 0.0);
        }
        if pressed(egui::Key::J) {
            self.nudge(-0.05, 0.0);
        }
        if pressed(egui::Key::K) {
            self.nudge(0.05, 0.0);
        }
        if pressed(egui::Key::OpenBracket) {
            self.nudge(0.0, -200.0);
        }
        if pressed(egui::Key::CloseBracket) {
            self.nudge(0.0, 200.0);
        }
        if pressed(egui::Key::H) {
            self.nudge(0.0, -100.0);
        }
        if pressed(egui::Key::L) {
            self.nudge(0.0, 100.0);
        }
        if pressed(egui::Key::R) {
            if let Some((ev, k)) = self.before {
                self.exposure = ev;
                self.temp = k;
                self.nudge(0.0, 0.0);
            }
        }
        if pressed(egui::Key::Backslash) {
            self.seeing_before = !self.seeing_before;
            if self.seeing_before {
                if let Some((ev, k)) = self.before {
                    let _ = self.link.tx.send(ToEngine::Edit {
                        exposure: ev,
                        temp: k,
                        begin: false,
                        end: false,
                        commit: false,
                        viewport: self.viewport,
                    });
                }
            } else {
                self.dirty = true;
                self.commit = true;
            }
        }
        if pressed(egui::Key::E) {
            if let Some(id) = self.photo {
                let dest = std::env::temp_dir().join(format!("omalight-{id}.jpg"));
                let _ = self.link.tx.send(ToEngine::Export {
                    dest,
                    long_edge: 2048,
                });
            }
        }
        for (key, rating) in [
            (egui::Key::Num0, 0),
            (egui::Key::Num1, 1),
            (egui::Key::Num2, 2),
            (egui::Key::Num3, 3),
            (egui::Key::Num4, 4),
            (egui::Key::Num5, 5),
        ] {
            if pressed(key) {
                let _ = self.link.tx.send(ToEngine::Command {
                    id: "photo.rate".into(),
                    params: serde_json::json!({"rating": rating}),
                });
            }
        }
        if pressed(egui::Key::P) {
            let _ = self.link.tx.send(ToEngine::Command {
                id: "photo.flag".into(),
                params: serde_json::json!({"flag": "pick"}),
            });
        }
        if pressed(egui::Key::X) {
            let _ = self.link.tx.send(ToEngine::Command {
                id: "photo.flag".into(),
                params: serde_json::json!({"flag": "reject"}),
            });
        }
        if pressed(egui::Key::U) {
            let _ = self.link.tx.send(ToEngine::Command {
                id: "photo.flag".into(),
                params: serde_json::json!({"flag": "none"}),
            });
        }
        if let Some(dropped) =
            ctx.input(|i| i.raw.dropped_files.iter().find_map(|f| f.path.clone()))
        {
            let _ = self.link.tx.send(ToEngine::Open(dropped));
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.pump(ctx);
        self.keys(ctx);
        ctx.request_repaint_after(Duration::from_millis(50));

        let theme = self.theme_watch.theme.clone();
        egui::TopBottomPanel::top("bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("Omalight");
                ui.label(&self.status);
                ui.separator();
                ui.label(format!("theme {}", theme.name));
                if !self.name.is_empty() {
                    ui.separator();
                    ui.label(&self.name);
                }
            });
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(if self.show_proxy { "proxy" } else { "engine" });
                ui.separator();
                ui.label(&self.note);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label("/ palette   ? help   q quit");
                });
            });
        });
        egui::SidePanel::left("develop")
            .exact_width(240.0)
            .show(ctx, |ui| {
                ui.add_space(8.0);
                ui.label("Exposure");
                let exp =
                    ui.add(egui::Slider::new(&mut self.exposure, -3.0..=3.0).fixed_decimals(2));
                slider_flags(&mut *self, &exp);
                ui.add_space(12.0);
                ui.label("Temperature");
                let temp = ui.add(
                    egui::Slider::new(&mut self.temp, 2500.0..=10000.0)
                        .fixed_decimals(0)
                        .suffix(" K"),
                );
                slider_flags(&mut *self, &temp);
                ui.add_space(16.0);
                if ui.button("Export 2048 px JPEG").clicked() {
                    if let Some(id) = self.photo {
                        let dest = std::env::temp_dir().join(format!("omalight-{id}.jpg"));
                        let _ = self.link.tx.send(ToEngine::Export {
                            dest,
                            long_edge: 2048,
                        });
                    }
                }
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("Originals stay where they are. Edits are undoable.")
                        .weak(),
                );
            });
        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_size();
            self.viewport = [avail.x.max(64.0) as u32, avail.y.max(64.0) as u32];
            if let Some(tex) = &self.texture {
                let size = tex.size_vec2();
                let scale = (avail.x / size.x).min(avail.y / size.y).min(8.0);
                let show = size * scale;
                ui.centered_and_justified(|ui| {
                    ui.add(egui::Image::new((tex.id(), show)).fit_to_exact_size(show));
                });
            } else {
                ui.centered_and_justified(|ui| {
                    ui.label("Drop a DNG, ARW, or JPEG here, or pass a path on the command line.");
                });
            }
        });

        if self.help {
            egui::Window::new("Keys")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("?  this list          q  quit");
                    ui.label("/  command palette    esc  close");
                    ui.label("+ / −  exposure 0.1    j / k  exposure 0.05");
                    ui.label("[ / ]  temperature 200 K    h / l  100 K");
                    ui.label("r  reset to import    \\  before / after");
                    ui.label("1–5 / 0  rating       p pick   x reject   u unflag");
                    ui.label("e  export 2048 px JPEG     ctrl-z  undo     ctrl-shift-z  redo");
                });
        }
        if let Some(code) = self.pair_code.clone() {
            let mut decision = None;
            egui::Window::new("Pair")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("The engine is showing this code. Confirm only if it matches.");
                    ui.heading(&code);
                    ui.horizontal(|ui| {
                        if ui.button("Confirm").clicked() {
                            decision = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            decision = Some(false);
                        }
                    });
                });
            if let Some(ok) = decision {
                let _ = self.link.pair_tx.send(ok);
                self.pair_code = None;
            }
        }
        if !self.engines.is_empty() && self.pair_code.is_none() {
            let mut pick = None;
            egui::Window::new("Engines")
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.label("Engines on this network");
                    for (label, addr) in &self.engines {
                        if ui.button(label).clicked() {
                            pick = Some(addr.clone());
                        }
                    }
                    if ui.button("This machine").clicked() {
                        pick = Some("local".into());
                    }
                });
            if let Some(addr) = pick {
                let _ = self.link.pick_tx.send(addr);
                self.engines.clear();
            }
        }

        if self.palette {
            let mut submit = false;
            egui::Window::new("Command")
                .anchor(egui::Align2::CENTER_TOP, [0.0, 80.0])
                .show(ctx, |ui| {
                    ui.label("Edits go through the engine. Undo is ctrl-z.");
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut self.palette_text)
                            .desired_width(520.0)
                            .hint_text("warm these portraits slightly and export 2048 px JPEGs"),
                    );
                    if self.palette_focus {
                        edit.request_focus();
                        self.palette_focus = false;
                    }
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        submit = true;
                    }
                    if let Some(plan) = plan_rules(&self.palette_text) {
                        ui.label(format!("rules: {}", plan.summary));
                    }
                    if ui.button("Run").clicked() {
                        submit = true;
                    }
                });
            if submit && !self.palette_text.trim().is_empty() {
                let text = self.palette_text.clone();
                let _ = self.link.tx.send(ToEngine::Plan { text });
                self.palette = false;
            }
        }

        self.grab_screenshot(ctx, frame);
    }
}

fn slider_flags(app: &mut App, response: &egui::Response) {
    if response.drag_started() {
        app.begin = true;
        app.commit = true;
    }
    if response.changed() {
        app.dirty = true;
        if response.drag_stopped() || !response.dragged() {
            app.end = true;
        }
    }
    if response.drag_stopped() {
        app.end = true;
        app.dirty = true;
        app.last_sent = Instant::now() - Duration::from_millis(20);
    }
    if response.changed() {
        // The proxy is applied on the next pump, before the engine replies.
        let ctx = response.ctx.clone();
        app.show_local_proxy(&ctx);
    }
}

impl App {
    fn grab_screenshot(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // The screenshot command captures the frame that just finished. Request on one update
        // and read on the next, after the palette window has actually been drawn.
        match self.shot {
            Shot::GrabDevelop | Shot::GrabPalette => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
                self.shot = match self.shot {
                    Shot::GrabDevelop => Shot::SaveDevelop,
                    _ => Shot::SavePalette,
                };
            }
            Shot::SaveDevelop | Shot::SavePalette => {
                let Some(image) = pending_screenshot(ctx) else {
                    return;
                };
                let Some(dir) = self.screenshot_dir.clone() else {
                    return;
                };
                let name = if self.shot == Shot::SaveDevelop {
                    "develop.png"
                } else {
                    "palette.png"
                };
                if let Err(err) = save_color(&dir, name, &image) {
                    self.note = err;
                }
                if self.shot == Shot::SaveDevelop {
                    self.palette = true;
                    self.palette_text =
                        "warm these portraits slightly and export 2048 px JPEGs".into();
                    self.shot = Shot::GrabPalette;
                } else {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    self.shot = Shot::Done;
                }
            }
            Shot::Off | Shot::WaitImage | Shot::Done => {}
        }
    }
}

fn pending_screenshot(ctx: &egui::Context) -> Option<std::sync::Arc<egui::ColorImage>> {
    ctx.input(|input| {
        input.events.iter().find_map(|event| match event {
            egui::Event::Screenshot { image, .. } => Some(std::sync::Arc::clone(image)),
            _ => None,
        })
    })
}

fn save_color(dir: &std::path::Path, name: &str, image: &egui::ColorImage) -> Result<(), String> {
    let _ = std::fs::create_dir_all(dir);
    let mut rgba = Vec::with_capacity(image.pixels.len() * 4);
    for px in &image.pixels {
        rgba.extend_from_slice(&px.to_array());
    }
    let path = dir.join(name);
    omalight_core::jpeg::write_png(&path, image.size[0] as u32, image.size[1] as u32, &rgba)
}

fn apply_theme(ctx: &egui::Context, theme: &Theme) {
    let mut visuals = egui::Visuals::dark();
    let bg = paint(theme.background);
    let fg = paint(theme.foreground);
    let accent = paint(theme.accent);
    visuals.panel_fill = bg;
    visuals.window_fill = paint(theme.darker_background);
    visuals.extreme_bg_color = paint(theme.darker_background);
    visuals.faint_bg_color = paint(theme.lighter_background);
    visuals.override_text_color = Some(fg);
    visuals.selection.bg_fill = accent;
    visuals.hyperlink_color = accent;
    visuals.widgets.noninteractive.fg_stroke.color = fg;
    visuals.widgets.inactive.fg_stroke.color = fg;
    visuals.widgets.hovered.fg_stroke.color = fg;
    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.fg_stroke.color = paint(theme.darker_background);
    ctx.set_visuals(visuals);
}

fn paint(color: Color) -> egui::Color32 {
    egui::Color32::from_rgb(color.r, color.g, color.b)
}

fn write_shot_fixture(w: usize, h: usize) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("omalight-shot");
    omalight_core::fixture::write_fixture(&dir, w, h)
}

fn print_help() {
    eprintln!(
        "\
omalight — photo window

  omalight [photo] [--connect HOST] [--local]
  omalight pair HOST [--yes] [--name NAME]
  omalight doctor [--connect HOST]
  omalight --headless --connect HOST:PORT --open FILE --export OUT
  omalight --screenshot DIR
  omalight skill --link

The window lists engines it finds on the network. The first connection shows a code
on both sides; confirm it once. Later connections use the saved device credential.
The path you open is read by the engine, so the file has to be on the engine machine."
    );
}

fn parse_gui(args: &[String]) -> Result<GuiOpts, String> {
    let mut connect = None;
    let mut token = None;
    let mut local = false;
    let mut screenshot = None;
    let mut open = None;
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
            "--screenshot" => {
                i += 1;
                screenshot = Some(PathBuf::from(
                    args.get(i)
                        .cloned()
                        .ok_or("--screenshot needs a directory")?,
                ));
            }
            other if other.starts_with('-') => return Err(format!("unknown argument `{other}`")),
            other => open = Some(PathBuf::from(other)),
        }
        i += 1;
    }
    let file = config::load_file()?;
    let host = connect.clone().or_else(|| {
        config::toml_str(&file, "host").map(|h| {
            let port = config::toml_i64(&file, "port").unwrap_or(7421);
            if h.contains(':') {
                h.to_string()
            } else {
                format!("{h}:{port}")
            }
        })
    });
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
    let fixture = if screenshot.is_some() && open.is_none() {
        Some((480, 320))
    } else {
        None
    };
    let mode = if screenshot.is_some() {
        ConnectMode::Local
    } else {
        mode
    };
    Ok(GuiOpts {
        link: LinkOpts {
            mode,
            remote: host,
            ai: config::ai_from_toml(&file),
            config_dir: config::config_dir(),
            credential: token,
        },
        open,
        screenshot_dir: screenshot,
        fixture,
    })
}

fn parse_headless(args: &[String]) -> Result<HeadlessArgs, String> {
    let mut connect = None;
    let mut token = None;
    let mut open = None;
    let mut export = None;
    let mut latency_json = None;
    let mut long_edge = 2048u32;
    let mut sets = BTreeMap::new();
    let mut config_dir = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--headless" => {}
            "--connect" => {
                i += 1;
                connect = Some(args.get(i).cloned().ok_or("--connect needs HOST:PORT")?);
            }
            "--token" => {
                i += 1;
                token = Some(args.get(i).cloned().ok_or("--token needs a value")?);
            }
            "--open" => {
                i += 1;
                open = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--open needs a path")?,
                ));
            }
            "--export" => {
                i += 1;
                export = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--export needs a path")?,
                ));
            }
            "--latency-json" => {
                i += 1;
                latency_json = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--latency-json needs a path")?,
                ));
            }
            "--long-edge" => {
                i += 1;
                long_edge = args
                    .get(i)
                    .ok_or("--long-edge needs a number")?
                    .parse()
                    .map_err(|e: std::num::ParseIntError| e.to_string())?;
            }
            "--set" => {
                i += 1;
                let kv = args.get(i).cloned().ok_or("--set needs control=value")?;
                let (k, v) = kv.split_once('=').ok_or("--set needs control=value")?;
                sets.insert(
                    k.to_string(),
                    v.parse()
                        .map_err(|e: std::num::ParseFloatError| e.to_string())?,
                );
            }
            "--config-dir" => {
                i += 1;
                config_dir = Some(PathBuf::from(
                    args.get(i).cloned().ok_or("--config-dir needs a path")?,
                ));
            }
            other => return Err(format!("unknown argument `{other}`")),
        }
        i += 1;
    }
    Ok(HeadlessArgs {
        connect: connect.ok_or("--headless needs --connect")?,
        token,
        open: open.ok_or("--headless needs --open")?,
        sets,
        export,
        long_edge,
        latency_json,
        config_dir: config_dir.unwrap_or_else(config::config_dir),
    })
}

fn link_skill(args: &[String]) -> Result<(), String> {
    if !args.iter().any(|a| a == "--link") && !args.is_empty() && args[0] != "--link" {
        // `omalight skill` prints where the skill lives. `--link` installs the symlinks.
    }
    let src = skill_source()?;
    println!("skill: {}", src.display());
    if args.iter().any(|a| a == "--link") {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or("HOME is unset")?;
        for rel in [
            "~/.agents/skills",
            "~/.claude/skills",
            "~/.codex/skills",
            "~/.pi/agent/skills",
        ] {
            let dir = home.join(rel.trim_start_matches("~/"));
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let dest = dir.join("omalight");
            let _ = std::fs::remove_file(&dest);
            std::os::unix::fs::symlink(&src, &dest)
                .map_err(|e| format!("link {}: {e}", dest.display()))?;
            println!("linked {}", dest.display());
        }
    }
    Ok(())
}

fn skill_source() -> Result<PathBuf, String> {
    if let Some(dir) = std::env::var_os("OMALIGHT_SKILL_DIR") {
        return Ok(PathBuf::from(dir));
    }
    for candidate in [
        "/usr/share/omalight/skills/omalight",
        "/usr/local/share/omalight/skills/omalight",
    ] {
        let path = PathBuf::from(candidate);
        if path.join("SKILL.md").is_file() {
            return Ok(path);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../skills/omalight");
    if dev.join("SKILL.md").is_file() {
        return Ok(dev);
    }
    Err("skill directory not found".into())
}
