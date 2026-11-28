//! Port of `CreditsState`: the credits scroll.
//!
//! The base game builds its credits into `Funkin.exe` at compile time
//! (`HARDCODED_CREDITS`), so they aren't in the assets: setup recovers them
//! from the user's executable as `data/credits.json` (see
//! `funkin_assets::credits`). Like a non-hardcoded build, this reads that file
//! (the same format, so mods can replace it), and falls back to
//! `CreditsDataHandler.getFallback()`.

use serde::Deserialize;
use winit::keyboard::KeyCode;

use crate::assets::Assets;
use crate::audio::Audio;
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Gfx, VIEW_H, VIEW_W};

use super::{MenuAction, Music};

const SCREEN_PAD: f32 = 24.0;
const FULL_WIDTH: f32 = VIEW_W - SCREEN_PAD * 2.0;
const FONT_SIZE: f32 = 24.0;
const HEADER_FONT_SIZE: f32 = 32.0;
const SCROLL_BASE_SPEED: f32 = 100.0;
const SCROLL_FAST_SPEED: f32 = SCROLL_BASE_SPEED * 4.0;

#[derive(Deserialize)]
struct CreditsData {
    entries: Vec<Role>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Role {
    header: Option<String>,
    #[serde(default)]
    body: Vec<Member>,
    #[allow(dead_code)]
    #[serde(default)]
    append_backers: bool,
}

#[derive(Deserialize)]
struct Member {
    line: String,
}

fn fallback() -> CreditsData {
    CreditsData {
        entries: vec![Role {
            header: Some("Founders".into()),
            body: ["ninjamuffin99", "PhantomArcade", "Kawai Sprite", "evilsk8r"]
                .into_iter()
                .map(|l| Member { line: l.into() })
                .collect(),
            append_backers: false,
        }],
    }
}

/// One text field: its wrapped lines, top y within the scroll, and size.
struct Line {
    lines: Vec<String>,
    y: f32,
    header: bool,
}

pub struct Credits {
    regular: Option<TtfFont>,
    bold: Option<TtfFont>,
    lines: Vec<Line>,
    /// `creditsGroup.y`.
    scroll: f32,
    held_fast: [bool; 2],
    held_pause: bool,
    exiting: bool,
}

/// The system monospace font the original asks for (`Consolas` on Windows).
fn system_font(bold: bool) -> Option<TtfFont> {
    let windir = std::env::var_os("WINDIR").map(std::path::PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    let name = if bold { "consolab.ttf" } else { "consola.ttf" };
    TtfFont::load(&windir.join("Fonts").join(name)).ok()
}

/// Greedy word wrap to `width` px (`FlxText.fieldWidth` minus its gutters).
fn wrap(gfx: &mut Gfx, font: &TtfFont, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split(' ') {
            let trial = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
            if !cur.is_empty() && font.width(gfx, &trial, size) > width {
                out.push(std::mem::take(&mut cur));
                cur = word.to_owned();
            } else {
                cur = trial;
            }
        }
        out.push(cur);
    }
    out
}

impl Credits {
    pub fn new(gfx: &mut Gfx, assets: &Assets, vcr: Option<&TtfFont>) -> Self {
        let path = assets.root().join("data").join("credits.json");
        let data = funkin_core::vfs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str::<CreditsData>(&s).map_err(|e| log::warn!("credits.json: {e}")).ok())
            .unwrap_or_else(fallback);
        let regular = system_font(false);
        let bold = system_font(true);

        // buildCreditsEntryLine, all at once: the y of each text field.
        let width = FULL_WIDTH / 2.0 - 4.0;
        let mut lines = Vec::new();
        let mut y = 0.0;
        {
            let pick = |header: bool| -> Option<&TtfFont> {
                if header { bold.as_ref().or(regular.as_ref()) } else { regular.as_ref() }.or(vcr)
            };
            for role in &data.entries {
                if let Some(h) = &role.header {
                    let wrapped = match pick(true) {
                        Some(f) => wrap(gfx, f, h, HEADER_FONT_SIZE, width),
                        None => vec![h.clone()],
                    };
                    let n = wrapped.len() as f32;
                    lines.push(Line { lines: wrapped, y, header: true });
                    y += HEADER_FONT_SIZE + n * HEADER_FONT_SIZE;
                }
                for m in &role.body {
                    let wrapped = match pick(false) {
                        Some(f) => wrap(gfx, f, &m.line, FONT_SIZE, width),
                        None => vec![m.line.clone()],
                    };
                    let n = wrapped.len() as f32;
                    lines.push(Line { lines: wrapped, y, header: false });
                    y += FONT_SIZE * n;
                }
                y += FONT_SIZE * 2.5;
            }
        }
        Self { regular, bold, lines, scroll: VIEW_H, held_fast: [false; 2], held_pause: false, exiting: false }
    }

    /// `FunkinSound.playMusic('freeplayRandom')`, faded in over 6 s to 0.8.
    pub fn start_music(audio: &Audio, assets: &Assets, music: &mut Music) {
        if music.play_menu(audio, assets, "freeplayRandom", true) {
            music.fade(0.0, 0.8, 6.0);
        }
    }

    fn font<'a>(&'a self, header: bool, vcr: Option<&'a TtfFont>) -> Option<&'a TtfFont> {
        if header { self.bold.as_ref().or(self.regular.as_ref()) } else { self.regular.as_ref() }.or(vcr)
    }

    /// Key presses and releases: Enter/Space speed up, Shift or Pause stop it,
    /// Back leaves.
    pub fn key(&mut self, code: KeyCode, menu: Option<KeyCode>, is_pause: bool, pressed: bool) -> MenuAction {
        match code {
            KeyCode::Enter | KeyCode::NumpadEnter => self.held_fast[0] = pressed,
            KeyCode::Space => self.held_fast[1] = pressed,
            _ => {}
        }
        if is_pause {
            self.held_pause = pressed;
        }
        if pressed && matches!(menu, Some(KeyCode::Escape | KeyCode::Backspace)) {
            return self.exit();
        }
        MenuAction::None
    }

    fn exit(&mut self) -> MenuAction {
        if std::mem::replace(&mut self.exiting, true) {
            return MenuAction::None;
        }
        MenuAction::CloseCredits
    }

    pub fn update(&mut self, dt: f32, shift: bool) -> MenuAction {
        let speed = if self.held_fast.iter().any(|&h| h) {
            SCROLL_FAST_SPEED
        } else if self.held_pause || shift {
            0.0
        } else {
            SCROLL_BASE_SPEED
        };
        self.scroll -= speed * dt;
        // hasEnded: every line has scrolled off the top.
        let end = self.lines.last().map(|l| l.y + Self::height(l)).unwrap_or(0.0);
        if self.scroll + end <= 0.0 {
            return self.exit();
        }
        MenuAction::None
    }

    fn height(l: &Line) -> f32 {
        let size = if l.header { HEADER_FONT_SIZE } else { FONT_SIZE };
        size * 1.2 * l.lines.len() as f32 + 4.0
    }

    pub fn draw(&self, gfx: &mut Gfx, vcr: Option<&TtfFont>) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        for l in &self.lines {
            let y = self.scroll + l.y;
            if y > VIEW_H || y + Self::height(l) <= 0.0 {
                continue;
            }
            let Some(font) = self.font(l.header, vcr) else { continue };
            let size = if l.header { HEADER_FONT_SIZE } else { FONT_SIZE };
            let lh = font.line_height(size);
            for (i, text) in l.lines.iter().enumerate() {
                // FlxText OUTLINE, black, borderSize 1.
                super::options::draw_outlined_alpha(gfx, font, text, SCREEN_PAD, y + i as f32 * lh, size, 1.0, 1.0);
            }
        }
    }
}
