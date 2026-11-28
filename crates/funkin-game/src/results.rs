//! The results screen after a song or week (`ResultState`, with its
//! `ResultScore`, `TallyCounter` and `ClearPercentCounter` parts).
//!
//! Everything runs off one clock (`t`, seconds since the screen opened), with
//! the original's timers and tweens at the same times. The player character's
//! rank animations come from `data/players/<id>.json`; those with a
//! `scriptClass` are made by the character's script (`ScriptedFunkinSprite`).

use std::collections::HashMap;
use std::sync::Arc;

use funkin_core::camera::ease;
use funkin_core::scoring::{tally_completion, ScoringRank, Tallies};
use serde::Deserialize;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{Affine, Color, Gfx, TexId};
use crate::menus::{anim, Music};
use crate::world::World;

const W: f32 = 1280.0;
const H: f32 = 720.0;
const WHITE: Color = [1.0; 4];

/// What the results screen was opened with (`ResultsStateParams`).
#[derive(Debug, Clone)]
pub struct ResultsParams {
    pub story: bool,
    /// "Song by Artist", or the week's name.
    pub title: String,
    /// The player's stage character (`bf`, `pico-playable`, ...).
    pub character_id: String,
    pub difficulty: String,
    pub score: i64,
    pub tallies: Tallies,
    pub new_highscore: bool,
    /// The rank saved before this play (for the freeplay rank-up).
    pub prev_rank: Option<ScoringRank>,
    /// Not practice mode or bot play.
    pub score_valid: bool,
}

/// Where the results screen goes when it's left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultsExit {
    /// Under the sticker transition (back to freeplay / the story menu).
    Stickers,
    /// A better rank: fade to black, then freeplay (`fromResults`).
    RankUp,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ResultsAnimData {
    render_type: String,
    asset_path: Option<String>,
    script_class: Option<String>,
    start_frame_label: Option<String>,
    filter: Option<String>,
    z_index: Option<i32>,
    offsets: Option<[f64; 2]>,
    scale: Option<f64>,
    delay: Option<f64>,
    looped: Option<bool>,
    loop_frame: Option<u32>,
    loop_frame_label: Option<String>,
    sound: Option<String>,
    apply_stage_matrix: Option<bool>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ResultsData {
    music: HashMap<String, String>,
    perfect_gold: Vec<Option<ResultsAnimData>>,
    perfect: Vec<Option<ResultsAnimData>>,
    excellent: Vec<Option<ResultsAnimData>>,
    great: Vec<Option<ResultsAnimData>>,
    good: Vec<Option<ResultsAnimData>>,
    loss: Vec<Option<ResultsAnimData>>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct PlayerJson {
    owned_chars: Vec<String>,
    results: Option<ResultsData>,
}

fn rank_key(r: ScoringRank) -> &'static str {
    match r {
        ScoringRank::PerfectGold => "PERFECT_GOLD",
        ScoringRank::Perfect => "PERFECT",
        ScoringRank::Excellent => "EXCELLENT",
        ScoringRank::Great => "GREAT",
        ScoringRank::Good => "GOOD",
        ScoringRank::Shit => "SHIT",
    }
}

fn bf_delay(r: ScoringRank) -> f64 {
    match r {
        ScoringRank::Excellent => 97.0 / 24.0,
        _ => 95.0 / 24.0,
    }
}

fn flash_delay(r: ScoringRank) -> f64 {
    match r {
        ScoringRank::PerfectGold | ScoringRank::Perfect => 129.0 / 24.0,
        ScoringRank::Excellent => 122.0 / 24.0,
        ScoringRank::Great => 109.0 / 24.0,
        ScoringRank::Good => 107.0 / 24.0,
        ScoringRank::Shit => 186.0 / 24.0,
    }
}

fn highscore_delay(r: ScoringRank) -> f64 {
    match r {
        ScoringRank::PerfectGold | ScoringRank::Perfect | ScoringRank::Excellent => 140.0 / 24.0,
        ScoringRank::Great => 129.0 / 24.0,
        ScoringRank::Good => 127.0 / 24.0,
        ScoringRank::Shit => 207.0 / 24.0,
    }
}

fn rank_text(r: ScoringRank) -> &'static str {
    match r {
        ScoringRank::PerfectGold | ScoringRank::Perfect => "PERFECT",
        ScoringRank::Excellent => "EXCELLENT",
        ScoringRank::Great => "GREAT",
        ScoringRank::Good => "GOOD",
        ScoringRank::Shit => "LOSS",
    }
}

/// `FlxGradient.createGradientFlxSprite(w, h, [top, bottom], 90)`.
fn gradient(gfx: &mut Gfx, top: u32, bottom: u32) -> TexId {
    let c = |v: u32, s: u32| ((v >> s) & 255) as f32;
    let mut px = Vec::with_capacity(256 * 4);
    for i in 0..256 {
        let k = i as f32 / 255.0;
        for s in [16, 8, 0] {
            px.push((c(top, s) + (c(bottom, s) - c(top, s)) * k).round() as u8);
        }
        px.push(255);
    }
    gfx.upload_rgba(1, 256, &px)
}

fn hex(c: u32) -> Color {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0]
}

/// Draw part of an atlas frame at `(x, y)` (its untrimmed top-left), cutting
/// off whatever is left of `clip_x` (`clipRect` / `LeftMaskShader`).
fn draw_frame(gfx: &mut Gfx, atlas: &Atlas, frame: usize, x: f32, y: f32, color: Color, clip_x: Option<f32>) {
    let f = &atlas.frames[frame];
    let (mut dx, dy) = (x + f.frame_x * -1.0, y + f.frame_y * -1.0);
    let (mut w, h) = (f.w, f.h);
    let (tw, th) = gfx.texture_size(atlas.texture);
    // TL, TR, BR, BL texture coordinates of the upright image.
    let mut uv = if f.rotated {
        let (u0, v0, u1, v1) = (f.x / tw, f.y / th, (f.x + f.h) / tw, (f.y + f.w) / th);
        [[u1, v0], [u1, v1], [u0, v1], [u0, v0]]
    } else {
        let (u0, v0, u1, v1) = (f.x / tw, f.y / th, (f.x + f.w) / tw, (f.y + f.h) / th);
        [[u0, v0], [u1, v0], [u1, v1], [u0, v1]]
    };
    if let Some(cx) = clip_x {
        if dx + w <= cx {
            return;
        }
        if dx < cx {
            let k = (cx - dx) / w;
            let lerp = |a: [f32; 2], b: [f32; 2]| [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k];
            uv[0] = lerp(uv[0], uv[1]);
            uv[3] = lerp(uv[3], uv[2]);
            w -= cx - dx;
            dx = cx;
        }
    }
    gfx.quad_uv(atlas.texture, uv, [[dx, dy], [dx + w, dy], [dx + w, dy + h], [dx, dy + h]], color);
}

/// A sparrow sprite on the results screen (`FunkinSprite.createSparrow`).
struct Spr {
    atlas: Atlas,
    anims: Vec<(String, Anim)>,
    cur: usize,
    /// Frame the current animation started on.
    start: usize,
    /// Seconds since it started.
    time: f32,
    x: f32,
    y: f32,
    visible: bool,
}

impl Spr {
    fn load(gfx: &mut Gfx, assets: &Assets, key: &str, x: f32, y: f32) -> Option<Self> {
        let base = assets.level_image(&format!("shared:{key}"), None, "png").with_extension("");
        match Atlas::load(gfx, &base) {
            Ok(atlas) => Some(Self { atlas, anims: Vec::new(), cur: 0, start: 0, time: 0.0, x, y, visible: false }),
            Err(e) => {
                log::warn!("results: {e}");
                None
            }
        }
    }

    fn add(&mut self, name: &str, prefix: &str) {
        let a = self.atlas.prefix(prefix);
        self.anims.push((name.to_owned(), a));
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.anims.iter().position(|(n, _)| n == name)
    }

    /// `animation.play(name, true, false, frame)`.
    fn play(&mut self, name: &str, frame: usize) {
        if let Some(i) = self.index(name) {
            self.cur = i;
            self.start = frame;
            self.time = 0.0;
        }
    }

    fn len(&self) -> usize {
        self.anims.get(self.cur).map(|(_, a)| a.frames.len()).unwrap_or(0)
    }

    /// The animation ran past its last frame (`onFinish`).
    fn done(&self) -> bool {
        self.start as f32 + self.time * 24.0 >= self.len() as f32
    }

    fn frame(&self) -> Option<usize> {
        let (_, a) = self.anims.get(self.cur)?;
        let i = (self.start + (self.time * 24.0) as usize).min(a.frames.len().checked_sub(1)?);
        Some(a.frames[i])
    }

    fn draw(&self, gfx: &mut Gfx, color: Color, clip_x: Option<f32>) {
        if !self.visible {
            return;
        }
        if let Some(f) = self.frame() {
            draw_frame(gfx, &self.atlas, f, self.x, self.y, color, clip_x);
        }
    }
}

/// `ScoreNum`: one digit of the score, which shuffles before landing.
struct ScoreNum {
    anim: String,
    anim_t: f32,
    anim_start: usize,
    glow: bool,
    digit: i32,
    final_digit: i32,
    final_delay: f32,
    /// Shuffle timer: (time to next tick, loops left).
    shuffle: Option<(f32, u32)>,
    /// The landing tween's elapsed time.
    tween: Option<f32>,
    /// Restart the animation at this time (after the tween).
    replay_in: Option<f32>,
    start_in: Option<f32>,
}

const NUMS: [&str; 11] = ["ZERO", "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE", "DISABLED"];

impl ScoreNum {
    fn new() -> Self {
        let mut s = Self { anim: String::new(), anim_t: 0.0, anim_start: 0, glow: true, digit: 10, final_digit: 10, final_delay: 0.0, shuffle: None, tween: None, replay_in: None, start_in: None };
        s.set_digit(10);
        s.play("DISABLED", 0);
        s
    }

    fn play(&mut self, name: &str, frame: usize) {
        self.anim = name.to_owned();
        self.anim_t = 0.0;
        self.anim_start = frame;
    }

    fn set_digit(&mut self, val: i32) {
        if (0..=10).contains(&val) && !self.anim.is_empty() && self.anim != NUMS[val as usize] {
            if self.glow {
                self.play(NUMS[val as usize], 0);
                self.glow = false;
            } else {
                self.play(NUMS[val as usize], 4);
            }
        }
        self.digit = val;
    }

    fn set_final(&mut self, val: i32) {
        self.play("GONE", 0);
        self.final_digit = val;
    }

    fn update(&mut self, dt: f32) {
        self.anim_t += dt;
        if let Some(t) = &mut self.start_in {
            *t -= dt;
            if *t <= 0.0 {
                self.start_in = None;
                // playAnim + shuffle.
                let d = self.digit.clamp(0, 10) as usize;
                self.play(NUMS[d], 0);
                let loops = ((41.0f64 / 24.0) / (1.0 / 24.0)) as u32;
                self.shuffle = Some((1.0 / 24.0, loops));
            }
        }
        if let Some((next, left)) = &mut self.shuffle {
            *next -= dt;
            if *next <= 0.0 {
                *next += 1.0 / 24.0;
                *left -= 1;
                let done = *left == 0;
                if done {
                    self.shuffle = None;
                }
                let mut d = self.digit + 1;
                if d > 9 {
                    d = 0;
                }
                self.set_digit(d.max(0));
                if done {
                    self.tween = Some(0.0);
                }
            }
        }
        if let Some(t) = &mut self.tween {
            *t += dt;
            let dur = 23.0 / 24.0;
            let k = (*t / dur).min(1.0);
            let done = k >= 1.0;
            let v = ease("quadOut", k as f64) * self.final_digit as f64;
            if done {
                self.tween = None;
                self.replay_in = Some(self.final_delay / 24.0);
            }
            self.set_digit(v.floor() as i32);
        }
        if let Some(t) = &mut self.replay_in {
            *t -= dt;
            if *t <= 0.0 {
                self.replay_in = None;
                let a = self.anim.clone();
                self.play(&a, 0);
            }
        }
    }
}

/// `ClearPercentCounter`.
struct ClearCounter {
    small: bool,
    x: f32,
    y: f32,
    cur: i32,
    /// Digit sprites made so far: whether each uses the "Right" font.
    made: Vec<bool>,
    flash: bool,
    alpha: f32,
    visible: bool,
}

impl ClearCounter {
    fn new(x: f32, y: f32, start: i32, small: bool) -> Self {
        let mut c = Self { small, x, y, cur: start, made: Vec::new(), flash: false, alpha: 1.0, visible: true };
        c.sync();
        c
    }

    fn digits(&self) -> Vec<i32> {
        let mut v = Vec::new();
        let mut n = self.cur.max(0);
        while n > 0 {
            v.push(n % 10);
            n /= 10;
        }
        if v.is_empty() {
            v.push(0);
        }
        v.reverse();
        v
    }

    /// `drawNumbers`: new digits get their font when they first appear.
    fn sync(&mut self) {
        let n = self.digits().len();
        while self.made.len() < n {
            let idx = self.made.len() + 1;
            let variant = if n == 3 { idx >= 2 } else { idx >= 1 };
            self.made.push(variant);
        }
    }

    fn set(&mut self, v: i32) {
        self.cur = v;
        self.sync();
    }

    fn height(&self, l: &Lib) -> f32 {
        let t = if self.small { l.clear_text_small } else { l.clear_text };
        l.tex_size[t].1
    }

    fn draw(&self, gfx: &mut Gfx, l: &Lib, clip: Option<f32>) {
        if !self.visible || self.alpha <= 0.0 {
            return;
        }
        let a = self.alpha;
        let text = if self.small { l.clear_text_small } else { l.clear_text };
        let (tw, th) = l.tex_size[text];
        let text_x = self.x + if self.small { 40.0 } else { 0.0 };
        let atlas = Atlas::single(l.texs[text], tw, th);
        draw_frame(gfx, &atlas, 0, text_x, self.y, [1.0, 1.0, 1.0, a], clip);
        let digits = self.digits();
        let n = digits.len();
        let off: i32 = if n == 1 { 1 } else if n == 3 { -1 } else { 0 };
        let (size, hoff) = if self.small { (32.0, -4.0) } else { (72.0, 0.0) };
        for (ind, d) in digits.iter().enumerate() {
            let k = (ind as i32 + off) as f32;
            let mut xp = k * size;
            xp += if self.small { -24.0 } else { 0.0 };
            let mut yp = k * hoff;
            yp += if self.small { 0.0 } else { 72.0 };
            let atlas = if self.small {
                &l.clear_small
            } else if self.made.get(ind).copied().unwrap_or(true) {
                &l.clear_right
            } else {
                &l.clear_left
            };
            let Some(&f) = atlas.prefix(&format!("number {d} 0")).frames.first() else { continue };
            // PureColor(white) while flashing.
            let c = if self.flash { [1.0, 1.0, 1.0, 2.0 + a] } else { [1.0, 1.0, 1.0, a] };
            draw_frame(gfx, atlas, f, self.x + xp, self.y + yp, c, clip);
        }
    }
}

/// Textures and atlases shared by the parts.
struct Lib {
    texs: Vec<TexId>,
    tex_size: Vec<(f32, f32)>,
    clear_text: usize,
    clear_text_small: usize,
    clear_left: Atlas,
    clear_right: Atlas,
    clear_small: Atlas,
    tally: Atlas,
    score: Atlas,
    font: usize,
    difficulty: Option<usize>,
    rank_vert: Option<usize>,
    rank_hor: Option<usize>,
}

/// A queued rank animation of the player character.
struct CharAnim {
    prop: usize,
    delay: f64,
    start_label: String,
    sound: String,
    sparrow: bool,
    looped: bool,
    loop_frame: Option<u32>,
    loop_label: Option<String>,
    started: bool,
}

pub struct Results {
    params: ResultsParams,
    rank: ScoringRank,
    t: f64,
    lib: Lib,
    bg: TexId,
    bg_flash: TexId,
    flash_alpha: f32,
    flash_visible: bool,
    /// bgFlash's alpha tween: (start time, duration).
    flash_tween: Option<(f64, f64)>,
    sound_system: Option<Spr>,
    results_anim: Option<Spr>,
    ratings_popin: Option<Spr>,
    score_popin: Option<Spr>,
    highscore: Option<Spr>,
    score: Vec<ScoreNum>,
    score_visible: bool,
    /// Counters: (x, y, needed, colour, shown at).
    tallies: Vec<(f32, f32, u32, Color, f64)>,
    clear_big: Option<ClearCounter>,
    clear_target: i32,
    clear_from: i32,
    clear_lerp: i32,
    clear_small: ClearCounter,
    small_added: bool,
    small_flash_until: f64,
    clear_started: f64,
    clear_done: bool,
    moving_at: Option<f64>,
    // The song name strip.
    diff: [f32; 2],
    name: [f32; 2],
    name_w: f32,
    clear_pos: [f32; 2],
    name_visible: bool,
    moving: bool,
    speed: [f32; 2],
    base_speed: [f32; 2],
    /// When the current strip intro started and how long until it scrolls.
    strip_at: f64,
    strip_wait: f64,
    strip_auto: bool,
    speed_tween: Option<f64>,
    bar_y: f32,
    rank_shown: Option<f64>,
    vert_y: f32,
    vert_vel: f32,
    rows_x: [f32; 12],
    rank_bg: f32,
    rank_bg_tween: Option<f64>,
    world: World,
    scripts: Option<crate::scripting::Scripts>,
    /// Keeps the script instances alive.
    _instances: Vec<funkin_script::Value>,
    chars: Vec<CharAnim>,
    music_started: bool,
    music_id: Option<String>,
    exiting: Option<ResultsExit>,
    sfx_scroll: Option<Arc<Pcm>>,
    sfx_confirm: Option<Arc<Pcm>>,
    pub tex_mark: usize,
}

impl Results {
    pub fn new(gfx: &mut Gfx, assets: &Assets, audio: &Audio, params: ResultsParams, naughty: bool) -> Self {
        let rank = funkin_core::scoring::calculate_rank(&params.tallies).unwrap_or(ScoringRank::Shit);
        let tex_mark = gfx.texture_mark();
        let mut texs = Vec::new();
        let mut tex_size = Vec::new();
        let mut img = |gfx: &mut Gfx, key: &str| -> Option<usize> {
            match gfx.load_texture(&assets.level_image(&format!("shared:{key}"), None, "png")) {
                Ok(t) => {
                    texs.push(t);
                    tex_size.push(gfx.texture_size(t));
                    Some(texs.len() - 1)
                }
                Err(e) => {
                    log::warn!("results: {e}");
                    None
                }
            }
        };
        let blank = gfx.white();
        let clear_text = img(gfx, "resultScreen/clearPercent/clearPercentText").unwrap_or(0);
        let clear_text_small = img(gfx, "resultScreen/clearPercent/clearPercentTextSmall").unwrap_or(0);
        let font = img(gfx, "resultScreen/tardlingSpritesheet").unwrap_or(0);
        let difficulty = img(gfx, &format!("resultScreen/diff_{}", params.difficulty));
        let rank_vert = img(gfx, &format!("resultScreen/rankText/rankText{}", rank_text(rank)));
        let rank_hor = img(gfx, &format!("resultScreen/rankText/rankScroll{}", rank_text(rank)));
        if texs.is_empty() {
            texs.push(blank);
            tex_size.push((1.0, 1.0));
        }
        let atlas = |gfx: &mut Gfx, key: &str| {
            let base = assets.level_image(&format!("shared:{key}"), None, "png").with_extension("");
            Atlas::load(gfx, &base).unwrap_or_else(|e| {
                log::warn!("results: {e}");
                Atlas { texture: blank, frames: Vec::new() }
            })
        };
        let lib = Lib {
            clear_left: atlas(gfx, "resultScreen/clearPercent/clearPercentNumberLeft"),
            clear_right: atlas(gfx, "resultScreen/clearPercent/clearPercentNumberRight"),
            clear_small: atlas(gfx, "resultScreen/clearPercent/clearPercentNumberSmall"),
            tally: atlas(gfx, "resultScreen/tallieNumber"),
            score: atlas(gfx, "resultScreen/score-digital-numbers"),
            texs,
            tex_size,
            clear_text,
            clear_text_small,
            font,
            difficulty,
            rank_vert,
            rank_hor,
        };

        let spr = |gfx: &mut Gfx, key: &str, x: f32, y: f32, name: &str, prefix: &str| {
            let mut s = Spr::load(gfx, assets, key, x, y)?;
            s.add(name, prefix);
            Some(s)
        };
        let sound_system = spr(gfx, "resultScreen/soundSystem", -15.0, -180.0, "idle", "sound system");
        let results_anim = spr(gfx, "resultScreen/results", W - 1480.0, -10.0, "result", "results instance 1");
        let ratings_popin = spr(gfx, "resultScreen/ratingsPopin", -135.0, 135.0, "idle", "Categories");
        let score_popin = spr(gfx, "resultScreen/scorePopin", -180.0, 515.0, "score", "tally score");
        let highscore = spr(gfx, "resultScreen/highscoreNew", 44.0, 557.0, "new", "highscoreAnim0");

        // ResultScore(35, 305, 10, score).
        let mut score: Vec<ScoreNum> = (0..10).map(|_| ScoreNum::new()).collect();
        let mut score_start = 0usize;
        {
            let mut n = params.score.clamp(0, 9_999_999_999);
            let mut i = score.len() as i64 - 1;
            while n > 0 && i >= 0 {
                score_start += 1;
                score[i as usize].set_final((n % 10) as i32);
                n /= 10;
                i -= 1;
            }
            while i > 0 {
                score[i as usize].set_digit(10);
                i -= 1;
            }
        }
        let len = score.len();
        for i in len.saturating_sub(score_start)..len {
            let s = &mut score[i];
            s.final_delay = score_start as f32 - (i as f32 - 1.0);
            // Started (i - 1) / 24 after the score shows (a timer of 0 or less fires next frame).
            s.start_in = Some((37.0 / 24.0 + (i as f32 - 1.0) / 24.0).max(37.0 / 24.0));
        }

        let t = &params.tallies;
        let mut hs = 50.0f32;
        let mut tallies = vec![(375.0, hs * 3.0, t.total_notes_hit, WHITE), (375.0, hs * 4.0, t.max_combo, WHITE)];
        if t.total_notes_hit >= 1000 {
            tallies[0].0 -= 30.0;
            tallies[1].0 -= 30.0;
        }
        hs += 4.0;
        let ey = 7.0;
        tallies.push((230.0, hs * 5.0 + ey, t.sick, hex(0x89E59E)));
        tallies.push((210.0, hs * 6.0 + ey, t.good, hex(0x89C9E5)));
        tallies.push((190.0, hs * 7.0 + ey, t.bad, hex(0xE6CF8A)));
        tallies.push((220.0, hs * 8.0 + ey, t.shit, hex(0xE68C8A)));
        tallies.push((260.0, hs * 9.0 + ey, t.missed, hex(0xC68AE6)));
        let tallies = tallies.into_iter().enumerate().map(|(i, (x, y, n, c))| (x, y, n, c, 0.3 * i as f64 + 1.2)).collect();

        let bg = gradient(gfx, 0xFECC5C, 0xFDC05C);
        let bg_flash = gradient(gfx, 0xFFF1A6, 0xFFF1BE);

        let name_w = text_width(&params.title);
        let angle = (-4.4f32).to_radians();
        let base_speed = [-angle.cos(), -angle.sin()];

        let mut r = Self {
            rank,
            t: 0.0,
            bg,
            bg_flash,
            flash_alpha: 1.0,
            flash_visible: false,
            flash_tween: None,
            sound_system,
            results_anim,
            ratings_popin,
            score_popin,
            highscore,
            score,
            score_visible: false,
            tallies,
            clear_big: None,
            clear_target: 100,
            clear_from: 0,
            clear_lerp: 0,
            clear_small: ClearCounter::new(W / 2.0 + 300.0, H / 2.0 - 100.0, 100, true),
            small_added: false,
            small_flash_until: 0.0,
            clear_started: 0.0,
            clear_done: false,
            moving_at: None,
            diff: [555.0, 0.0],
            name: [0.0, 0.0],
            name_w,
            clear_pos: [0.0, 0.0],
            name_visible: false,
            moving: false,
            speed: base_speed,
            base_speed,
            strip_at: 0.0,
            strip_wait: 1.0,
            strip_auto: false,
            speed_tween: None,
            bar_y: -bar_height(),
            rank_shown: None,
            vert_y: 100.0,
            vert_vel: 0.0,
            rows_x: [W / 2.0 - 320.0; 12],
            rank_bg: 0.0,
            rank_bg_tween: None,
            world: World::empty(),
            scripts: None,
            _instances: Vec::new(),
            chars: Vec::new(),
            music_started: false,
            music_id: None,
            exiting: None,
            sfx_scroll: assets.sound("default:scrollMenu"),
            sfx_confirm: assets.sound("default:confirmMenu"),
            tex_mark,
            lib,
            params,
        };
        r.clear_small.visible = false;
        r.restart_strip(1.0, false);
        r.build_characters(gfx, assets, audio, naughty);
        gfx.end_scope();
        r
    }

    /// The player character owning `character_id` (`getCharacterOwnerId`), default bf.
    fn player_data(assets: &Assets, character_id: &str) -> PlayerJson {
        let dir = assets.root().join("data").join("players");
        let read = |id: &str| -> Option<PlayerJson> {
            let s = funkin_core::vfs::read_to_string(&dir.join(format!("{id}.json"))).ok()?;
            serde_json::from_str(&s).map_err(|e| log::warn!("players/{id}: {e}")).ok()
        };
        let owner = crate::menus::players::list_players(assets).into_iter().find(|p| p.owned_chars.iter().any(|c| c == character_id)).map(|p| p.id);
        owner.as_deref().and_then(read).or_else(|| read("bf")).unwrap_or_default()
    }

    fn build_characters(&mut self, gfx: &mut Gfx, assets: &Assets, audio: &Audio, naughty: bool) {
        let player = Self::player_data(assets, &self.params.character_id);
        let Some(res) = player.results else { return };
        let list = match self.rank {
            ScoringRank::PerfectGold => &res.perfect_gold,
            ScoringRank::Perfect => &res.perfect,
            ScoringRank::Excellent => &res.excellent,
            ScoringRank::Great => &res.great,
            ScoringRank::Good => &res.good,
            ScoringRank::Shit => &res.loss,
        };
        self.music_id = res.music.get(rank_key(self.rank)).cloned();
        for d in list.iter().flatten() {
            let filter = d.filter.as_deref().unwrap_or("both");
            if filter != "both" && ((naughty && filter != "naughty") || (!naughty && filter != "safe")) {
                continue;
            }
            let off = d.offsets.unwrap_or([0.0, 0.0]);
            let path = d.asset_path.clone().unwrap_or_default();
            let sparrow = match d.render_type.as_str() {
                "animateatlas" => false,
                "sparrow" => true,
                _ => continue,
            };
            let prop = match &d.script_class {
                Some(cls) => {
                    if self.scripts.is_none() {
                        let st = crate::scripting::HostState { naughty, ..Default::default() };
                        self.scripts = Some(crate::scripting::Scripts::load(gfx, assets, audio, Some(&mut self.world), st, self.tex_mark));
                    }
                    let Some(scripts) = self.scripts.as_mut() else { continue };
                    match scripts.new_script_sprite(gfx, &mut self.world, audio, cls, off[0], off[1]) {
                        Some((inst, id)) => {
                            self._instances.push(inst);
                            id
                        }
                        None => continue,
                    }
                }
                None => {
                    let kind = if sparrow { "sparrow" } else { "animateatlas" };
                    let anims = if sparrow { vec![anim("idle", "", 24.0, false, &[])] } else { Vec::new() };
                    match Actor::animated(gfx, assets, &path, kind, &path, None, &anims, true) {
                        Ok(mut a) => {
                            a.x = off[0] as f32;
                            a.y = off[1] as f32;
                            self.world.new_prop(a)
                        }
                        Err(e) => {
                            log::warn!("results: {path}: {e}");
                            continue;
                        }
                    }
                }
            };
            let Some(a) = self.world.props.get_mut(prop) else { continue };
            if sparrow && !a.has_anim("idle") {
                a.add_sparrow_anim("idle", "", &[], 24.0, false, false);
            }
            if d.apply_stage_matrix.unwrap_or(false) {
                a.apply_stage_matrix = true;
            }
            a.z_index = d.z_index.unwrap_or(500);
            let s = d.scale.unwrap_or(1.0) as f32;
            a.scale = [s, s];
            // No updateHitbox after scaling: the offset stays zero.
            a.offset = Some([0.0, 0.0]);
            a.should_bop = false;
            a.visible = false;
            self.world.add_prop(prop);
            self.chars.push(CharAnim {
                prop,
                delay: d.delay.unwrap_or(0.0),
                start_label: d.start_frame_label.clone().unwrap_or_default(),
                sound: d.sound.clone().unwrap_or_default(),
                sparrow,
                looped: d.looped.unwrap_or(true),
                loop_frame: d.loop_frame,
                loop_label: d.loop_frame_label.clone(),
                started: false,
            });
        }
        self.world.refresh();
    }
}

/// `BitmapUtil.createResultsBar`'s height.
fn bar_height() -> f32 {
    ((W * 1.011).ceil() / 8.7).ceil()
}

const FONT_LETTERS: &str = "AaBbCcDdEeFfGgHhiIJjKkLlMmNnOoPpQqRrSsTtUuVvWwXxYyZz:1234567890().-";
const GLYPH: [f32; 2] = [49.0, 61.0];
const LETTER_SPACING: f32 = -15.0;

/// Glyphs of the song name: (local x, sheet cell), skipping letters the font lacks.
fn glyphs(text: &str) -> Vec<(f32, Option<usize>)> {
    let mut out = Vec::new();
    let mut x = 0.0;
    for c in text.chars() {
        let cell = FONT_LETTERS.chars().position(|l| l == c);
        if cell.is_none() && c != ' ' {
            continue;
        }
        out.push((x, cell));
        x += GLYPH[0] + LETTER_SPACING;
    }
    out
}

fn text_width(text: &str) -> f32 {
    let n = glyphs(text).len() as f32;
    if n == 0.0 {
        0.0
    } else {
        n * GLYPH[0] + (n - 1.0) * LETTER_SPACING
    }
}

fn play_sfx(audio: &Audio, s: &Option<Arc<Pcm>>) {
    if let Some(s) = s {
        audio.play_sfx(s.clone(), 1.0);
    }
}

impl Results {
    fn diff_size(&self) -> (f32, f32) {
        self.lib.difficulty.map(|i| self.lib.tex_size[i]).unwrap_or((0.0, 0.0))
    }

    /// `timerThenSongName`: drop the difficulty, clear % and song name back in.
    fn restart_strip(&mut self, wait: f64, auto: bool) {
        self.moving = false;
        self.diff[0] = 555.0;
        let (dw, dh) = self.diff_size();
        self.diff[1] = -dh;
        self.clear_pos[0] = self.diff[0] + dw + 60.0;
        self.clear_pos[1] = -self.clear_small.height(&self.lib);
        self.name[1] = -GLYPH[1];
        self.name[0] = self.clear_pos[0] + 94.0;
        self.strip_at = self.t;
        self.strip_wait = wait;
        self.strip_auto = auto;
        self.speed_tween = None;
    }

    fn strip_update(&mut self, prev: f64, dt: f64) {
        let el = self.t - self.strip_at;
        let lift = bar_height() - 148.0;
        let tw = |delay: f64, from: f32, to: f32| -> Option<f32> {
            if el < delay {
                return None;
            }
            let k = ((el - delay) / 0.5).min(1.0);
            Some(from + (to - from) * ease("expoOut", k) as f32)
        };
        if el < 1.5 {
            let (_, dh) = self.diff_size();
            let ch = self.clear_small.height(&self.lib);
            let angle = (-4.4f32).to_radians();
            let bump = -(self.name_w * 0.5) * angle.sin() - 10.0;
            if let Some(y) = tw(0.8, -dh, 122.0 + lift) {
                self.diff[1] = y;
            }
            if let Some(y) = tw(0.85, -ch, 117.0 + lift) {
                self.clear_pos[1] = y;
            }
            if let Some(y) = tw(0.9, -GLYPH[1], 122.0 - 25.0 - bump + lift) {
                self.name[1] = y;
            }
        }
        let at = self.strip_at + self.strip_wait;
        if prev < at && self.t >= at {
            self.speed = [0.0, 0.0];
            self.speed_tween = Some(self.t);
            self.moving = self.strip_auto;
        }
        if let Some(m) = self.moving_at {
            if self.t >= m {
                self.moving_at = None;
                self.moving = true;
            }
        }
        if let Some(st) = self.speed_tween {
            let k = ((self.t - st) / 0.7).min(1.0);
            let e = ease("quadIn", k) as f32;
            self.speed = [self.base_speed[0] * e, self.base_speed[1] * e];
            if k >= 1.0 {
                self.speed_tween = None;
            }
        }
        if self.moving {
            let (sx, sy) = (self.speed[0] * 60.0 * dt as f32, self.speed[1] * 60.0 * dt as f32);
            for p in [&mut self.name, &mut self.diff, &mut self.clear_pos] {
                p[0] += sx;
                p[1] += sy;
            }
            if self.name[0] + self.name_w < 100.0 {
                self.restart_strip(3.0, true);
            }
        }
    }

    fn start_music(&mut self, assets: &Assets, audio: &Audio, music: &mut Music) {
        let id = self.music_id.clone().unwrap_or_else(|| "resultsNORMAL".to_owned());
        let main = assets.music_path(&id);
        let intro = main.with_file_name(format!("{id}-intro.ogg"));
        let loop_pcm = match Pcm::load_ogg(&main) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("results music: {e}");
                return;
            }
        };
        let intro_pcm = if funkin_core::vfs::exists(&intro) { Pcm::load_ogg(&intro).ok().filter(|p| p.rate == loop_pcm.rate) } else { None };
        // The intro plays once, then the loop repeats.
        let (pcm, a) = match intro_pcm {
            Some(mut i) => {
                let a = i.duration_ms();
                i.samples.extend_from_slice(&loop_pcm.samples);
                (i, a)
            }
            None => (loop_pcm, 0.0),
        };
        let b = pcm.duration_ms();
        music.start(audio, &id, Track::new(pcm), 0.0, (a, b));
        music.fade(1.0, 1.0, 0.001);
    }

    /// Step the screen. Returns where to go once it's done leaving.
    pub fn update(&mut self, gfx: &mut Gfx, assets: &Assets, audio: &Audio, music: &mut Music, dt: f64) -> Option<ResultsExit> {
        let prev = self.t;
        self.t += dt;
        let t = self.t;
        let at = |x: f64| prev < x && t >= x;
        let fdt = dt as f32;

        for s in [&mut self.sound_system, &mut self.results_anim, &mut self.ratings_popin, &mut self.score_popin, &mut self.highscore].into_iter().flatten() {
            if s.visible {
                s.time += fdt;
            }
        }
        if at(8.0 / 24.0) {
            if let Some(s) = &mut self.sound_system {
                s.play("idle", 0);
                s.visible = true;
            }
        }
        if at(6.0 / 24.0) {
            if let Some(s) = &mut self.results_anim {
                s.visible = true;
                s.play("result", 0);
            }
        }
        if at(21.0 / 24.0) {
            if let Some(s) = &mut self.ratings_popin {
                s.visible = true;
                s.play("idle", 0);
            }
        }
        if at(36.0 / 24.0) {
            if let Some(s) = &mut self.score_popin {
                s.visible = true;
                s.play("score", 0);
            }
        }
        if at(37.0 / 24.0) {
            self.score_visible = true;
            self.start_rank_tally();
        }
        if at(bf_delay(self.rank)) {
            self.after_rank_tally();
        }
        if at(flash_delay(self.rank)) {
            self.display_rank_text();
        }
        if at(highscore_delay(self.rank)) && self.params.new_highscore {
            if let Some(s) = &mut self.highscore {
                s.visible = true;
                s.play("new", 0);
            }
        }
        if let Some(s) = &mut self.highscore {
            if s.visible && s.done() {
                s.play("new", 16);
            }
        }
        if at(self.rank.music_delay().max(1e-6)) && !self.music_started && self.exiting.is_none() {
            self.music_started = true;
            self.start_music(assets, audio, music);
        }
        // The black bar slides down, then the song name shows.
        let bk = ((t - 3.0 / 24.0) / (7.0 / 24.0)).clamp(0.0, 1.0);
        self.bar_y = -bar_height() * (1.0 - ease("quartOut", bk) as f32);
        if bk >= 1.0 {
            self.name_visible = true;
        }
        self.strip_update(prev, dt);

        if let Some((st, dur)) = self.flash_tween {
            let k = ((t - st) / dur).min(1.0);
            self.flash_alpha = 1.0 - k as f32;
            if k >= 1.0 {
                self.flash_tween = None;
            }
        }
        if self.score_visible {
            for s in &mut self.score {
                s.update(fdt);
            }
        }
        self.clear_update(audio);
        self.clear_small.flash = t < self.small_flash_until;
        if let Some(shown) = self.rank_shown {
            if t - shown >= 30.0 / 24.0 {
                self.vert_vel = -80.0;
            }
            self.vert_y += self.vert_vel * fdt;
            for (i, x) in self.rows_x.iter_mut().enumerate() {
                *x += if i % 2 == 0 { -7.0 } else { 7.0 } * fdt;
            }
        }

        // The player character's animations.
        for a in &mut self.world.props {
            a.update(fdt);
        }
        if let Some(s) = self.scripts.as_mut() {
            s.update(gfx, Some(&mut self.world), audio, dt);
        }
        let start = bf_delay(self.rank);
        for c in &mut self.chars {
            let Some(a) = self.world.props.get_mut(c.prop) else { continue };
            if !c.started {
                if t >= start + c.delay {
                    c.started = true;
                    a.visible = true;
                    if c.sparrow {
                        a.play("idle", true, false);
                    } else if c.start_label.is_empty() {
                        a.play_default(false);
                    } else {
                        a.play(&c.start_label, false, false);
                    }
                    if !c.sound.is_empty() {
                        if let Some(p) = assets.sound(&c.sound) {
                            audio.play_sfx(p, 1.0);
                        }
                    }
                }
                continue;
            }
            if !a.finished() {
                continue;
            }
            if c.sparrow {
                if let Some(n) = c.loop_frame {
                    a.play("idle", true, false);
                    a.set_frame_number(n as usize);
                }
            } else if !c.looped {
                // Paused on its last frame.
            } else if let Some(l) = &c.loop_label {
                a.play(l, true, false);
                a.set_looped(true);
            } else if let Some(n) = c.loop_frame {
                a.play_default(true);
                a.set_frame_number(n as usize);
            }
        }

        if let Some(st) = self.rank_bg_tween {
            let k = ((t - st) / 0.5).min(1.0);
            self.rank_bg = ease("expoOut", k) as f32;
            if k >= 1.0 {
                return self.exiting;
            }
        }
        None
    }

    fn start_rank_tally(&mut self) {
        self.flash_visible = true;
        self.flash_alpha = 1.0;
        self.flash_tween = Some((self.t, 5.0 / 24.0));
        let tl = &self.params.tallies;
        let pct = if tl.total_notes == 0 { 0.0 } else { tally_completion(tl) * 100.0 };
        self.clear_target = pct.floor() as i32;
        self.clear_lerp = (self.clear_target - 36).max(0);
        self.clear_from = self.clear_lerp;
        self.clear_big = Some(ClearCounter::new(W / 2.0 + 190.0, H / 2.0 - 70.0, self.clear_lerp, false));
        self.clear_started = self.t;
    }

    fn clear_update(&mut self, audio: &Audio) {
        let Some(c) = &mut self.clear_big else { return };
        let el = self.t - self.clear_started;
        let dur = 58.0 / 24.0;
        if el < dur {
            let v = (self.clear_from as f64 + (self.clear_target - self.clear_from) as f64 * ease("quartOut", el / dur)).round() as i32;
            c.set(v);
            if v != self.clear_lerp {
                self.clear_lerp = v;
                play_sfx(audio, &self.sfx_scroll);
            }
            return;
        }
        if !self.clear_done {
            self.clear_done = true;
            play_sfx(audio, &self.sfx_confirm);
            c.set(self.clear_target);
        }
        c.flash = el < dur + 0.4;
        let fade = el - dur - 0.25 - 0.5;
        if fade > 0.0 {
            c.alpha = 1.0 - ease("quartOut", (fade / 0.5).min(1.0)) as f32;
            if fade >= 0.5 {
                self.clear_big = None;
            }
        }
    }

    fn after_rank_tally(&mut self) {
        self.small_added = true;
        self.clear_small.visible = true;
        self.clear_small.set(self.clear_target);
        self.small_flash_until = self.t + 0.4;
        self.moving_at = Some(self.t + 2.5);
    }

    fn display_rank_text(&mut self) {
        self.flash_visible = true;
        self.flash_alpha = 1.0;
        self.flash_tween = Some((self.t, 14.0 / 24.0));
        self.rank_shown = Some(self.t);
    }

    /// Enter / Escape. Returns `Stickers` to leave now; a rank-up fades to
    /// black first (`update` returns it when done).
    pub fn accept(&mut self, music: &mut Music) -> Option<ResultsExit> {
        if self.exiting.is_some() {
            return None;
        }
        if music.playing() {
            music.fade(1.0, 0.0, 0.8);
        }
        let rank_up = !self.params.story && self.params.score_valid && self.params.prev_rank.is_none_or(|p| self.rank > p);
        if rank_up {
            self.exiting = Some(ResultsExit::RankUp);
            self.rank_bg_tween = Some(self.t);
            None
        } else {
            self.exiting = Some(ResultsExit::Stickers);
            Some(ResultsExit::Stickers)
        }
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        let l = &self.lib;
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        gfx.set_transform(Affine::IDENTITY);
        let full = [[0.0, 0.0], [W, 0.0], [W, H], [0.0, H]];
        gfx.quad(self.bg, [0.0, 0.0, 1.0, 256.0], full, WHITE);

        // cameraScroll: the rank text rows, at an angle.
        if let (Some(_), Some(hi)) = (self.rank_shown, l.rank_hor) {
            gfx.set_transform(Affine::rotate_deg(-3.8));
            let (rw, rh) = l.tex_size[hi];
            let step = rw + 10.0;
            for (i, &x0) in self.rows_x.iter().enumerate() {
                let y = 50.0 + 135.0 * i as f32 / 2.0 + 10.0;
                let mut x = x0 - ((x0 / step).ceil()) * step;
                while x < W {
                    gfx.image(l.texs[hi], [0.0, 0.0, rw, rh], x, y, rw, rh, WHITE);
                    x += step;
                }
            }
            gfx.set_transform(Affine::IDENTITY);
        }

        // cameraEverything, by zIndex.
        if self.flash_visible && self.flash_alpha > 0.0 {
            gfx.quad(self.bg_flash, [0.0, 0.0, 1.0, 256.0], full, [1.0, 1.0, 1.0, self.flash_alpha]);
        }
        if let Some(c) = &self.clear_big {
            c.draw(gfx, l, None);
        }
        self.world.draw(gfx);
        gfx.set_transform(Affine::IDENTITY);
        if let (Some(shown), Some(vi)) = (self.rank_shown, l.rank_vert) {
            let el = self.t - shown;
            // FlxFlicker(6/24, 2/24): hidden for the second interval.
            let hidden = (2.0 / 24.0..4.0 / 24.0).contains(&el);
            if !hidden {
                let (vw, vh) = l.tex_size[vi];
                let step = vh + 30.0;
                let mut y = self.vert_y - (self.vert_y / step).ceil() * step;
                while y < H {
                    gfx.image(l.texs[vi], [0.0, 0.0, vw, vh], W - 44.0, y, vw, vh, WHITE);
                    y += step;
                }
            }
        }
        if let Some(di) = l.difficulty {
            let (dw, dh) = l.tex_size[di];
            let atlas = Atlas::single(l.texs[di], dw, dh);
            draw_frame(gfx, &atlas, 0, self.diff[0], self.diff[1], WHITE, Some(525.0));
        }
        if self.name_visible {
            self.draw_name(gfx);
        }
        if self.small_added {
            let mut c = ClearCounter { made: self.clear_small.made.clone(), ..ClearCounter::new(0.0, 0.0, 0, true) };
            c.cur = self.clear_small.cur;
            c.flash = self.clear_small.flash;
            c.x = self.clear_pos[0];
            c.y = self.clear_pos[1];
            c.draw(gfx, l, Some(520.0));
        }
        // The black bar (BitmapUtil.createResultsBar).
        {
            let m = Affine::translate(-15.0, self.bar_y).then(&Affine::rotate_deg(-3.8));
            let (bw, bh) = ((W * 1.011).ceil(), bar_height());
            let c = [[0.0, 0.0], [bw, 0.0], [bw, bh], [0.0, bh]].map(|p| m.apply(p));
            gfx.quad(gfx.white(), [0.0, 0.0, 1.0, 1.0], c, [0.0, 0.0, 0.0, 1.0]);
        }
        for s in [&self.sound_system, &self.results_anim, &self.ratings_popin, &self.score_popin, &self.highscore].into_iter().flatten() {
            s.draw(gfx, WHITE, None);
        }
        // Tally counters.
        for &(x, y, needed, color, shown) in &self.tallies {
            if self.t < shown {
                continue;
            }
            let k = ((self.t - shown) / 0.5).min(1.0);
            let n = (needed as f64 * ease("quartOut", k)).round() as u32;
            for (i, ch) in n.to_string().chars().enumerate() {
                let Some(&f) = l.tally.prefix(&format!("{ch} small")).frames.first() else { continue };
                draw_frame(gfx, &l.tally, f, x + i as f32 * 43.0, y, color, None);
            }
        }
        // ResultScore: the group's position is added twice (FlxSpriteGroup.add).
        if self.score_visible {
            for (i, s) in self.score.iter().enumerate() {
                let prefix = match s.anim.as_str() {
                    "DISABLED" | "GONE" => s.anim.clone(),
                    a => format!("{a} DIGITAL"),
                };
                let frames = l.score.prefix(&prefix).frames;
                if frames.is_empty() {
                    continue;
                }
                let fi = (s.anim_start + (s.anim_t * 24.0) as usize).min(frames.len() - 1);
                draw_frame(gfx, &l.score, frames[fi], 70.0 + 65.0 * i as f32, 610.0, WHITE, None);
            }
        }
        if self.rank_bg > 0.0 {
            gfx.rect(0.0, 0.0, W, H, [0.0, 0.0, 0.0, self.rank_bg]);
        }
    }

    /// The song name (`FlxBitmapText`, angle -4.4), cut off left of x = 520.
    fn draw_name(&self, gfx: &mut Gfx) {
        let l = &self.lib;
        let tex = l.texs[l.font];
        let (tw, th) = l.tex_size[l.font];
        let cols = (tw / GLYPH[0]).floor().max(1.0) as usize;
        let (ox, oy) = (self.name_w / 2.0, GLYPH[1] / 2.0);
        let m = Affine::translate(self.name[0] + ox, self.name[1] + oy).then(&Affine::rotate_deg(-4.4)).then(&Affine::translate(-ox, -oy));
        let clip = (520.0 - self.name[0]).max(0.0);
        for (x, cell) in glyphs(&self.params.title) {
            let Some(cell) = cell else { continue };
            let (cx, cy) = ((cell % cols) as f32 * GLYPH[0], (cell / cols) as f32 * GLYPH[1]);
            let (mut x0, x1) = (x, x + GLYPH[0]);
            if x1 <= clip {
                continue;
            }
            let mut u0 = cx;
            if x0 < clip {
                u0 += clip - x0;
                x0 = clip;
            }
            let uv = [[u0 / tw, cy / th], [(cx + GLYPH[0]) / tw, cy / th], [(cx + GLYPH[0]) / tw, (cy + GLYPH[1]) / th], [u0 / tw, (cy + GLYPH[1]) / th]];
            let dst = [[x0, 0.0], [x1, 0.0], [x1, GLYPH[1]], [x0, GLYPH[1]]].map(|p| m.apply(p));
            gfx.quad_uv(tex, uv, dst, WHITE);
        }
    }
}
