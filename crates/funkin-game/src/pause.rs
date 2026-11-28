//! Port of `PauseSubState` (desktop): Resume, Restart Song, Change Difficulty,
//! Practice Mode and Exit to Menu as the sliding list, the song info in the
//! corner (artist and charter taking turns), the breakfast music, and
//! Shift+Up/Down to nudge the global offset.

use std::sync::Arc;

use winit::keyboard::KeyCode;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::{Gfx, VIEW_H, VIEW_W};
use crate::menus::options::draw_outlined_alpha;
use crate::menus::{Fonts, Tween};

const MUSIC_FADE_IN_TIME: f32 = 5.0;
const MUSIC_FINAL_VOLUME: f32 = 0.75;
const CHARTER_FADE_DELAY: f32 = 15.0;
const CHARTER_FADE_DURATION: f32 = 0.75;

pub enum PauseAction {
    None,
    Resume,
    Restart,
    ChangeDifficulty(String),
    Exit,
    /// The global offset changed (ms).
    Offset(i32),
    /// Cutscene mode: `VideoCutscene.finishVideo()` / `restartVideo()`.
    SkipCutscene,
    RestartCutscene,
}

#[derive(PartialEq)]
enum Mode {
    Standard,
    Difficulty,
    /// Paused during a video cutscene (`PauseMode.Cutscene`).
    Cutscene,
}

/// One entry's position: tweened towards its slot in the list.
struct Slot {
    x: Tween,
    y: Tween,
}

pub struct PauseMenu {
    mode: Mode,
    entries: Vec<String>,
    sel: usize,
    time: f32,
    slots: Vec<Slot>,
    music: Option<Arc<Track>>,
    song: String,
    artist: String,
    charter: String,
    difficulty: String,
    deaths: u32,
    difficulties: Vec<String>,
    practice: bool,
    /// `offset` is a float: holding Shift+Up/Down adds 30 ms per second.
    offset: f32,
    shift: bool,
    up: bool,
    down: bool,
    fast_offset: bool,
    last_offset_press: f32,
    scroll_sfx: Option<Arc<Pcm>>,
}

fn title_case(s: &str) -> String {
    s.replace('-', " ")
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl PauseMenu {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        audio: &Audio,
        assets: &Assets,
        song: &str,
        artist: &str,
        charter: Option<&str>,
        difficulty: &str,
        variation: &str,
        difficulties: Vec<String>,
        deaths: u32,
        practice: bool,
        offset: i32,
        cutscene: bool,
    ) -> Self {
        // `breakfast$musicSuffix`: the Pico mix for Pico's variation.
        let suffix = if variation == "pico" { "-pico" } else { "" };
        let music = Pcm::load_ogg(&assets.music_path(&format!("breakfast{suffix}")))
            .or_else(|_| Pcm::load_ogg(&assets.music_path("breakfast")))
            .map(Track::new)
            .ok();
        // No pause music over a cutscene (its own sound is just held).
        let music = if cutscene { None } else { music };
        if let Some(m) = &music {
            m.set_volume(0.0);
            // Starts somewhere in the first half of the track.
            let start = (crate::menus::random() as f64 * (m.pcm.frames() / 2) as f64).floor();
            audio.set_overlay(Some((m.clone(), start)));
        }
        let mut p = Self {
            mode: if cutscene { Mode::Cutscene } else { Mode::Standard },
            entries: Vec::new(),
            sel: 0,
            time: 0.0,
            slots: Vec::new(),
            music,
            song: song.to_owned(),
            artist: artist.to_owned(),
            charter: charter.unwrap_or("Unknown").to_owned(),
            difficulty: title_case(difficulty),
            deaths,
            difficulties,
            practice,
            offset: offset as f32,
            shift: false,
            up: false,
            down: false,
            fast_offset: false,
            last_offset_press: 0.0,
            scroll_sfx: assets.sound("default:scrollMenu"),
        };
        p.regenerate();
        p
    }

    /// `regenerateMenu`: new entries start at `(0, 70i + 30)` and slide to their slots.
    fn regenerate(&mut self) {
        self.entries = match self.mode {
            Mode::Standard => {
                let mut e = vec!["Resume".to_owned(), "Restart Song".to_owned(), "Change Difficulty".to_owned()];
                if !self.practice {
                    e.push("Enable Practice Mode".to_owned());
                }
                e.push("Exit to Menu".to_owned());
                e
            }
            Mode::Cutscene => vec!["Resume".to_owned(), "Skip Cutscene".to_owned(), "Restart Cutscene".to_owned(), "Exit to Menu".to_owned()],
            Mode::Difficulty => {
                let mut e: Vec<String> = self.difficulties.iter().map(|d| title_case(d)).collect();
                e.push("Back".to_owned());
                e
            }
        };
        self.sel = 0;
        self.slots = (0..self.entries.len())
            .map(|i| Slot { x: Tween::hold(0.0), y: Tween::hold(70.0 * i as f32 + 30.0) })
            .collect();
        self.change_selection(None, 0);
    }

    pub fn practice(&self) -> bool {
        self.practice
    }

    /// `changeSelection`: wrap, then tween every entry to
    /// `x = 26d + 90, y = 156d + 345.6` (d = distance from the selection).
    fn change_selection(&mut self, audio: Option<&Audio>, by: i32) {
        let n = self.entries.len() as i32;
        let prev = self.sel;
        self.sel = (self.sel as i32 + by).rem_euclid(n) as usize;
        if self.sel != prev {
            if let (Some(a), Some(s)) = (audio, &self.scroll_sfx) {
                a.play_sfx(s.clone(), 0.4);
            }
        }
        let t = self.time;
        for (i, slot) in self.slots.iter_mut().enumerate() {
            let d = i as f32 - self.sel as f32;
            let (x, y) = (d * 1.3 * 20.0 + 90.0, d * 1.3 * 120.0 + VIEW_H * 0.48);
            slot.x = Tween::new(slot.x.at(t), x, t, 0.33, "quartOut");
            slot.y = Tween::new(slot.y.at(t), y, t, 0.33, "quartOut");
        }
    }

    /// A key went down. `menu` is the key through the UI bindings; `pause` is
    /// whether it's bound to PAUSE; `shift` whether Shift is held.
    pub fn key(&mut self, audio: &Audio, menu: Option<KeyCode>, pause: bool, shift: bool) -> PauseAction {
        self.shift = shift;
        match menu {
            Some(KeyCode::ArrowUp) => self.up = true,
            Some(KeyCode::ArrowDown) => self.down = true,
            _ => {}
        }
        // `handleModifyingOffsets`: Shift+Up/Down steps by 1 until held for 0.5 s.
        if self.shift && (self.up || self.down) {
            if matches!(menu, Some(KeyCode::ArrowUp | KeyCode::ArrowDown)) && !self.fast_offset {
                self.offset = (self.offset + if self.up { 1.0 } else { -1.0 }).clamp(-1500.0, 1500.0);
                return PauseAction::Offset(self.offset as i32);
            }
            return PauseAction::None;
        }
        match menu {
            Some(KeyCode::ArrowUp) => self.change_selection(Some(audio), -1),
            Some(KeyCode::ArrowDown) => self.change_selection(Some(audio), 1),
            Some(KeyCode::Enter) => {
                let e = self.entries[self.sel].clone();
                match (&self.mode, e.as_str()) {
                    (Mode::Standard, "Resume") => return PauseAction::Resume,
                    (Mode::Standard, "Restart Song") => return PauseAction::Restart,
                    (Mode::Standard, "Change Difficulty") => {
                        self.mode = Mode::Difficulty;
                        self.regenerate();
                    }
                    (Mode::Standard, "Enable Practice Mode") => {
                        self.practice = true;
                        self.regenerate();
                    }
                    (Mode::Standard, _) => return PauseAction::Exit,
                    (Mode::Cutscene, "Resume") => return PauseAction::Resume,
                    (Mode::Cutscene, "Skip Cutscene") => return PauseAction::SkipCutscene,
                    (Mode::Cutscene, "Restart Cutscene") => return PauseAction::RestartCutscene,
                    (Mode::Cutscene, _) => return PauseAction::Exit,
                    (Mode::Difficulty, "Back") => {
                        self.mode = Mode::Standard;
                        self.regenerate();
                    }
                    (Mode::Difficulty, _) => return PauseAction::ChangeDifficulty(self.difficulties[self.sel].clone()),
                }
            }
            _ if pause => return PauseAction::Resume,
            _ => {}
        }
        PauseAction::None
    }

    /// A key came up (for the held Shift+Up/Down offset).
    pub fn key_up(&mut self, menu: Option<KeyCode>, shift: bool) {
        self.shift = shift;
        match menu {
            Some(KeyCode::ArrowUp) => self.up = false,
            Some(KeyCode::ArrowDown) => self.down = false,
            _ => {}
        }
    }

    pub fn update(&mut self, dt: f32) -> PauseAction {
        self.time += dt;
        if let Some(m) = &self.music {
            m.set_volume((self.time / MUSIC_FADE_IN_TIME).min(1.0) * MUSIC_FINAL_VOLUME);
        }
        if self.shift && (self.up || self.down) {
            self.last_offset_press += dt;
            if !self.fast_offset {
                if self.last_offset_press > 0.5 {
                    self.fast_offset = true;
                    self.last_offset_press = 0.0;
                }
            } else {
                let before = self.offset as i32;
                let dir = if self.up { 1.0 } else { -1.0 };
                self.offset = (self.offset + dir * dt * 30.0).clamp(-1500.0, 1500.0);
                if self.offset as i32 != before {
                    return PauseAction::Offset(self.offset as i32);
                }
            }
        } else {
            self.fast_offset = false;
            self.last_offset_press = 0.0;
        }
        PauseAction::None
    }

    /// Stop the pause music.
    pub fn close(&mut self, audio: &Audio) {
        if self.music.take().is_some() {
            audio.set_overlay(None);
        }
    }

    /// The artist line: "Artist: ..." and "Charter: ..." swap every 15 s with
    /// 0.75 s quartOut fades (`startCharterTimer` / `startArtistTimer`).
    fn artist_line(&self) -> (String, f32) {
        let period = CHARTER_FADE_DELAY + 2.0 * CHARTER_FADE_DURATION;
        let k = (self.time / period).floor();
        let p = self.time - k * period;
        let charter_first = (k as i64) % 2 == 1;
        let (before, after) = if charter_first { (true, false) } else { (false, true) };
        let fade_out_end = CHARTER_FADE_DELAY + CHARTER_FADE_DURATION;
        let (charter, alpha) = if p < CHARTER_FADE_DELAY {
            (before, 1.0)
        } else if p < fade_out_end {
            (before, 1.0 - Tween::new(0.0, 1.0, CHARTER_FADE_DELAY, CHARTER_FADE_DURATION, "quartOut").at(p))
        } else {
            (after, Tween::new(0.0, 1.0, fade_out_end, CHARTER_FADE_DURATION, "quartOut").at(p))
        };
        let text = if charter { format!("Charter: {}", self.charter) } else { format!("Artist: {}", self.artist) };
        (text, alpha)
    }

    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts) {
        let t = self.time;
        let bg = Tween::new(0.0, 0.6, 0.0, 0.8, "quartOut").at(t);
        gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, bg]);
        for (i, e) in self.entries.iter().enumerate() {
            let a = if i == self.sel { 1.0 } else { 0.6 };
            let s = &self.slots[i];
            fonts.bold.draw(gfx, e, s.x.at(t), s.y.at(t), 1.0, [1.0, 1.0, 1.0, a]);
        }
        let Some(vcr) = &fonts.vcr else { return };
        // `transitionMetadataIn`: each text fades in and drops 5 px over 1.8 s,
        // 0.1 s apart. Right-aligned in a FlxText at x = 20, 1240 wide.
        let (artist, artist_alpha) = self.artist_line();
        let rows: [(String, f32, bool); 5] = [
            (self.song.clone(), 1.0, true),
            (artist, artist_alpha, true),
            (format!("Difficulty: {}", self.difficulty), 1.0, true),
            (format!("{} Blue Balls", self.deaths), 1.0, true),
            ("PRACTICE MODE".to_owned(), 1.0, self.practice),
        ];
        let intro = |i: usize| {
            let start = 0.1 * (i + 1) as f32;
            (Tween::new(0.0, 1.0, start, 1.8, "quartOut").at(t), Tween::new(0.0, 5.0, start, 1.8, "quartOut").at(t))
        };
        for (i, (line, alpha, visible)) in rows.iter().enumerate() {
            let (a, dy) = intro(i);
            if !visible {
                continue;
            }
            // The artist line's own fades take over once it has faded in.
            let a = if i == 1 && t >= CHARTER_FADE_DELAY { *alpha } else { a };
            let w = vcr.width(gfx, line, 32.0);
            draw_outlined_alpha(gfx, vcr, line, 20.0 + 1240.0 - 4.0 - w, 15.0 + 32.0 * i as f32 + dy, 32.0, 0.0, a);
        }
        // Offset text: 16 px, right-aligned in a 1250-wide field, near the bottom.
        let h = vcr.text_height(16.0, 1);
        let oy = VIEW_H - (h + h + 40.0);
        let lines = [
            (format!("Global Offset: {}ms", self.offset as i32), oy),
            ("Hold SHIFT-UP/DOWN,".to_owned(), oy + h + 4.0),
            ("to change the offset.".to_owned(), oy + h + 4.0 + vcr.line_height(16.0)),
        ];
        for (j, (l, y)) in lines.iter().enumerate() {
            let (a, dy) = intro(5 + j.min(1));
            let w = vcr.width(gfx, l, 16.0);
            draw_outlined_alpha(gfx, vcr, l, 20.0 + 1250.0 - 4.0 - w, y + dy, 16.0, 0.0, a);
        }
    }
}
