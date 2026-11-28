//! Port of `CharSelectSubState` (with `CharSelectPlayer`, `CharSelectGF`,
//! `CharSelectCursors`, `Lock` and `Nametag`): the 3×3 character grid opened
//! from freeplay.
//!
//! The first visit plays the lights video (`IntroSubState`), then every
//! unlocked character not seen yet gets its unlock animation (`unLock`).
//!
//! Not ported: the nametag's mosaic shader and the music's pitch bend on
//! confirm.

use std::collections::HashMap;
use std::sync::Arc;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::save::Save;
use crate::video::Video;
use crate::gfx::animate::{AnimRange, AnimateAtlas, Bounds, DrawOpts};
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{fill, Affine, Blend, Color, Gfx, TexId, VIEW_H, VIEW_W, WHITE};
use crate::spectrum::Analyzer;
use winit::keyboard::KeyCode;

use super::freeplay::PixelIcon;
use super::players::{Players, DEFAULT_CHARACTER};
use super::{MenuAction, Music, Tween};

const CURSOR_FACTOR: f32 = 110.0;
const CURSOR_OFFSET: [f32; 2] = [-16.0, -48.0];
const GRID_X: f32 = 450.0;
const GRID_Y: f32 = 120.0;
const SPREAD: [f32; 2] = [107.0, 127.0];
const INIT_SPAM: f32 = 0.5;
const STAY_FUNKY_BPM: f64 = 90.0;
/// `Lock.colors`.
const LOCK_COLORS: [u32; 9] = [0x31F2A5, 0x20ECCD, 0x24D9E8, 0x20ECCD, 0x20C8D4, 0x209BDD, 0x209BDD, 0x2362C9, 0x243FB9];

/// `unLock`'s steps for one newly unlocked character.
#[derive(Clone, Copy, PartialEq)]
enum UnlockStep {
    /// `checkNewChar`'s 2 s wait before the first unlock.
    Wait,
    /// The cursor moved onto the lock; 0.5 s until it opens.
    Cursor,
    /// The lock's "unlock" animation (the locked silhouette dies on frame 40).
    Lock { died: bool },
    /// The new character's "unlock" animation; the next unlock follows it.
    Chill,
}

fn hex(c: u32) -> Color {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0]
}

fn ease(name: &str, t: f32) -> f32 {
    funkin_core::camera::ease(name, t.clamp(0.0, 1.0) as f64) as f32
}

/// One running animation of an Animate atlas.
#[derive(Clone)]
struct Play {
    name: String,
    range: AnimRange,
    t: f32,
    looped: bool,
}

impl Play {
    fn new(atlas: &AnimateAtlas, name: &str, looped: bool) -> Option<Self> {
        Some(Self { name: name.to_owned(), range: atlas.anim(name)?, t: 0.0, looped })
    }

    fn whole(atlas: &AnimateAtlas) -> Self {
        Self { name: String::new(), range: atlas.whole(), t: 0.0, looped: true }
    }

    fn index(&self, fps: f32) -> u32 {
        let i = (self.t * fps) as u32;
        let len = self.range.len.max(1);
        if self.looped {
            i % len
        } else {
            i.min(len - 1)
        }
    }

    fn frame(&self, fps: f32) -> u32 {
        self.range.start + self.index(fps)
    }

    fn finished(&self, fps: f32) -> bool {
        !self.looped && self.t * fps >= self.range.len as f32
    }

    fn draw(&self, atlas: &AnimateAtlas, gfx: &mut Gfx, m: &Affine, color: Color) {
        atlas.draw(gfx, self.range.tl, self.frame(atlas.fps), m, color);
    }
}

/// `CharSelectPlayer`: a character's "chill" atlas.
struct Chill {
    id: String,
    play: Option<Play>,
    handled_finish: bool,
}

/// A grid cell: a character's pixel icon, or a lock.
enum Cell {
    Icon { id: String, confirm_at: Option<f32> },
    Lock { play: Option<Play> },
}

/// The "static loop" while a locked cell is selected.
struct StaticLoop {
    id: u64,
    started: f32,
}

pub struct CharSelect {
    time: f32,
    /// Seconds since `goToFreeplay` (exiting), and the character to return with.
    exit: Option<(f32, String)>,
    remembered: String,
    went_back: bool,

    bg: Option<TexId>,
    curtains: Option<TexId>,
    char_light: Option<TexId>,
    fg_blur: Option<TexId>,
    choose: Option<TexId>,
    gradient: Option<TexId>,
    selector: Option<TexId>,
    nametags: HashMap<String, Option<TexId>>,
    crowd: Option<(AnimateAtlas, Play)>,
    stage: Option<(AnimateAtlas, Play)>,
    speakers: Option<(AnimateAtlas, Play)>,
    barthing: Option<(AnimateAtlas, Play)>,
    lock_atlas: Option<AnimateAtlas>,
    lock_bounds: Bounds,
    dipshit_blur: Option<(Atlas, Anim)>,
    dipshit_backing: Option<(Atlas, Anim)>,
    cursor_confirm: Option<(Atlas, Anim)>,
    cursor_denied: Option<(Atlas, Anim)>,
    icons: HashMap<String, Option<PixelIcon>>,

    /// Character id → its chill atlas (`charSelect/<id>Chill`).
    chills: HashMap<String, AnimateAtlas>,
    /// GF atlas path → atlas.
    gfs: HashMap<String, AnimateAtlas>,
    /// Character id → (gf atlas path, has a visualizer).
    gf_of: HashMap<String, (String, bool)>,
    analyzer: Analyzer<7>,

    /// Grid position → character id.
    available: HashMap<usize, String>,
    cells: Vec<Cell>,

    cursor: [i32; 2],
    cursor_main: [f32; 2],
    cursor_light: [f32; 2],
    cursor_dark: [f32; 2],
    confirmed: bool,
    denied_at: Option<f32>,

    cur_char: String,
    player: Chill,
    /// `playerChillOut`: the old character sliding out.
    player_out: Option<Chill>,
    /// Who slides in once the old one starts sliding out.
    pending: Option<String>,
    gf: Option<(String, Play)>,
    gf_visible: bool,
    /// When the deselect tween (back to idle) finishes.
    deselect_until: Option<f32>,

    pressed_select: bool,
    select_at: Option<f32>,
    allow_input: bool,
    held: [bool; 4],
    hold_tmr: [f32; 4],
    spam: [bool; 4],
    last_beat: i64,
    last_step: i64,

    cam: [f32; 2],
    auto_follow: bool,
    flash_at: Option<f32>,
    fade: Tween,

    sfx_select: Option<Arc<Pcm>>,
    sfx_confirm: Option<Arc<Pcm>>,
    sfx_locked: Option<Arc<Pcm>>,
    sfx_cancel: Option<Arc<Pcm>>,
    sfx_static: Option<Arc<Pcm>>,
    sfx_unlock: Option<Arc<Pcm>>,
    assets: Assets,
    sfx_lights: Option<Arc<Pcm>>,
    static_loop: Option<StaticLoop>,

    /// The lights video, over a black screen; nothing else runs meanwhile.
    video: Option<Video>,
    /// Waiting for the video to close (`subStateClosed`).
    intro_pending: bool,
    flash_dur: f32,
    save: Save,
    save_dirty: bool,
    /// Grid positions of unlocked characters not seen yet (`nonLocks`).
    non_locks: Vec<usize>,
    /// Grid position → character id, for the locks that will open.
    unlockable: HashMap<usize, String>,
    unlock: Option<(usize, UnlockStep, f32)>,
}

fn load_animate(gfx: &mut Gfx, assets: &Assets, path: &str, swf: bool) -> Option<AnimateAtlas> {
    let dir = assets.image_path(&format!("default:{path}")).with_extension("");
    match AnimateAtlas::load(gfx, &dir, true) {
        Ok(mut a) => {
            a.swf_mode = swf;
            Some(a)
        }
        Err(e) => {
            log::warn!("char select: {e}");
            None
        }
    }
}

fn load_whole(gfx: &mut Gfx, assets: &Assets, path: &str) -> Option<(AnimateAtlas, Play)> {
    let a = load_animate(gfx, assets, path, false)?;
    let p = Play::whole(&a);
    Some((a, p))
}

fn tex(gfx: &mut Gfx, assets: &Assets, path: &str) -> Option<TexId> {
    gfx.load_texture(&assets.image_path(&format!("default:{path}"))).map_err(|e| log::warn!("char select: {e}")).ok()
}

fn sparrow(gfx: &mut Gfx, assets: &Assets, path: &str, prefix: &str) -> Option<(Atlas, Anim)> {
    let a = assets.atlas(gfx, &format!("default:{path}")).map_err(|e| log::warn!("char select: {e}")).ok()?;
    let an = a.prefix(prefix);
    Some((a, an))
}

/// `setCursorPosition`'s index → (x, y) in -1..=1.
fn cursor_of(index: usize) -> [i32; 2] {
    [(index % 3) as i32 - 1, (index / 3) as i32 - 1]
}

fn intended(c: [i32; 2]) -> [f32; 2] {
    [
        CURSOR_FACTOR * c[0] as f32 + VIEW_W / 2.0 - 124.0 / 2.0 + CURSOR_OFFSET[0],
        CURSOR_FACTOR * c[1] as f32 + VIEW_H / 2.0 - 112.0 / 2.0 + CURSOR_OFFSET[1],
    ]
}

impl CharSelect {
    #[allow(clippy::too_many_arguments)]
    pub fn new(gfx: &mut Gfx, assets: &Assets, audio: &Audio, music: &mut Music, players: &Players, save: &Save, character: &str) -> Self {
        // loadAvailableCharacters.
        let mut available = HashMap::new();
        let mut chills = HashMap::new();
        let mut gfs = HashMap::new();
        let mut gf_of = HashMap::new();
        for p in &players.list {
            let Some(cs) = &p.char_select else { continue };
            let mut pos = cs.position.unwrap_or(0);
            while available.contains_key(&pos) {
                pos += 1;
            }
            if pos >= 9 {
                continue;
            }
            available.insert(pos, p.id.clone());
            if let Some(a) = load_animate(gfx, assets, &format!("charSelect/{}Chill", p.id), true) {
                chills.insert(p.id.clone(), a);
            }
            if let Some(gf) = &cs.gf {
                if !gfs.contains_key(&gf.asset_path) {
                    if let Some(a) = load_animate(gfx, assets, &gf.asset_path, true) {
                        gfs.insert(gf.asset_path.clone(), a);
                    }
                }
                gf_of.insert(p.id.clone(), (gf.asset_path.clone(), gf.visualizer));
            }
        }
        if let Some(mut a) = load_animate(gfx, assets, "charSelect/lockedChill", true) {
            a.swf_mode = true;
            chills.insert("locked".into(), a);
        }

        let mut icons = HashMap::new();
        let mut cells = Vec::new();
        let mut non_locks = Vec::new();
        let mut unlockable = HashMap::new();
        for i in 0..9 {
            let unlocked = available.get(&i).filter(|id| players.get(id).is_some_and(|p| p.unlocked));
            match unlocked {
                Some(id) => {
                    icons.entry(id.clone()).or_insert_with(|| PixelIcon::load(gfx, assets, id));
                    if save.is_seen(id) {
                        cells.push(Cell::Icon { id: id.clone(), confirm_at: None });
                    } else {
                        // Unlocked but not seen: a lock that opens (`nonLocks`).
                        non_locks.push(i);
                        unlockable.insert(i, id.clone());
                        cells.push(Cell::Lock { play: None });
                    }
                }
                None => cells.push(Cell::Lock { play: None }),
            }
        }
        // Locked and unseen players count as not there.
        available.retain(|i, _| matches!(cells[*i], Cell::Icon { .. }));

        let lock_atlas = load_animate(gfx, assets, "charSelect/lock", true);
        let lock_bounds = lock_atlas.as_ref().map(|a| a.frame_bounds(a.whole().tl, 0)).unwrap_or([0.0; 4]);
        if let Some(a) = &lock_atlas {
            for c in &mut cells {
                if let Cell::Lock { play } = c {
                    *play = Play::new(a, "idle", false);
                }
            }
        }

        let mut nametags = HashMap::new();
        for id in available.values().chain(unlockable.values()).cloned().chain(["locked".to_owned()]) {
            let path = if id == "bf" { "boyfriend".to_owned() } else { id.clone() };
            nametags.insert(id, tex(gfx, assets, &format!("charSelect/{path}Nametag")));
        }

        let remembered = if character.is_empty() { DEFAULT_CHARACTER.to_owned() } else { character.to_owned() };
        let start_index = available.iter().find(|(_, id)| **id == remembered).map(|(i, _)| *i).unwrap_or(4);
        let cur_char = available.get(&start_index).cloned().unwrap_or_else(|| "locked".into());
        let cursor = cursor_of(start_index);
        let at = intended(cursor);

        // IntroSubState: the lights video, the first time only.
        let video = if save.old_char {
            None
        } else {
            music.stop(audio);
            let path = assets.root().join("videos").join("videos").join("introSelect.mp4");
            match funkin_core::vfs::read(&path).map_err(|e| format!("{}: {e}", path.display())).and_then(Video::open) {
                Ok(v) => Some(v),
                Err(e) => {
                    log::warn!("char select: {e}");
                    None
                }
            }
        };

        let mut cs = Self {
            time: 0.0,
            exit: None,
            remembered,
            went_back: false,
            bg: tex(gfx, assets, "charSelect/charSelectBG"),
            curtains: tex(gfx, assets, "charSelect/curtains"),
            char_light: tex(gfx, assets, "charSelect/charLight"),
            fg_blur: tex(gfx, assets, "charSelect/foregroundBlur"),
            choose: tex(gfx, assets, "charSelect/chooseDipshit"),
            gradient: tex(gfx, assets, "freeplay/transitionGradient"),
            selector: tex(gfx, assets, "charSelect/charSelector"),
            nametags,
            crowd: load_whole(gfx, assets, "charSelect/crowd"),
            stage: load_whole(gfx, assets, "charSelect/charSelectStage"),
            speakers: load_whole(gfx, assets, "charSelect/charSelectSpeakers"),
            barthing: load_whole(gfx, assets, "charSelect/barThing"),
            lock_atlas,
            lock_bounds,
            dipshit_blur: sparrow(gfx, assets, "charSelect/dipshitBlur", "CHOOSE vertical offset instance 1"),
            dipshit_backing: sparrow(gfx, assets, "charSelect/dipshitBacking", "CHOOSE horizontal offset instance 1"),
            cursor_confirm: sparrow(gfx, assets, "charSelect/charSelectorConfirm", "cursor ACCEPTED instance 1"),
            cursor_denied: sparrow(gfx, assets, "charSelect/charSelectorDenied", "cursor DENIED instance 1"),
            icons,
            chills,
            gfs,
            gf_of,
            analyzer: Analyzer::default(),
            available,
            cells,
            cursor,
            cursor_main: at,
            cursor_light: at,
            cursor_dark: at,
            video,
            intro_pending: true,
            flash_dur: 1.0,
            save: save.clone(),
            save_dirty: false,
            non_locks,
            unlockable,
            unlock: None,
            confirmed: false,
            denied_at: None,
            cur_char: cur_char.clone(),
            player: Chill { id: cur_char.clone(), play: None, handled_finish: false },
            player_out: None,
            pending: None,
            gf: None,
            gf_visible: true,
            deselect_until: None,
            pressed_select: false,
            select_at: None,
            allow_input: false,
            held: [false; 4],
            hold_tmr: [0.0; 4],
            spam: [false; 4],
            last_beat: i64::MIN,
            last_step: i64::MIN,
            cam: [VIEW_W / 2.0, VIEW_H / 2.0 - 150.0],
            auto_follow: false,
            flash_at: None,
            fade: Tween::new(0.0, 1.0, 0.0, 0.8, "quadOut"),
            sfx_select: assets.sound("default:CS_select"),
            sfx_confirm: assets.sound("default:CS_confirm"),
            sfx_locked: assets.sound("default:CS_locked"),
            sfx_cancel: assets.sound("default:cancelMenu"),
            sfx_static: assets.sound("default:static loop"),
            sfx_unlock: assets.sound("default:CS_unlock"),
            assets: assets.clone(),
            sfx_lights: assets.sound("default:CS_Lights"),
            static_loop: None,
        };
        cs.switch_player(&cur_char, false);
        cs.switch_gf(&cur_char);
        cs
    }

    fn sfx(&self, audio: &Audio, s: &Option<Arc<Pcm>>, v: f32) {
        if let Some(s) = s {
            audio.play_sfx(s.clone(), v);
        }
    }

    /// The save, when it changed (for the app to write).
    pub fn take_save(&mut self) -> Option<Save> {
        std::mem::take(&mut self.save_dirty).then(|| self.save.clone())
    }

    /// `subStateClosed`: the lights video is over (or was skipped).
    fn intro_closed(&mut self, audio: &Audio, music: &mut Music) {
        self.intro_pending = false;
        if !self.save.old_char {
            self.flash_at = Some(self.time);
            self.flash_dur = 1.0;
            self.sfx(audio, &self.sfx_lights, 1.0);
            self.save.old_char = true;
            self.save_dirty = true;
        }
        // checkNewChar.
        if self.non_locks.is_empty() {
            self.start_music(audio, music);
        } else {
            self.unlock = Some((self.non_locks[0], UnlockStep::Wait, self.time));
        }
    }

    /// `FunkinSound.playMusic('stayFunky')`; input opens once it plays.
    fn start_music(&mut self, audio: &Audio, music: &mut Music) {
        if music.play_menu(audio, &self.assets, "stayFunky", true) {
            music.fade(1.0, 1.0, 0.01);
        }
        self.last_step = i64::MIN;
        self.allow_input = true;
    }

    /// Steps `unLock` for the next unseen character.
    fn update_unlock(&mut self, audio: &Audio, music: &mut Music) {
        let Some((index, step, at)) = self.unlock else { return };
        let t = self.time;
        match step {
            UnlockStep::Wait if t - at >= 2.0 => self.begin_unlock(audio, index),
            UnlockStep::Cursor if t - at >= 0.5 => {
                // The lock opens; the locked silhouette stays on as playerChillOut.
                if let Some(a) = &self.lock_atlas {
                    if let Cell::Lock { play } = &mut self.cells[index] {
                        *play = Play::new(a, "unlock", false);
                    }
                }
                self.sfx(audio, &self.sfx_unlock, 0.7);
                let out_play = self.player.play.take().or_else(|| self.chills.get("locked").and_then(|a| Play::new(a, "idle", true)));
                self.player_out = Some(Chill { id: self.player.id.clone(), play: out_play.map(|mut p| {
                    p.looped = true;
                    p
                }), handled_finish: false });
                self.pending = None;
                let id = self.unlockable.get(&index).cloned().unwrap_or_default();
                self.player.id = id;
                self.player.play = None;
                self.unlock = Some((index, UnlockStep::Lock { died: false }, t));
            }
            UnlockStep::Lock { died } => {
                let Some(a) = &self.lock_atlas else { return };
                let Cell::Lock { play: Some(p) } = &self.cells[index] else { return };
                if !died && p.index(a.fps) >= 40 {
                    if let Some(out) = &mut self.player_out {
                        out.play = self.chills.get(&out.id).and_then(|a| Play::new(a, "death", false));
                    }
                    self.unlock = Some((index, UnlockStep::Lock { died: true }, at));
                }
                if p.name == "unlock" && p.finished(a.fps) {
                    self.finish_unlock(audio, music, index);
                }
            }
            UnlockStep::Chill => {
                let name = self.player.play.as_ref().map(|p| p.name.as_str()).unwrap_or("");
                if name != "unlock" {
                    self.unlock = None;
                    if let Some(&next) = self.non_locks.first() {
                        self.begin_unlock(audio, next);
                    }
                }
            }
            _ => {}
        }
    }

    /// `unLock`'s start: the cursor jumps onto the lock.
    fn begin_unlock(&mut self, audio: &Audio, index: usize) {
        self.pressed_select = true;
        self.cursor = cursor_of(index);
        self.sfx(audio, &self.sfx_select, 0.7);
        self.non_locks.retain(|&i| i != index);
        self.unlock = Some((index, UnlockStep::Cursor, self.time));
    }

    /// The lock's animation ended: the character's icon replaces it.
    fn finish_unlock(&mut self, audio: &Audio, music: &mut Music, index: usize) {
        let id = self.unlockable.remove(&index).unwrap_or_else(|| DEFAULT_CHARACTER.to_owned());
        self.flash_at = Some(self.time);
        self.flash_dur = 0.1;
        self.player.id = id.clone();
        self.play_player("unlock", false);
        self.switch_gf(&id);
        self.gf_visible = true;
        self.cells[index] = Cell::Icon { id: id.clone(), confirm_at: None };
        self.available.insert(index, id.clone());
        self.save.add_seen(&id);
        self.save_dirty = true;
        // set curChar without its setter: no slide, the static stops.
        self.cur_char = id;
        if let Some(l) = self.static_loop.take() {
            audio.stop_sfx(l.id);
        }
        if self.non_locks.is_empty() {
            self.unlock = None;
            self.pressed_select = false;
            self.start_music(audio, music);
        } else {
            self.unlock = Some((index, UnlockStep::Chill, self.time));
        }
    }

    fn current_index(&self) -> usize {
        ((self.cursor[0] + 1) + (self.cursor[1] + 1) * 3) as usize
    }

    /// `CharSelectPlayer.switchChar`.
    fn switch_player(&mut self, id: &str, slide: bool) {
        self.player.id = id.to_owned();
        self.player.handled_finish = false;
        self.player.play = self.chills.get(id).and_then(|a| Play::new(a, if slide { "slidein" } else { "idle" }, false));
    }

    fn play_player(&mut self, name: &str, looped: bool) {
        if let Some(p) = self.chills.get(&self.player.id).and_then(|a| Play::new(a, name, looped)) {
            self.player.play = Some(p);
            self.player.handled_finish = false;
        }
    }

    /// `CharSelectGF.switchGF`.
    fn switch_gf(&mut self, bf: &str) {
        let Some((path, _)) = self.gf_of.get(bf).cloned() else {
            self.gf = None;
            return;
        };
        let Some(a) = self.gfs.get(&path) else {
            self.gf = None;
            return;
        };
        self.gf = Play::new(a, "idle", false).map(|p| (path, p));
        self.gf_visible = true;
    }

    fn play_gf(&mut self, name: &str, looped: bool) {
        if let Some((path, p)) = &mut self.gf {
            if let Some(n) = self.gfs.get(path.as_str()).and_then(|a| Play::new(a, name, looped)) {
                *p = n;
            }
        }
    }

    /// `set_curChar`.
    fn set_cur_char(&mut self, audio: &Audio, value: &str) {
        if self.cur_char == value {
            return;
        }
        self.cur_char = value.to_owned();
        if value == "locked" {
            if let (Some(s), None) = (&self.sfx_static, &self.static_loop) {
                self.static_loop = Some(StaticLoop { id: audio.play_sfx_tracked(s.clone(), 0.6), started: self.time });
            }
        } else if let Some(l) = self.static_loop.take() {
            audio.stop_sfx(l.id);
        }
        self.gf_visible = false;
        // playerChillOut (still the previous character) slides out; the new one
        // slides in on its next frame.
        let out_id = self.player.id.clone();
        let play = self.chills.get(&out_id).and_then(|a| Play::new(a, "slideout", false));
        self.player_out = Some(Chill { id: out_id, play, handled_finish: false });
        self.player.play = None;
        self.pending = Some(value.to_owned());
    }

    /// Key presses and releases (`menu` is the key the menu expects).
    pub fn key(&mut self, audio: &Audio, music: &mut Music, menu: Option<KeyCode>, pressed: bool) -> MenuAction {
        let dir = match menu {
            Some(KeyCode::ArrowUp) => Some(0),
            Some(KeyCode::ArrowDown) => Some(1),
            Some(KeyCode::ArrowLeft) => Some(2),
            Some(KeyCode::ArrowRight) => Some(3),
            _ => None,
        };
        if let Some(d) = dir {
            if !pressed {
                self.held[d] = false;
                self.hold_tmr[d] = 0.0;
                self.spam[d] = false;
                return MenuAction::None;
            }
            if self.held[d] {
                return MenuAction::None;
            }
            self.held[d] = true;
        }
        if !pressed || !self.allow_input || self.exit.is_some() {
            return MenuAction::None;
        }
        if !self.pressed_select {
            if let Some(d) = dir {
                self.move_cursor(audio, d);
                self.hold_tmr[d] = 0.0;
                return MenuAction::None;
            }
        }
        let on_char = self.available.contains_key(&self.current_index());
        match menu {
            Some(KeyCode::Escape) if self.pressed_select && on_char => self.deselect(music),
            Some(KeyCode::Escape) if !self.pressed_select => self.go_back(audio, music),
            Some(KeyCode::Enter) if !self.pressed_select && on_char => self.confirm(audio, music),
            Some(KeyCode::Enter) if !on_char => {
                self.play_player("cannot select Label", false);
                self.sfx(audio, &self.sfx_locked, 1.0);
                self.denied_at = Some(self.time);
                let i = self.current_index();
                if let (Some(a), Cell::Lock { play }) = (&self.lock_atlas, &mut self.cells[i]) {
                    *play = Play::new(a, "clicked", false);
                }
            }
            _ => {}
        }
        MenuAction::None
    }

    fn move_cursor(&mut self, audio: &Audio, d: usize) {
        match d {
            0 => self.cursor[1] -= 1,
            1 => self.cursor[1] += 1,
            2 => self.cursor[0] -= 1,
            _ => self.cursor[0] += 1,
        }
        self.denied_at = None;
        self.sfx(audio, &self.sfx_select, 0.7);
    }

    fn confirm(&mut self, audio: &Audio, music: &mut Music) {
        self.spam = [false; 4];
        self.confirmed = true;
        self.sfx(audio, &self.sfx_confirm, 1.0);
        music.fade(music.volume(), 0.0, 1.5);
        self.play_player("select", false);
        self.play_gf("confirm", true);
        self.pressed_select = true;
        self.select_at = Some(self.time);
        self.deselect_until = None;
        let i = self.current_index();
        if let Cell::Icon { confirm_at, .. } = &mut self.cells[i] {
            *confirm_at = Some(self.time);
        }
    }

    fn deselect(&mut self, music: &mut Music) {
        self.confirmed = false;
        music.fade(music.volume(), 1.0, 1.0);
        self.play_player("deselect", false);
        self.play_gf("deselect", false);
        self.pressed_select = false;
        self.select_at = None;
        self.deselect_until = Some(self.time + 1.0);
        for c in &mut self.cells {
            if let Cell::Icon { confirm_at, .. } = c {
                *confirm_at = None;
            }
        }
    }

    fn go_back(&mut self, audio: &Audio, music: &mut Music) {
        self.went_back = true;
        self.sfx(audio, &self.sfx_cancel, 1.0);
        music.fade(music.volume(), 0.0, 0.7);
        self.go_to_freeplay();
    }

    /// `goToFreeplay`: everything slides away, then freeplay opens.
    fn go_to_freeplay(&mut self) {
        self.allow_input = false;
        self.auto_follow = false;
        let char = if self.went_back { self.remembered.clone() } else { self.cur_char.clone() };
        self.exit = Some((self.time, char));
    }

    pub fn update(&mut self, dt: f32, gfx: Option<&mut Gfx>, audio: &Audio, music: &mut Music) -> MenuAction {
        // The video substate pauses everything under it.
        if let Some(v) = &mut self.video {
            let done = match gfx {
                Some(g) => v.update(dt, g, audio),
                None => false,
            };
            if !done {
                return MenuAction::None;
            }
            self.video = None;
        }
        if self.intro_pending {
            self.intro_closed(audio, music);
        }
        self.time += dt;
        let t = self.time;
        self.update_unlock(audio, music);

        // Conductor (stayFunky, 90 BPM).
        let ms = music.time_ms(audio);
        let step_ms = 60000.0 / STAY_FUNKY_BPM / 4.0;
        let step = (ms / step_ms).floor() as i64;
        if self.last_step == i64::MIN {
            self.last_step = step;
            self.last_beat = step.div_euclid(4);
        }
        if step != self.last_step {
            self.last_step = step;
            self.spam_on_step(audio);
            let beat = step.div_euclid(4);
            if beat != self.last_beat {
                self.last_beat = beat;
                self.beat_hit(beat);
            }
        }

        // Hold to scroll.
        if self.allow_input && !self.pressed_select {
            for d in 0..4 {
                if self.held[d] {
                    self.hold_tmr[d] += dt;
                    if self.hold_tmr[d] >= INIT_SPAM {
                        self.spam[d] = true;
                    }
                }
            }
        }
        self.cursor[0] = (self.cursor[0] + 1).rem_euclid(3) - 1;
        self.cursor[1] = (self.cursor[1] + 1).rem_euclid(3) - 1;

        let idx = self.current_index();
        match self.available.get(&idx).cloned() {
            Some(id) => self.set_cur_char(audio, &id),
            None => {
                self.set_cur_char(audio, "locked");
                self.gf_visible = false;
            }
        }

        // The static loop keeps going.
        if let (Some(l), Some(s)) = (&mut self.static_loop, &self.sfx_static) {
            if t - l.started >= (s.duration_ms() / 1000.0) as f32 {
                audio.stop_sfx(l.id);
                l.id = audio.play_sfx_tracked(s.clone(), 0.6);
                l.started = t;
            }
        }

        if let Some(at) = self.select_at {
            if t - at >= 1.5 {
                self.select_at = None;
                self.go_to_freeplay();
            }
        }
        if let Some(until) = self.deselect_until {
            if t >= until {
                self.deselect_until = None;
                let name = self.player.play.as_ref().map(|p| p.name.clone()).unwrap_or_default();
                if name == "deselect loop start" || name == "deselect" {
                    self.play_player("idle", true);
                    self.play_gf("idle", true);
                }
            }
        }

        self.update_anims(dt);
        self.update_locks();

        // Nene's speaker visualizer.
        if let Some((path, _)) = &self.gf {
            if self.gf_of.values().any(|(p, v)| p == path && *v) {
                if let Some(pcm) = music.pcm() {
                    self.analyzer.update(pcm, ms.rem_euclid(pcm.duration_ms().max(1.0)));
                }
                if let Some(a) = self.gfs.get_mut(path.as_str()) {
                    for (i, l) in self.analyzer.levels.iter().enumerate() {
                        let f = ((l * 12.0).round() as i32).clamp(0, 12);
                        a.set_symbol_frame(&format!("VIZ{}", i + 1), Some((f - 12).unsigned_abs()));
                    }
                }
            }
        }

        // Camera.
        let center = [VIEW_W / 2.0, VIEW_H / 2.0];
        if let Some((at, _)) = &self.exit {
            let e = t - at;
            self.cam = [center[0], center[1] - 150.0 * ease("backIn", e / 0.8)];
        } else if self.auto_follow {
            let target = [center[0] + self.cursor[0] as f32 * 10.0, center[1] + self.cursor[1] as f32 * 10.0];
            self.cam[0] = super::follow_lerp(self.cam[0], target[0], dt, 0.01);
            self.cam[1] = super::follow_lerp(self.cam[1], target[1], dt, 0.01);
        } else {
            self.cam = [center[0], center[1] - 150.0 + 150.0 * ease("expoOut", t / 1.5)];
            if t >= 1.5 {
                self.auto_follow = true;
            }
        }

        // Cursors.
        let goal = intended(self.cursor);
        for k in 0..2 {
            let m = super::smooth_lerp(self.cursor_main[k], goal[k], dt, 0.1);
            self.cursor_main[k] = if (m - goal[k]).abs() < 1.0 { goal[k] } else { m };
            self.cursor_light[k] = super::smooth_lerp(self.cursor_light[k], self.cursor_main[k], dt, 0.202);
            self.cursor_dark[k] = super::smooth_lerp(self.cursor_dark[k], goal[k], dt, 0.404);
        }

        if let Some((at, char)) = &self.exit {
            if t - at >= 0.8 {
                if let Some(l) = self.static_loop.take() {
                    audio.stop_sfx(l.id);
                }
                return MenuAction::CloseCharSelect(char.clone());
            }
        }
        MenuAction::None
    }

    fn spam_on_step(&mut self, audio: &Audio) {
        if !self.spam.iter().any(|&s| s) || self.pressed_select || !self.allow_input {
            return;
        }
        self.sfx(audio, &self.sfx_select, 0.7);
        self.denied_at = None;
        for d in 0..4 {
            if self.spam[d] {
                match d {
                    0 => self.cursor[1] -= 1,
                    1 => self.cursor[1] += 1,
                    2 => self.cursor[0] -= 1,
                    _ => self.cursor[0] += 1,
                }
                self.hold_tmr[d] = 0.0;
            }
        }
    }

    fn beat_hit(&mut self, beat: i64) {
        // CharSelectPlayer.onBeatHit: replay a finished idle.
        let fps = self.chills.get(&self.player.id).map(|a| a.fps).unwrap_or(24.0);
        if let Some(p) = &self.player.play {
            if p.name == "idle" && p.finished(fps) {
                self.play_player("idle", false);
            }
        }
        // CharSelectGF.onBeatHit: every other beat.
        if self.gf.as_ref().is_some_and(|(_, p)| p.name == "idle") && beat % 2 == 0 {
            self.play_gf("idle", false);
        }
    }

    fn update_anims(&mut self, dt: f32) {
        for (_, p) in [&mut self.crowd, &mut self.stage, &mut self.speakers, &mut self.barthing].into_iter().flatten() {
            p.t += dt;
        }
        if let Some((_, p)) = &mut self.gf {
            p.t += dt;
        }
        for c in &mut self.cells {
            if let Cell::Lock { play: Some(p) } = c {
                p.t += dt;
            }
        }

        // playerChillOut: on its first frame change the new character appears.
        let mut out_state = None;
        if let Some(out) = &mut self.player_out {
            if let Some(p) = &mut out.play {
                p.t += dt;
            }
            let fps = self.chills.get(&out.id).map(|a| a.fps).unwrap_or(24.0);
            let changed = out.play.as_ref().is_none_or(|p| p.index(fps) > 0 || p.finished(fps));
            let done = out.play.as_ref().is_none_or(|p| p.finished(fps));
            out_state = Some((changed, done));
        }
        if let Some((frame_changed, done)) = out_state {
            if frame_changed {
                if let Some(v) = self.pending.take() {
                    self.switch_player(&v, true);
                    if v != "locked" {
                        self.switch_gf(&v);
                        self.gf_visible = true;
                    }
                }
            }
            if done {
                self.player_out = None;
            }
        }

        // playerChill and its onFinish handlers.
        let Some(atlas) = self.chills.get(&self.player.id) else { return };
        let fps = atlas.fps;
        let Some(p) = &mut self.player.play else { return };
        p.t += dt;
        if !p.finished(fps) || self.player.handled_finish {
            return;
        }
        self.player.handled_finish = true;
        let name = p.name.clone();
        let locked = self.player.id == "locked";
        match name.as_str() {
            "slidein" => {
                if atlas.has("slidein idle point") {
                    self.play_player("slidein idle point", false);
                } else {
                    self.play_player("idle", true);
                }
            }
            "deselect" => self.play_player("deselect loop start", true),
            "slidein idle point" | "cannot select Label" | "unlock" => self.play_player("idle", locked),
            "idle" if locked => self.play_player("idle", true),
            _ => {}
        }
    }

    /// `updateLockAnims`.
    fn update_locks(&mut self) {
        let sel = self.current_index();
        let Some(a) = &self.lock_atlas else { return };
        for (i, c) in self.cells.iter_mut().enumerate() {
            let Cell::Lock { play } = c else { continue };
            let name = play.as_ref().map(|p| p.name.as_str()).unwrap_or("");
            if i == sel {
                if name == "idle" {
                    *play = Play::new(a, "selected", false);
                }
            } else if name != "idle" {
                *play = Play::new(a, "idle", false);
            }
        }
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        if self.intro_pending {
            // blackScreen, with the video over it.
            if let Some(v) = &self.video {
                v.draw(gfx);
            }
            return;
        }
        let t = self.time;
        let exit = self.exit.as_ref().map(|(at, _)| t - at);
        let scroll = [self.cam[0] - VIEW_W / 2.0, self.cam[1] - VIEW_H / 2.0];
        let at = |x: f32, y: f32, f: f32| (x - scroll[0] * f, y - scroll[1] * f);
        let sprite = |gfx: &mut Gfx, tex: Option<TexId>, x: f32, y: f32, f: f32| {
            if let Some(tx) = tex {
                let (sx, sy) = at(x, y, f);
                gfx.sprite(tx, sx, sy, 1.0, WHITE);
            }
        };
        // Enter tweens (expoOut) plus exit tweens (backIn, 0.8 s).
        let slide = |amount: f32, dur: f32| -> f32 {
            let enter = amount * (1.0 - ease("expoOut", t / dur));
            let out = exit.map(|e| amount * ease("backIn", e / 0.8)).unwrap_or(0.0);
            enter + out
        };
        let animate = |gfx: &mut Gfx, item: &Option<(AnimateAtlas, Play)>, x: f32, y: f32, f: f32, scale: [f32; 2]| {
            if let Some((a, p)) = item {
                let (sx, sy) = at(x, y, f);
                let mut m = Affine::translate(sx, sy).then(&a.stage);
                if scale != [1.0, 1.0] {
                    let b = a.frame_bounds(p.range.tl, p.frame(a.fps));
                    let (cx, cy) = ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0);
                    m = m.then(&Affine::translate(cx, cy)).then(&Affine::scale(scale[0], scale[1])).then(&Affine::translate(-cx, -cy));
                }
                p.draw(a, gfx, &m, WHITE);
            }
        };

        sprite(gfx, self.bg, -153.0, -140.0, 0.1);
        animate(gfx, &self.crowd, 0.0, 0.0, 0.3, [1.0, 1.0]);
        animate(gfx, &self.stage, -2.0, 1.0, 1.0, [1.0, 1.0]);
        sprite(gfx, self.curtains, -212.0, -99.0, 1.4);
        gfx.set_blend(Blend::Multiply);
        animate(gfx, &self.barthing, 0.0, slide(80.0, 1.3), 0.0, [2.5, 1.0]);
        gfx.set_blend(Blend::Normal);
        sprite(gfx, self.char_light, 800.0, 250.0, 1.0);
        sprite(gfx, self.char_light, 180.0, 240.0, 1.0);

        // GF, then the outgoing and current player.
        let (ox, oy) = at(0.0, 0.0, 1.0);
        let base = Affine::translate(ox, oy);
        if self.gf_visible && self.cur_char != "locked" {
            if let Some((path, p)) = &self.gf {
                if let Some(a) = self.gfs.get(path) {
                    p.draw(a, gfx, &base.then(&a.stage), WHITE);
                }
            }
        }
        if let Some(out) = &self.player_out {
            if let (Some(p), Some(a)) = (&out.play, self.chills.get(&out.id)) {
                p.draw(a, gfx, &base.then(&a.stage), WHITE);
            }
        }
        if let (Some(p), Some(a)) = (&self.player.play, self.chills.get(&self.player.id)) {
            p.draw(a, gfx, &base.then(&a.stage), WHITE);
        }

        animate(gfx, &self.speakers, -10.0, 0.0, 1.8, [1.05, 1.05]);
        gfx.set_blend(Blend::Multiply);
        sprite(gfx, self.fg_blur, -125.0, 170.0, 1.0);
        gfx.set_blend(Blend::Normal);

        gfx.set_blend(Blend::Add);
        if let Some((a, an)) = &self.dipshit_blur {
            if let Some(f) = an.frame_at(t, 24.0, true) {
                a.draw(gfx, f, 419.0, -65.0 + slide(220.0, 1.2), 1.0, WHITE);
            }
        }
        if let Some((a, an)) = &self.dipshit_backing {
            if let Some(f) = an.frame_at(t, 24.0, true) {
                a.draw(gfx, f, 423.0, -17.0 + slide(210.0, 1.1), 1.0, WHITE);
            }
        }
        gfx.set_blend(Blend::Normal);
        if let Some(c) = self.choose {
            gfx.sprite(c, 426.0, -13.0 + slide(200.0, 1.0), 1.0, WHITE);
        }

        // Nametag: scaled 0.77 about its midpoint (1008, 100).
        let exit_tag = exit.map(|e| 80.0 * ease("backIn", e / 0.8)).unwrap_or(0.0);
        let tag_y = 100.0 + 200.0 * (1.0 - ease("expoOut", t / 1.0)) + exit_tag;
        if let Some(Some(tag)) = self.nametags.get(&self.cur_char) {
            let (w, h) = gfx.texture_size(*tag);
            let (dw, dh) = (w * 0.77, h * 0.77);
            gfx.image(*tag, [0.0, 0.0, w, h], 1008.0 - dw / 2.0, tag_y - dh / 2.0, dw, dh, WHITE);
        }

        self.draw_cursors(gfx, exit);
        self.draw_icons(gfx, slide(300.0, 1.0));

        // transitionGradient: flipped, stretched to 1280 wide.
        if let Some(g) = self.gradient {
            let y = match exit {
                Some(e) => -720.0 + 570.0 * ease("backIn", e / 0.8),
                None => -720.0 * ease("expoOut", t / 1.0),
            };
            if y > -720.0 {
                gfx.quad_uv(g, [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]], [[0.0, y], [VIEW_W, y], [VIEW_W, y + 720.0], [0.0, y + 720.0]], WHITE);
            }
        }

        // BlueFade: red and green fade faster than blue.
        let f = match exit {
            Some(e) => 1.0 - ease("quadIn", e / 0.8),
            None => self.fade.at(t),
        };
        if f < 1.0 {
            gfx.set_blend(Blend::Multiply);
            gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [f * f, f * f, f, 1.0]);
            gfx.set_blend(Blend::Normal);
        }
        if let Some(at) = self.flash_at {
            super::draw_flash(gfx, t - at, self.flash_dur);
        }
    }

    fn draw_cursors(&self, gfx: &mut Gfx, exit: Option<f32>) {
        let alpha = exit.map(|e| 1.0 - ease("expoOut", e / 0.8)).unwrap_or(1.0);
        if alpha <= 0.0 {
            return;
        }
        let t = self.time;
        let [mx, my] = self.cursor_main;
        if self.confirmed {
            if let Some((a, an)) = &self.cursor_confirm {
                if let Some(f) = an.frame_at(t, 24.0, true) {
                    a.draw(gfx, f, mx - 2.0, my - 4.0, 1.0, [1.0, 1.0, 1.0, alpha]);
                }
            }
        } else if let Some(sel) = self.selector {
            gfx.set_blend(Blend::Add);
            let dark = hex(0x3C74F7);
            let light = hex(0x3EBBFF);
            gfx.sprite(sel, self.cursor_dark[0], self.cursor_dark[1], 1.0, [dark[0], dark[1], dark[2], alpha]);
            gfx.sprite(sel, self.cursor_light[0], self.cursor_light[1], 1.0, [light[0], light[1], light[2], alpha]);
            gfx.set_blend(Blend::Normal);
            // FlxTween.color PINGPONG between 0xFFFF00 and 0xFFCC00 every 0.2 s.
            let k = (t / 0.2) % 2.0;
            let k = if k > 1.0 { 2.0 - k } else { k };
            gfx.sprite(sel, mx, my, 1.0, [1.0, 1.0 - 0.2 * k, 0.0, alpha]);
        }
        if let (Some(d), Some((a, an))) = (self.denied_at, &self.cursor_denied) {
            if let Some(f) = an.frame_at(t - d, 24.0, false) {
                if t - d < an.duration(24.0) {
                    a.draw(gfx, f, mx - 2.0, my - 4.0, 1.0, [1.0, 1.0, 1.0, alpha]);
                }
            }
        }
    }

    fn draw_icons(&self, gfx: &mut Gfx, y_off: f32) {
        let sel = self.current_index();
        let t = self.time;
        for (i, c) in self.cells.iter().enumerate() {
            let x = GRID_X + (i % 3) as f32 * SPREAD[0];
            let y = GRID_Y + (i / 3) as f32 * SPREAD[1] + y_off;
            match c {
                Cell::Icon { id, confirm_at } => {
                    let Some(Some(icon)) = self.icons.get(id) else { continue };
                    let (cx, cy) = (x + 64.0, y + 64.0);
                    let confirm = confirm_at.map(|a| t - a);
                    if i == sel {
                        // selectedBizz: a black drop shadow and a white glow.
                        icon.draw_centered(gfx, cx + 3.5, cy + 3.5, 2.6, t, confirm, fill([0.0, 0.0, 0.0, 1.0]));
                        for k in 0..8 {
                            let a = k as f32 * std::f32::consts::FRAC_PI_4;
                            icon.draw_centered(gfx, cx + 2.0 * a.cos(), cy + 2.0 * a.sin(), 2.6, t, confirm, fill(WHITE));
                        }
                        icon.draw_centered(gfx, cx, cy, 2.6, t, confirm, WHITE);
                    } else {
                        icon.draw_centered(gfx, cx, cy, 2.0, t, None, WHITE);
                    }
                }
                Cell::Lock { play: Some(p) } => {
                    let Some(a) = &self.lock_atlas else { continue };
                    // Centre the lock's first frame on the cell.
                    let b = self.lock_bounds;
                    let m = Affine::translate(x + 64.0 - (b[0] + b[2]) / 2.0, y + 64.0 - (b[1] + b[3]) / 2.0);
                    let tint = hex(LOCK_COLORS[i]);
                    a.draw_opts(gfx, p.range.tl, p.frame(a.fps), &m, WHITE, DrawOpts { tint_layer: Some(("color", tint)), ..Default::default() });
                }
                Cell::Lock { play: None } => {}
            }
        }
    }
}
