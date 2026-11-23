//! First-run setup: the Rusted Mic logo, then "show me your game folder". The
//! chosen installation is validated and packed into `assets.pak` next to the
//! executable on a background thread, with a progress bar. Runs only when no
//! usable assets are found.

use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};

use funkin_assets::{import_installation, scan_common_locations, validate_installation, ImportMode, ImportProgress, Installation};
use winit::keyboard::KeyCode;

use crate::gfx::ttf::TtfFont;
use crate::gfx::{Color, Gfx, TexId, VIEW_H, VIEW_W};
use crate::logo;

const RUST: Color = [0.78, 0.36, 0.12, 1.0];
const RUST_HI: Color = [0.93, 0.50, 0.20, 1.0];
const STEEL: Color = [0.80, 0.81, 0.84, 1.0];
const DIM: Color = [0.62, 0.58, 0.55, 1.0];
const ERROR: Color = [1.0, 0.45, 0.38, 1.0];
/// The Funkin' release this port is based on.
pub const BASED_ON: &str = "0.8.6";

#[derive(Clone, Copy, PartialEq)]
enum Button {
    UseFound,
    Browse,
    Quit,
}

enum Msg {
    Progress(ImportProgress),
    Done(Result<PathBuf, String>),
}

enum Phase {
    Choose { error: Option<String> },
    Importing { rx: Receiver<Msg>, source: PathBuf, progress: Option<ImportProgress> },
}

/// What the app should do after a frame.
pub enum SetupAction {
    None,
    /// Assets are packed and mounted at this path.
    Ready(PathBuf),
    Quit,
}

pub struct SetupScreen {
    logo: TexId,
    backdrop: TexId,
    font: Option<TtfFont>,
    bold: Option<TtfFont>,
    phase: Phase,
    found: Option<Installation>,
    scan: Option<Receiver<Vec<Installation>>>,
    mouse: (f32, f32),
    time: f32,
}

fn system_ttf(names: &[&str]) -> Option<TtfFont> {
    let dir = std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts")).unwrap_or_else(|| PathBuf::from(r"C:\Windows\Fonts"));
    names.iter().find_map(|n| TtfFont::load(&dir.join(n)).ok())
}

impl SetupScreen {
    pub fn new(gfx: &mut Gfx, reason: Option<String>) -> Self {
        let logo = gfx.upload_rgba(logo::LOGO_W, logo::LOGO_H, &logo::render_logo());
        let backdrop = gfx.upload_rgba(320, 180, &logo::render_backdrop(320, 180));
        // Look for an installation in the usual places while the screen is up.
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(scan_common_locations());
        });
        let error = reason.filter(|r| !r.starts_with("No game assets"));
        Self {
            logo,
            backdrop,
            font: system_ttf(&["segoeui.ttf", "arial.ttf", "tahoma.ttf"]),
            bold: system_ttf(&["segoeuib.ttf", "arialbd.ttf", "tahomabd.ttf"]),
            phase: Phase::Choose { error },
            found: None,
            scan: Some(rx),
            mouse: (-1.0, -1.0),
            time: 0.0,
        }
    }

    fn buttons(&self) -> Vec<(Button, [f32; 4], String)> {
        if !matches!(self.phase, Phase::Choose { .. }) {
            return Vec::new();
        }
        let mut out = Vec::new();
        let y = 590.0;
        match &self.found {
            Some(_) => {
                out.push((Button::UseFound, [VIEW_W / 2.0 - 330.0, y, 320.0, 56.0], "Use this folder".to_owned()));
                out.push((Button::Browse, [VIEW_W / 2.0 + 10.0, y, 320.0, 56.0], "Choose another folder...".to_owned()));
            }
            None => out.push((Button::Browse, [VIEW_W / 2.0 - 180.0, y, 360.0, 56.0], "Choose game folder...".to_owned())),
        }
        out.push((Button::Quit, [VIEW_W - 130.0, VIEW_H - 56.0, 110.0, 38.0], "Quit".to_owned()));
        out
    }

    fn hovered(&self) -> Option<Button> {
        let (mx, my) = self.mouse;
        self.buttons().into_iter().find(|(_, r, _)| mx >= r[0] && mx <= r[0] + r[2] && my >= r[1] && my <= r[1] + r[3]).map(|(b, ..)| b)
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        self.mouse = (x, y);
    }

    pub fn click(&mut self) -> SetupAction {
        match self.hovered() {
            Some(b) => self.press(b),
            None => SetupAction::None,
        }
    }

    pub fn key(&mut self, code: KeyCode) -> SetupAction {
        if !matches!(self.phase, Phase::Choose { .. }) {
            return SetupAction::None;
        }
        match code {
            KeyCode::Enter | KeyCode::Space => self.press(if self.found.is_some() { Button::UseFound } else { Button::Browse }),
            KeyCode::KeyB | KeyCode::KeyO => self.press(Button::Browse),
            KeyCode::Escape => SetupAction::Quit,
            _ => SetupAction::None,
        }
    }

    fn press(&mut self, b: Button) -> SetupAction {
        match b {
            Button::Quit => return SetupAction::Quit,
            Button::UseFound => {
                if let Some(inst) = self.found.clone() {
                    self.start_import(inst.install_dir);
                }
            }
            Button::Browse => {
                #[cfg(feature = "dialog")]
                {
                    let picked = rfd::FileDialog::new()
                        .set_title("Select your Friday Night Funkin' folder (the one with Funkin.exe and assets)")
                        .pick_folder();
                    if let Some(dir) = picked {
                        self.start_import(dir);
                    }
                }
                #[cfg(not(feature = "dialog"))]
                {
                    self.phase = Phase::Choose { error: Some("This build has no folder picker: run `RustedMic setup --path <folder>`.".into()) };
                }
            }
        }
        SetupAction::None
    }

    fn start_import(&mut self, dir: PathBuf) {
        let inst = match validate_installation(&dir) {
            Ok(i) => i,
            Err(e) => {
                self.phase = Phase::Choose { error: Some(format!("That folder doesn't look like the game: {e}")) };
                return;
            }
        };
        let (tx, rx) = channel();
        let source = inst.install_dir.clone();
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let result = import_installation(&inst.install_dir, ImportMode::Pack, move |p| {
                let _ = progress_tx.send(Msg::Progress(p));
            });
            let _ = tx.send(Msg::Done(result.map(|m| m.asset_root).map_err(|e| e.to_string())));
        });
        self.phase = Phase::Importing { rx, source, progress: None };
    }

    pub fn update(&mut self, dt: f32) -> SetupAction {
        self.time += dt;
        if let Some(rx) = &self.scan {
            if let Ok(found) = rx.try_recv() {
                self.found = found.into_iter().next();
                self.scan = None;
            }
        }
        let mut done = None;
        if let Phase::Importing { rx, progress, .. } = &mut self.phase {
            while let Ok(m) = rx.try_recv() {
                match m {
                    Msg::Progress(p) => *progress = Some(p),
                    Msg::Done(r) => done = Some(r),
                }
            }
        }
        match done {
            Some(Ok(path)) => SetupAction::Ready(path),
            Some(Err(e)) => {
                self.phase = Phase::Choose { error: Some(format!("Packing failed: {e}")) };
                SetupAction::None
            }
            None => SetupAction::None,
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn text(&self, gfx: &mut Gfx, bold: bool, s: &str, cx: f32, y: f32, size: f32, color: Color) {
        let Some(f) = (if bold { self.bold.as_ref().or(self.font.as_ref()) } else { self.font.as_ref() }) else { return };
        let w = f.width(gfx, s, size);
        f.draw(gfx, s, cx - w / 2.0, y, size, color);
    }

    fn frame_rect(gfx: &mut Gfx, r: [f32; 4], t: f32, color: Color) {
        gfx.rect(r[0], r[1], r[2], t, color);
        gfx.rect(r[0], r[1] + r[3] - t, r[2], t, color);
        gfx.rect(r[0], r[1], t, r[3], color);
        gfx.rect(r[0] + r[2] - t, r[1], t, r[3], color);
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        gfx.begin([0.03, 0.02, 0.02, 1.0]);
        gfx.image(self.backdrop, [0.0, 0.0, 320.0, 180.0], 0.0, 0.0, VIEW_W, VIEW_H, [1.0; 4]);

        // Logo, gently bobbing in after launch.
        let k = (self.time / 0.6).min(1.0);
        let ease = 1.0 - (1.0 - k).powi(3);
        let s = 0.8;
        let (lw, lh) = (logo::LOGO_W as f32 * s, logo::LOGO_H as f32 * s);
        let bob = (self.time * 1.6).sin() * 4.0;
        gfx.image(self.logo, [0.0, 0.0, logo::LOGO_W as f32, logo::LOGO_H as f32], (VIEW_W - lw) / 2.0, 6.0 - 30.0 * (1.0 - ease) + bob, lw, lh, [1.0, 1.0, 1.0, ease]);

        let cx = VIEW_W / 2.0;
        // Credit line, on every phase of the screen.
        if let Some(f) = self.font.as_ref() {
            let credit = format!("Based on Funkin' v{BASED_ON}, the open-source game by The Funkin' Crew");
            f.draw(gfx, &credit, 20.0, VIEW_H - 44.0, 16.0, [0.5, 0.46, 0.43, 1.0]);
        }
        match &self.phase {
            Phase::Choose { error } => {
                self.text(gfx, true, "Let's set up your game", cx, 392.0, 34.0, STEEL);
                self.text(gfx, false, "Rusted Mic plays Friday Night Funkin' with the files from your own copy of the game.", cx, 440.0, 20.0, DIM);
                self.text(gfx, false, "Pick the game's main folder: the one with Funkin.exe and the assets folder inside.", cx, 468.0, 20.0, DIM);
                let status = match (&self.found, &self.scan) {
                    (Some(f), _) => format!("Found: {}  ({} songs)", f.install_dir.display(), f.song_count),
                    (None, Some(_)) => "Looking for an installation...".to_owned(),
                    (None, None) => "No installation found automatically.".to_owned(),
                };
                self.text(gfx, false, &status, cx, 516.0, 19.0, if self.found.is_some() { RUST_HI } else { DIM });
                if let Some(e) = error {
                    // Long errors: keep the first line, trimmed to fit.
                    let first = e.lines().next().unwrap_or("");
                    let mut line: String = first.chars().take(120).collect();
                    if first.chars().count() > 120 {
                        line.push_str("...");
                    }
                    self.text(gfx, false, &line, cx, 545.0, 18.0, ERROR);
                }
                let hover = self.hovered();
                for (b, r, label) in self.buttons() {
                    let primary = b == Button::UseFound || (b == Button::Browse && self.found.is_none());
                    let hot = hover == Some(b);
                    let fill = match (primary, hot) {
                        (true, true) => RUST_HI,
                        (true, false) => RUST,
                        (false, true) => [0.32, 0.27, 0.25, 1.0],
                        (false, false) => [0.20, 0.17, 0.16, 1.0],
                    };
                    gfx.rect(r[0], r[1], r[2], r[3], fill);
                    Self::frame_rect(gfx, r, 2.0, [0.0, 0.0, 0.0, 0.6]);
                    let size = if b == Button::Quit { 18.0 } else { 22.0 };
                    let ty = r[1] + (r[3] - size * 1.33) / 2.0;
                    self.text(gfx, true, &label, r[0] + r[2] / 2.0, ty, size, if primary { [1.0, 0.97, 0.93, 1.0] } else { STEEL });
                }
                self.text(gfx, false, "Enter: confirm    B: browse    Esc: quit", cx, 660.0, 16.0, [0.5, 0.46, 0.43, 1.0]);
            }
            Phase::Importing { source, progress, .. } => {
                self.text(gfx, true, "Packing your game files", cx, 392.0, 34.0, STEEL);
                self.text(gfx, false, &format!("From {}", source.display()), cx, 440.0, 19.0, DIM);
                self.text(gfx, false, "Everything goes into assets.pak next to Rusted Mic. This only happens once.", cx, 468.0, 20.0, DIM);
                let frac = progress.map(|p| p.files_done as f32 / p.files_total.max(1) as f32).unwrap_or(0.0);
                let bar = [cx - 360.0, 530.0, 720.0, 30.0];
                gfx.rect(bar[0], bar[1], bar[2], bar[3], [0.12, 0.10, 0.09, 1.0]);
                gfx.rect(bar[0] + 3.0, bar[1] + 3.0, (bar[2] - 6.0) * frac, bar[3] - 6.0, RUST);
                // A moving sheen on the filled part.
                let sheen = ((self.time * 0.8).fract() * (bar[2] - 6.0) * frac).max(0.0);
                gfx.rect(bar[0] + 3.0 + sheen, bar[1] + 3.0, 24.0f32.min((bar[2] - 6.0) * frac - sheen).max(0.0), bar[3] - 6.0, [1.0, 0.7, 0.45, 0.35]);
                Self::frame_rect(gfx, bar, 2.0, [0.0, 0.0, 0.0, 0.7]);
                let detail = match progress {
                    Some(p) => format!("{:.0}%   {} / {} files   {} MB", frac * 100.0, p.files_done, p.files_total, p.bytes_done / 1_048_576),
                    None => "Starting...".to_owned(),
                };
                self.text(gfx, false, &detail, cx, 574.0, 19.0, STEEL);
            }
        }
    }
}
