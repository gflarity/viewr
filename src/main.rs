//! viewr: a minimalist native image viewer. `viewr [DIR]` shows the JPEG/PNG/NEF images
//! in a directory; flip through them with the arrow keys.

mod files;
mod loader;
mod nef;
mod orient;
// Vendored TIFF reader (see ATTRIBUTION.md): only the preview-extraction subset is used.
#[allow(dead_code)]
mod tiff;

use std::collections::HashMap;
use std::path::PathBuf;

use eframe::egui;

fn main() -> eframe::Result<()> {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse_args(argv) {
        Ok(Action::Help) => {
            print!("{}", usage());
            return Ok(());
        }
        Ok(Action::Run(args)) => args,
        Err(e) => {
            eprintln!("viewr: {e}\n{}", usage());
            std::process::exit(2);
        }
    };
    let files = match files::collect(&args.dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("viewr: {}: {e}", args.dir.display());
            std::process::exit(1);
        }
    };
    let viewer = Viewer::new(files, args.dir, args.on_key);
    let opts = eframe::NativeOptions { viewport: egui::ViewportBuilder::default().with_inner_size([1100.0, 750.0]), ..Default::default() };
    eframe::run_native("viewr", opts, Box::new(|_cc| Ok(Box::new(viewer) as Box<dyn eframe::App>)))
}

fn usage() -> &'static str {
    "usage: viewr [DIR] [--on-key PROG]\n\
     \n\
     Minimalist native image viewer (JPEG, PNG, Nikon NEF previews).\n\
     \n\
     \x20 DIR            directory of images to view (default: current directory)\n\
     \x20 --on-key PROG  run `PROG <file> <key>` on every handled keypress\n\
     \x20 -h, --help     this help\n\
     \n\
     Keys: Left/Right (H/L) navigate, Home/End jump, +/- zoom, 0 fit, 1 actual\n\
     size, F fullscreen, Q quit, Esc exits fullscreen (or quits). Pinch/spread\n\
     on a trackpad zooms toward the pointer.\n"
}

struct Args {
    dir: PathBuf,
    on_key: Option<String>,
}

enum Action {
    Run(Args),
    Help,
}

fn parse_args(argv: Vec<String>) -> Result<Action, String> {
    let mut dir: Option<PathBuf> = None;
    let mut on_key: Option<String> = None;
    let mut i = 0;
    while i < argv.len() {
        let a = argv[i].as_str();
        if let Some(prog) = a.strip_prefix("--on-key=") {
            if prog.is_empty() {
                return Err("--on-key needs a program".to_string());
            }
            on_key = Some(prog.to_string());
        } else if a == "--on-key" {
            i += 1;
            match argv.get(i) {
                Some(prog) => on_key = Some(prog.clone()),
                None => return Err("--on-key needs a program".to_string()),
            }
        } else if a == "-h" || a == "--help" {
            return Ok(Action::Help);
        } else if a.starts_with('-') {
            return Err(format!("unknown flag `{a}`"));
        } else if dir.is_none() {
            dir = Some(PathBuf::from(a));
        } else {
            return Err("only one directory is accepted".to_string());
        }
        i += 1;
    }
    let dir = dir.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    Ok(Action::Run(Args { dir, on_key }))
}

enum Slot {
    Loading,
    Ready { tex: egui::TextureHandle, w: u32, h: u32 },
    Failed(String),
}

enum Frame {
    Loading,
    Ready { tex: egui::TextureHandle, w: u32, h: u32 },
    Failed(String),
}

/// Idle frames after a pinch before the overshoot spring starts, so a slow gesture with gaps
/// between events doesn't bounce back mid-pinch (~130ms at 60fps).
const SPRING_DELAY_FRAMES: u32 = 8;

struct Viewer {
    files: Vec<PathBuf>,
    dir: PathBuf,
    index: usize,
    loader: loader::Loader,
    cache: HashMap<usize, Slot>,
    /// Multiplier on the fit-to-window scale (1 = fit).
    zoom: f32,
    offset: egui::Vec2,
    /// Zoom spring velocity for the bounce-back after pinching past the minimum zoom.
    spring_v: f32,
    /// Consecutive frames with no pinch, to tell a released gesture from a slow one.
    still_frames: u32,
    /// Whether the overshoot spring is mid-flight (it runs past the minimum while bouncing).
    springing: bool,
    last_avail: egui::Vec2,
    fullscreen: bool,
    on_key: Option<String>,
    last_title: String,
}

impl Viewer {
    fn new(files: Vec<PathBuf>, dir: PathBuf, on_key: Option<String>) -> Self {
        let loader = loader::Loader::spawn(files.clone());
        let mut v = Self {
            files,
            dir,
            index: 0,
            loader,
            cache: HashMap::new(),
            zoom: 1.0,
            offset: egui::Vec2::ZERO,
            spring_v: 0.0,
            still_frames: 0,
            springing: false,
            last_avail: egui::vec2(1100.0, 750.0),
            fullscreen: false,
            on_key,
            last_title: String::new(),
        };
        v.request_around();
        v
    }

    /// Ask the loader for the current image plus its neighbours (for instant paging).
    fn request_around(&mut self) {
        let n = self.files.len();
        if n == 0 {
            return;
        }
        let mut idx = vec![self.index, (self.index + 1) % n, (self.index + n - 1) % n];
        idx.dedup();
        idx.retain(|i| !matches!(self.cache.get(i), Some(Slot::Ready { .. }) | Some(Slot::Failed(_))));
        for i in &idx {
            self.cache.entry(*i).or_insert(Slot::Loading);
        }
        if !idx.is_empty() {
            self.loader.request(idx);
        }
    }

    fn goto(&mut self, index: usize) {
        if self.files.is_empty() || index == self.index {
            return;
        }
        self.index = index;
        self.zoom = 1.0;
        self.offset = egui::Vec2::ZERO;
        self.spring_v = 0.0;
        self.still_frames = 0;
        self.springing = false;
        let (at, n) = (self.index, self.files.len());
        self.cache.retain(|i, _| files::circular_distance(*i, at, n) <= 1);
        self.request_around();
    }

    fn step(&mut self, delta: isize) {
        let n = self.files.len();
        if n == 0 {
            return;
        }
        self.goto((self.index as isize + delta).rem_euclid(n as isize) as usize);
    }

    fn set_fullscreen(&mut self, on: bool, ctx: &egui::Context) {
        self.fullscreen = on;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        use egui::Key as K;
        const BINDS: &[(K, &str)] = &[
            (K::ArrowLeft, "ArrowLeft"),
            (K::ArrowRight, "ArrowRight"),
            (K::H, "h"),
            (K::L, "l"),
            (K::Home, "Home"),
            (K::End, "End"),
            (K::Plus, "+"),
            (K::Minus, "-"),
            (K::Num0, "0"),
            (K::Num1, "1"),
            (K::F, "f"),
            (K::Q, "q"),
            (K::Escape, "Escape"),
        ];
        let mut fired: Vec<&'static str> = Vec::new();
        ctx.input(|i| {
            for (k, label) in BINDS {
                if i.key_pressed(*k) {
                    fired.push(label);
                }
            }
        });
        for key in fired {
            self.on_key(key, ctx);
        }
    }

    fn on_key(&mut self, key: &str, ctx: &egui::Context) {
        let has_files = !self.files.is_empty();
        match key {
            "q" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "Escape" if self.fullscreen => self.set_fullscreen(false, ctx),
            "Escape" => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            "f" if has_files => self.set_fullscreen(!self.fullscreen, ctx),
            "ArrowLeft" | "h" if has_files => self.step(-1),
            "ArrowRight" | "l" if has_files => self.step(1),
            "Home" if has_files => self.goto(0),
            "End" if has_files => self.goto(self.files.len() - 1),
            "+" if has_files => {
                self.zoom = (self.zoom * 1.25).clamp(self.current_min_zoom(), 64.0);
                self.spring_v = 0.0;
                self.springing = false;
            }
            "-" if has_files => {
                self.zoom = (self.zoom / 1.25).clamp(self.current_min_zoom(), 64.0);
                self.spring_v = 0.0;
                self.springing = false;
            }
            "0" if has_files => {
                self.zoom = 1.0;
                self.offset = egui::Vec2::ZERO;
                self.spring_v = 0.0;
                self.springing = false;
            }
            "1" if has_files => self.actual_size(),
            _ => return,
        }
        if has_files && let (Some(prog), Some(file)) = (&self.on_key, self.files.get(self.index)) {
            fire_hook(prog, file, key);
        }
    }

    fn actual_size(&mut self) {
        if let Some(Slot::Ready { w, h, .. }) = self.cache.get(&self.index) {
            let fit = (self.last_avail.x / *w as f32).min(self.last_avail.y / *h as f32);
            if fit > 0.0 && fit.is_finite() {
                self.zoom = (1.0 / fit).min(64.0);
            }
            self.offset = egui::Vec2::ZERO;
            self.spring_v = 0.0;
            self.springing = false;
        }
    }

    /// Lowest useful zoom: fit for images larger than the viewport, actual size for smaller ones.
    fn min_zoom_for(w: u32, h: u32, avail: egui::Vec2) -> f32 {
        if w == 0 || h == 0 {
            return 1.0;
        }
        let fit = (avail.x / w as f32).min(avail.y / h as f32);
        if fit > 0.0 && fit.is_finite() { 1.0f32.min(1.0 / fit) } else { 1.0 }
    }

    fn current_min_zoom(&self) -> f32 {
        match self.cache.get(&self.index) {
            Some(Slot::Ready { w, h, .. }) => Self::min_zoom_for(*w, *h, self.last_avail),
            _ => 1.0,
        }
    }

    /// Move decoded images from the loader into GPU textures (`Ready` slots).
    fn poll_loader(&mut self, ctx: &egui::Context) {
        for loaded in self.loader.res.try_iter() {
            let slot = match loaded.result {
                Ok(d) => {
                    let img = egui::ColorImage::from_rgba_unmultiplied([d.width as usize, d.height as usize], &d.rgba);
                    let name = self.files.get(loaded.index).map(|p| p.display().to_string()).unwrap_or_default();
                    Slot::Ready { tex: ctx.load_texture(name, img, egui::TextureOptions::LINEAR), w: d.width, h: d.height }
                }
                Err(e) => Slot::Failed(e),
            };
            self.cache.insert(loaded.index, slot);
        }
    }

    /// Pan by a drag delta, keeping the image from drifting away from the viewport.
    fn apply_pan(&mut self, delta: egui::Vec2, img_size: egui::Vec2) {
        self.offset += delta;
        self.clamp_offset(img_size);
    }

    /// This frame's trackpad pinch, if any: fluid zoom toward the pointer. Returns whether a pinch is active.
    fn poll_pinch(&mut self, ctx: &egui::Context, fit_size: egui::Vec2, view_center: egui::Pos2, min_zoom: f32) -> bool {
        let (delta, hover) = ctx.input(|i| (i.zoom_delta(), i.pointer.hover_pos()));
        if delta != 1.0 {
            self.apply_pinch(delta, fit_size, view_center, hover, min_zoom);
            true
        } else {
            false
        }
    }

    /// Trackpad pinch: fluid multiplicative zoom that keeps the image point under the pointer
    /// fixed (the viewport center when the pointer position is unknown). Past the minimum zoom
    /// the image rubber-bands with diminishing response; releasing lets it spring back.
    fn apply_pinch(&mut self, factor: f32, fit_size: egui::Vec2, view_center: egui::Pos2, hover: Option<egui::Pos2>, min_zoom: f32) {
        if factor == 1.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        self.spring_v = 0.0;
        self.springing = false;
        let old = self.zoom;
        let target = old * factor;
        let new = if target < min_zoom { min_zoom + (target - min_zoom) * 0.55 } else { target.min(64.0) };
        let p = hover.unwrap_or(view_center);
        let c = view_center + self.offset;
        self.zoom = new;
        self.offset = p + (c - p) * (new / old) - view_center;
        self.clamp_offset(fit_size * new);
    }

    /// Advance the overshoot spring once the gesture has been still for a few frames, so a slow
    /// pinch with idle frames between events doesn't bounce back mid-gesture. Returns true while
    /// frames must keep coming (countdown or bounce in progress).
    fn poll_spring(&mut self, pinched: bool, dragged: bool, min_zoom: f32) -> bool {
        if pinched {
            self.still_frames = 0;
            return self.zoom < min_zoom;
        }
        self.still_frames = self.still_frames.saturating_add(1);
        if self.still_frames >= SPRING_DELAY_FRAMES && !dragged && (self.zoom < min_zoom || self.springing) {
            self.springing = !self.spring_back(min_zoom);
        }
        self.springing || self.zoom < min_zoom
    }

    /// Ease an overshot zoom back to the minimum with a slight bounce past it. Returns true once settled.
    fn spring_back(&mut self, min_zoom: f32) -> bool {
        self.spring_v += (min_zoom - self.zoom) * 0.25;
        self.spring_v *= 0.6;
        self.zoom += self.spring_v;
        self.offset *= 0.6;
        if (min_zoom - self.zoom).abs() < 0.001 && self.spring_v.abs() < 0.001 {
            self.zoom = min_zoom;
            self.offset = egui::Vec2::ZERO;
            self.spring_v = 0.0;
            true
        } else {
            false
        }
    }

    fn clamp_offset(&mut self, img_size: egui::Vec2) {
        self.offset.x = self.offset.x.clamp(-(img_size.x / 2.0 + 200.0), img_size.x / 2.0 + 200.0);
        self.offset.y = self.offset.y.clamp(-(img_size.y / 2.0 + 200.0), img_size.y / 2.0 + 200.0);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.files.get(self.index) {
            Some(path) => {
                let name = path.file_name().map(|s| s.to_string_lossy()).unwrap_or_default();
                match self.cache.get(&self.index) {
                    Some(Slot::Ready { w, h, .. }) => {
                        let fit = (self.last_avail.x / *w as f32).min(self.last_avail.y / *h as f32);
                        let pct = (fit * self.zoom * 100.0).round().max(1.0);
                        format!("{name} — {w}×{h} @ {pct:.0}% ({} of {})", self.index + 1, self.files.len())
                    }
                    Some(Slot::Failed(_)) => format!("{name} — failed"),
                    _ => format!("{name} — loading…"),
                }
            }
            None => "viewr".to_string(),
        };
        if title != self.last_title {
            self.last_title = title.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
    }
}

fn fire_hook(prog: &str, file: &std::path::Path, key: &str) {
    let _ = std::process::Command::new(prog)
        .arg(file)
        .arg(key)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

impl eframe::App for Viewer {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_loader(ctx);
        self.handle_keys(ctx);
        if !self.files.is_empty() && !matches!(self.cache.get(&self.index), Some(Slot::Ready { .. }) | Some(Slot::Failed(_))) {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        self.update_title(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(ui, |ui| {
            if self.files.is_empty() {
                ui.centered_and_justified(|ui| ui.label(format!("No supported images in {}", self.dir.display())));
                return;
            }
            let frame = match self.cache.get(&self.index) {
                Some(Slot::Ready { tex, w, h }) => Frame::Ready { tex: tex.clone(), w: *w, h: *h },
                Some(Slot::Failed(e)) => Frame::Failed(e.clone()),
                _ => Frame::Loading,
            };
            match frame {
                Frame::Loading => {
                    ui.centered_and_justified(|ui| ui.spinner());
                }
                Frame::Failed(e) => {
                    ui.centered_and_justified(|ui| ui.label(format!("Could not load this image:\n{e}")));
                }
                Frame::Ready { tex, w, h } => {
                    let avail = ui.available_size();
                    self.last_avail = avail;
                    let fit = (avail.x / w as f32).min(avail.y / h as f32);
                    let min_zoom = Self::min_zoom_for(w, h, avail);
                    let (rect, resp) = ui.allocate_exact_size(avail, egui::Sense::drag());
                    let pinched = self.poll_pinch(ui.ctx(), egui::vec2(w as f32, h as f32) * fit, rect.center(), min_zoom);
                    if self.poll_spring(pinched, resp.dragged(), min_zoom) {
                        ui.ctx().request_repaint();
                    }
                    let scale = fit * self.zoom;
                    let size = egui::vec2(w as f32 * scale, h as f32 * scale);
                    if resp.dragged() {
                        self.apply_pan(resp.drag_delta(), size);
                    }
                    let rect = egui::Rect::from_center_size(rect.center() + self.offset, size);
                    ui.put(rect, egui::Image::new(&tex).fit_to_exact_size(size));
                }
            }
        });
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;

    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_positional_and_defaults() {
        let Action::Run(a) = parse_args(args(&[])).unwrap() else { panic!("expected Run") };
        assert_eq!(a.dir, std::env::current_dir().unwrap());
        assert!(a.on_key.is_none());
        let Action::Run(a) = parse_args(args(&["pics"])).unwrap() else { panic!("expected Run") };
        assert_eq!(a.dir, PathBuf::from("pics"));
    }

    #[test]
    fn parses_on_key_forms() {
        let Action::Run(a) = parse_args(args(&["--on-key", "hook.sh", "pics"])).unwrap() else { panic!("expected Run") };
        assert_eq!(a.on_key.as_deref(), Some("hook.sh"));
        assert_eq!(a.dir, PathBuf::from("pics"));
        let Action::Run(a) = parse_args(args(&["--on-key=hook.sh"])).unwrap() else { panic!("expected Run") };
        assert_eq!(a.on_key.as_deref(), Some("hook.sh"));
    }

    #[test]
    fn rejects_bad_args() {
        assert!(parse_args(args(&["--on-key"])).is_err());
        assert!(parse_args(args(&["--on-key="])).is_err());
        assert!(parse_args(args(&["--bogus"])).is_err());
        assert!(parse_args(args(&["a", "b"])).is_err());
    }

    #[test]
    fn help_flag() {
        assert!(matches!(parse_args(args(&["-h"])).unwrap(), Action::Help));
        assert!(matches!(parse_args(args(&["--help"])).unwrap(), Action::Help));
        assert!(usage().contains("usage: viewr"));
    }
}

/// Headless behaviour tests: the real viewer state machine (navigation, zoom, pan, fullscreen,
/// key hook, decode-to-texture) driven through headless egui logic passes — no display needed.
#[cfg(test)]
mod state_tests {
    use super::*;
    use std::path::Path;

    fn write_png(path: &Path, w: u32, h: u32) {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb([x as u8, y as u8, 0]));
        img.save(path).unwrap();
    }

    fn fixture_dir(name: &str, specs: &[(u32, u32)]) -> (PathBuf, Vec<PathBuf>) {
        let dir = std::env::temp_dir().join(format!("viewr-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut files = Vec::new();
        for (i, (w, h)) in specs.iter().enumerate() {
            let p = dir.join(format!("img{i}.png"));
            write_png(&p, *w, *h);
            files.push(p);
        }
        (dir, files)
    }

    /// One key through the viewer's handler inside a headless logic pass; returns the pass output
    /// (viewport commands included).
    fn press(v: &mut Viewer, key: &'static str) -> egui::LogicOutput {
        let ctx = egui::Context::default();
        let input = egui::RawInput::default();
        ctx.run_logic(&input, |ctx| v.on_key(key, ctx))
    }

    fn has_cmd(out: &egui::LogicOutput, want: egui::ViewportCommand) -> bool {
        out.viewport_commands.values().any(|cmds| cmds.contains(&want))
    }

    fn ready_slot(w: u32, h: u32) -> Slot {
        let ctx = egui::Context::default();
        let rgba = vec![255u8; w as usize * h as usize * 4];
        let img = egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &rgba);
        Slot::Ready { tex: ctx.load_texture("t", img, egui::TextureOptions::LINEAR), w, h }
    }

    #[test]
    fn arrows_hl_home_end_navigate_and_wrap() {
        let (_dir, files) = fixture_dir("nav", &[(8, 8), (8, 8), (8, 8)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        assert_eq!(v.index, 0);
        press(&mut v, "ArrowRight");
        assert_eq!(v.index, 1);
        press(&mut v, "l");
        assert_eq!(v.index, 2);
        press(&mut v, "ArrowRight");
        assert_eq!(v.index, 0, "wraps forward");
        press(&mut v, "ArrowLeft");
        assert_eq!(v.index, 2, "wraps back");
        press(&mut v, "h");
        assert_eq!(v.index, 1);
        press(&mut v, "Home");
        assert_eq!(v.index, 0);
        press(&mut v, "End");
        assert_eq!(v.index, 2);
        // navigating resets zoom and pan
        v.zoom = 3.0;
        v.offset = egui::vec2(10.0, 10.0);
        press(&mut v, "ArrowLeft");
        assert_eq!(v.index, 1);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, egui::Vec2::ZERO);
    }

    #[test]
    fn zoom_keys_scale_reset_and_stop_at_fit() {
        let (_dir, files) = fixture_dir("zoom", &[(8, 8)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        // a large image: zooming out stops at fit instead of shrinking past it
        v.cache.insert(0, ready_slot(1600, 1200));
        v.last_avail = egui::vec2(1100.0, 750.0);
        press(&mut v, "+");
        assert_eq!(v.zoom, 1.25);
        press(&mut v, "-");
        assert!((v.zoom - 1.0).abs() < 1e-6, "zoom = {}", v.zoom);
        for _ in 0..100 {
            press(&mut v, "+");
        }
        assert_eq!(v.zoom, 64.0);
        for _ in 0..100 {
            press(&mut v, "-");
        }
        assert_eq!(v.zoom, 1.0, "zoom-out stops at fit");
        v.offset = egui::vec2(5.0, 5.0);
        press(&mut v, "0");
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, egui::Vec2::ZERO);
    }

    #[test]
    fn zoom_out_stops_at_actual_size_for_small_images() {
        let (_dir, files) = fixture_dir("zoom-small", &[(8, 8)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        v.cache.insert(0, ready_slot(200, 100));
        v.last_avail = egui::vec2(1100.0, 750.0);
        // fit = 5.5, so the floor is actual size: 1/5.5
        for _ in 0..100 {
            press(&mut v, "-");
        }
        assert!((v.zoom * 5.5 - 1.0).abs() < 1e-6, "zoom = {}", v.zoom);
    }

    #[test]
    fn min_zoom_is_fit_or_actual_size() {
        assert_eq!(Viewer::min_zoom_for(6048, 4032, egui::vec2(1100.0, 750.0)), 1.0);
        assert!((Viewer::min_zoom_for(200, 100, egui::vec2(1100.0, 750.0)) * 5.5 - 1.0).abs() < 1e-6);
        assert_eq!(Viewer::min_zoom_for(0, 100, egui::vec2(1100.0, 750.0)), 1.0);
    }

    #[test]
    fn actual_size_matches_screen_pixels() {
        let (_dir, files) = fixture_dir("actual", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        // pretend the loader delivered a 200x100 texture into an 1100x750 viewport
        let ctx = egui::Context::default();
        let rgba = vec![255u8; 200 * 100 * 4];
        let img = egui::ColorImage::from_rgba_unmultiplied([200, 100], &rgba);
        let tex = ctx.load_texture("t", img, egui::TextureOptions::LINEAR);
        v.cache.insert(0, Slot::Ready { tex, w: 200, h: 100 });
        v.last_avail = egui::vec2(1100.0, 750.0);
        press(&mut v, "1");
        // fit = min(1100/200, 750/100) = 5.5, so zoom must be 1/5.5 for a scale of 1.0
        assert!((v.zoom * 5.5 - 1.0).abs() < 1e-6, "zoom = {}", v.zoom);
        assert_eq!(v.offset, egui::Vec2::ZERO);
    }

    #[test]
    fn fullscreen_toggle_quit_and_escape_emit_commands() {
        let (_dir, files) = fixture_dir("fs", &[(8, 8)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        let out = press(&mut v, "f");
        assert!(v.fullscreen);
        assert!(has_cmd(&out, egui::ViewportCommand::Fullscreen(true)));
        // Escape leaves fullscreen instead of quitting
        let out = press(&mut v, "Escape");
        assert!(!v.fullscreen);
        assert!(has_cmd(&out, egui::ViewportCommand::Fullscreen(false)));
        assert!(!has_cmd(&out, egui::ViewportCommand::Close));
        // Escape outside fullscreen quits, as does Q
        let out = press(&mut v, "Escape");
        assert!(has_cmd(&out, egui::ViewportCommand::Close));
        let out = press(&mut v, "q");
        assert!(has_cmd(&out, egui::ViewportCommand::Close));
    }

    #[test]
    fn pan_accumulates_and_clamps() {
        let (_dir, files) = fixture_dir("pan", &[(8, 8)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        v.apply_pan(egui::vec2(30.0, -40.0), egui::vec2(100.0, 100.0));
        assert_eq!(v.offset, egui::vec2(30.0, -40.0));
        v.apply_pan(egui::vec2(1e6, 1e6), egui::vec2(100.0, 100.0));
        assert_eq!(v.offset, egui::vec2(250.0, 250.0), "clamped to half size + 200px");
    }

    #[cfg(unix)]
    #[test]
    fn key_hook_receives_file_and_key() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, files) = fixture_dir("hook", &[(8, 8)]);
        let log = dir.join("hook.log");
        let script = dir.join("hook.sh");
        std::fs::write(&script, format!("#!/bin/sh\necho \"$1|$2\" >> \"{}\"\n", log.display())).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut v = Viewer::new(files.clone(), dir, Some(script.to_str().unwrap().to_string()));
        press(&mut v, "ArrowRight");
        let mut body = String::new();
        for _ in 0..100 {
            body = std::fs::read_to_string(&log).unwrap_or_default();
            if !body.is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(body.trim(), format!("{}|ArrowRight", files[0].display()));
    }

    #[test]
    fn loader_delivers_textures_with_true_dimensions() {
        let (_dir, files) = fixture_dir("tex", &[(32, 16), (16, 48)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        let ctx = egui::Context::default();
        let t = std::time::Instant::now();
        let dims = loop {
            v.poll_loader(&ctx);
            match (v.cache.get(&0), v.cache.get(&1)) {
                (Some(Slot::Ready { w: w0, h: h0, .. }), Some(Slot::Ready { w: w1, h: h1, .. })) => break [(*w0, *h0), (*w1, *h1)],
                _ if t.elapsed() > std::time::Duration::from_secs(10) => panic!("loader never delivered"),
                _ => std::thread::sleep(std::time::Duration::from_millis(10)),
            }
        };
        assert_eq!(dims, [(32, 16), (16, 48)]);
    }

    #[test]
    fn pinch_zooms_fluidly_toward_pointer() {
        let (_dir, files) = fixture_dir("pinch", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        // viewport center (500, 375); pointer 100px right of the image center
        v.apply_pinch(2.0, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), Some(egui::pos2(600.0, 375.0)), 1.0);
        assert_eq!(v.zoom, 2.0);
        // the scale doubled, so the center shifts left by the pointer's old offset to keep it fixed
        assert_eq!(v.offset, egui::vec2(-100.0, 0.0));
        // pinching back out restores the center
        v.apply_pinch(0.5, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), Some(egui::pos2(600.0, 375.0)), 1.0);
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, egui::Vec2::ZERO);
    }

    #[test]
    fn pinch_clamps_at_max_and_rubber_bands_past_min() {
        let (_dir, files) = fixture_dir("pinch-edge", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        // no pointer: zooms about the viewport center, offset untouched
        v.apply_pinch(1.5, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), None, 1.0);
        assert_eq!(v.zoom, 1.5);
        assert_eq!(v.offset, egui::Vec2::ZERO);
        // hard stop at the maximum ...
        v.apply_pinch(1e6, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), None, 1.0);
        assert_eq!(v.zoom, 64.0);
        // ... but diminishing resistance past the minimum: 1 + (64e-9 - 1) * 0.55
        v.apply_pinch(1e-9, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), None, 1.0);
        assert!((v.zoom - 0.45).abs() < 1e-6, "zoom = {}", v.zoom);
        // degenerate deltas are ignored
        for f in [1.0, 0.0, -2.0, f32::NAN, f32::INFINITY] {
            let (z, o) = (v.zoom, v.offset);
            v.apply_pinch(f, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), Some(egui::pos2(0.0, 0.0)), 1.0);
            assert_eq!((v.zoom, v.offset), (z, o), "factor {f}");
        }
    }

    #[test]
    fn pinch_overshoot_springs_back_with_bounce() {
        let (_dir, files) = fixture_dir("bounce", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        v.apply_pinch(0.5, egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), Some(egui::pos2(600.0, 375.0)), 1.0);
        assert!((v.zoom - 0.725).abs() < 1e-6, "rubber-banded zoom = {}", v.zoom);
        let mut max = v.zoom;
        let mut settled = false;
        for _ in 0..200 {
            settled = v.spring_back(1.0);
            max = max.max(v.zoom);
            if settled {
                break;
            }
        }
        assert!(settled, "spring must settle");
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, egui::Vec2::ZERO);
        assert_eq!(v.spring_v, 0.0);
        assert!(max > 1.0 && max < 1.15, "a bit of bounce, max = {max}");
    }

    #[test]
    fn spring_waits_for_gesture_to_end() {
        let (_dir, files) = fixture_dir("debounce", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        v.zoom = 0.8;
        // active pinching never springs, however long it lasts
        for _ in 0..20 {
            v.poll_spring(true, false, 1.0);
            assert_eq!(v.zoom, 0.8);
        }
        // ...nor do the first idle frames after release
        for _ in 0..SPRING_DELAY_FRAMES - 1 {
            v.poll_spring(false, false, 1.0);
            assert_eq!(v.zoom, 0.8);
        }
        // then the bounce runs to completion on its own frames
        for _ in 0..200 {
            v.poll_spring(false, false, 1.0);
        }
        assert_eq!(v.zoom, 1.0);
        assert_eq!(v.offset, egui::Vec2::ZERO);
        assert_eq!(v.spring_v, 0.0);
    }

    #[test]
    fn spread_and_pinch_gestures_drive_fluid_anchored_zoom() {
        let (_dir, files) = fixture_dir("gesture", &[(200, 100)]);
        let mut v = Viewer::new(files, PathBuf::from("."), None);
        let ctx = egui::Context::default();
        let (img, center, hover) = (egui::vec2(200.0, 100.0), egui::pos2(500.0, 375.0), egui::pos2(600.0, 375.0));

        // one headless frame carrying authentic trackpad-style input: a pointer position and
        // the Zoom event the backend synthesizes from a pinch gesture
        let frame = |v: &mut Viewer, factor: f32| {
            let input = egui::RawInput { events: vec![egui::Event::PointerMoved(hover), egui::Event::Zoom(factor)], ..Default::default() };
            // headless: no backend consumes the font-texture upload, so drain it instead of dropping it
            ctx.run_ui(input, |ui| _ = v.poll_pinch(ui.ctx(), img, center, 1.0)).textures_delta.clear();
        };

        // spread: 30 fluid frames of x1.02 compound without jumps
        let t = std::time::Instant::now();
        let mut last = v.zoom;
        for _ in 0..30 {
            frame(&mut v, 1.02);
            assert!(v.zoom > last, "zoom must grow every spread frame");
            assert!((v.zoom / last - 1.02).abs() < 1e-4, "each step compounds the gesture factor");
            last = v.zoom;
        }
        let z = 1.02f32.powi(30);
        assert!((v.zoom - z).abs() < 1e-3, "zoom = {}, want {z}", v.zoom);
        // pointer-anchored: offset matches the closed form (center - hover) * (Z - 1)
        let want = (center - hover) * (z - 1.0);
        assert!((v.offset.x - want.x).abs() < 0.5, "offset = {:?}, want {want:?}", v.offset);
        assert_eq!(v.offset.y, 0.0);

        // pinch back out: 30 frames of /1.02 return to fit, recentered
        for _ in 0..30 {
            frame(&mut v, 1.0 / 1.02);
        }
        assert!((v.zoom - 1.0).abs() < 1e-3, "zoom = {}", v.zoom);
        assert!(v.offset.length() < 0.5, "offset = {:?}", v.offset);
        assert!(t.elapsed() < std::time::Duration::from_secs(5), "60 gesture frames took {:?}", t.elapsed());
    }
}
