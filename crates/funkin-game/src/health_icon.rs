//! Port of `HealthIcon` (the character faces riding the health bar).

use crate::assets::Assets;
use crate::gfx::{Gfx, TexId};

/// `HealthIcon.HEALTH_ICON_SIZE`.
const ICON_SIZE: f32 = 150.0;
const PIXEL_ICON_SIZE: f32 = 32.0;
const BOP_SCALE: f32 = 0.2;
const POSITION_OFFSET: f32 = 26.0;
const WINNING_THRESHOLD: f64 = 0.8 * 2.0;
const LOSING_THRESHOLD: f64 = 0.2 * 2.0;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    Winning,
    Losing,
}

pub struct HealthIcon {
    tex: TexId,
    /// Frame size in the legacy grid (150, or 32 for pixel icons).
    frame: f32,
    /// Frames in the grid: idle, losing, (winning).
    frames: usize,
    cols: usize,
    /// `healthIcon.scale`.
    size: f32,
    flip: bool,
    offset: [f32; 2],
    should_bop: bool,
    /// Player 1 (bf) or 2 (dad).
    player: bool,
    /// Current graphic width (`setGraphicSize(width, 0)`).
    width: f32,
    /// Bop tween: (from, elapsed, duration).
    bop: Option<(f32, f32, f32)>,
    state: State,
}

/// `healthIcon` from `data/characters/<id>.json`, with `CharacterData` defaults.
struct IconData {
    id: String,
    scale: f32,
    flip_x: bool,
    is_pixel: bool,
    offsets: [f32; 2],
    should_bop: bool,
}

fn icon_data(assets: &Assets, character: &str) -> IconData {
    let mut d = IconData { id: character.to_owned(), scale: 1.0, flip_x: false, is_pixel: false, offsets: [0.0, 0.0], should_bop: true };
    let path = assets.root().join("data").join("characters").join(format!("{character}.json"));
    let Some(v) = funkin_core::vfs::read_to_string(&path).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()) else {
        return d;
    };
    let Some(h) = v.get("healthIcon") else { return d };
    if let Some(id) = h.get("id").and_then(|x| x.as_str()) {
        d.id = id.to_owned();
    }
    if let Some(s) = h.get("scale").and_then(|x| x.as_f64()) {
        d.scale = s as f32;
    }
    d.flip_x = h.get("flipX").and_then(|x| x.as_bool()).unwrap_or(false);
    d.is_pixel = h.get("isPixel").and_then(|x| x.as_bool()).unwrap_or(false);
    d.should_bop = h.get("shouldBop").and_then(|x| x.as_bool()).unwrap_or(true);
    if let Some(o) = h.get("offsets").and_then(|x| x.as_array()).filter(|o| o.len() == 2) {
        d.offsets = [o[0].as_f64().unwrap_or(0.0) as f32, o[1].as_f64().unwrap_or(0.0) as f32];
    }
    d
}

impl HealthIcon {
    /// `new HealthIcon(...)` + `BaseCharacter.initHealthIcon`.
    pub fn load(gfx: &mut Gfx, assets: &Assets, character: &str, player: bool) -> Option<Self> {
        let d = icon_data(assets, character);
        let path = |id: &str| assets.image_path(&format!("default:icons/icon-{id}"));
        let mut p = path(&d.id);
        let mut is_pixel = d.is_pixel;
        if !funkin_core::vfs::is_file(&p) {
            log::warn!("no health icon for {}; using face", d.id);
            p = path("face");
            is_pixel = false;
        }
        let tex = gfx.load_texture_filtered(&p, !is_pixel).map_err(|e| log::warn!("health icon: {e}")).ok()?;
        let (tw, th) = gfx.texture_size(tex);
        let frame = if is_pixel { PIXEL_ICON_SIZE } else { ICON_SIZE };
        let cols = ((tw / frame).floor() as usize).max(1);
        let rows = ((th / frame).floor() as usize).max(1);
        // BF looks the other way.
        let flip = if player { !d.flip_x } else { d.flip_x };
        Some(Self {
            tex,
            frame,
            frames: cols * rows,
            cols,
            size: d.scale,
            flip,
            offset: d.offsets,
            should_bop: d.should_bop,
            player,
            width: ICON_SIZE * d.scale,
            bop: None,
            state: State::Idle,
        })
    }

    fn base_width(&self) -> f32 {
        ICON_SIZE * self.size
    }

    /// `onStepHit`: bop every beat.
    pub fn on_step(&mut self, step: i64, step_ms: f64) {
        if !self.should_bop || step.rem_euclid(4) != 0 {
            return;
        }
        let from = self.width + self.base_width() * BOP_SCALE;
        self.width = from.floor();
        self.bop = Some((from, 0.0, (step_ms as f32 * 0.002).min(0.175)));
    }

    /// `update` / `updateHealthIcon`: tween the bop and pick the face.
    pub fn update(&mut self, dt: f32, health: f64) {
        let base = self.base_width();
        if let Some((from, t, dur)) = &mut self.bop {
            *t += dt;
            let k = if *dur > 0.0 { (*t / *dur).min(1.0) } else { 1.0 };
            self.width = (*from + (base - *from) * k).floor();
            if k >= 1.0 {
                self.bop = None;
            }
        }
        let h = if self.player { health } else { 2.0 - health };
        let can_win = self.frames >= 3;
        self.state = match self.state {
            State::Idle if h < LOSING_THRESHOLD => State::Losing,
            State::Idle if h > WINNING_THRESHOLD && can_win => State::Winning,
            State::Winning if h < WINNING_THRESHOLD => State::Idle,
            State::Losing if h > LOSING_THRESHOLD => State::Idle,
            s => s,
        };
    }

    /// Draw on a bar whose fill area is at `(bar_x, bar_y)`, `bar_w` wide,
    /// showing `value` (0..2, `healthLerp`).
    pub fn draw(&self, gfx: &mut Gfx, bar_x: f32, bar_y: f32, bar_w: f32, value: f64) {
        let scale = self.width / self.frame;
        let (w, h) = (self.width, self.frame * scale);
        let along = bar_w * ((1.0 - value / 2.0) as f32);
        let x = if self.player { bar_x + along - POSITION_OFFSET } else { bar_x + along - (w - POSITION_OFFSET) };
        let y = bar_y - h / 2.0;
        let (x, y) = (x - self.offset[0], y - self.offset[1]);
        let i = match self.state {
            State::Idle => 0,
            State::Losing => 1,
            State::Winning => 2,
        }
        .min(self.frames - 1);
        let src = [(i % self.cols) as f32 * self.frame, (i / self.cols) as f32 * self.frame, self.frame, self.frame];
        let (x0, x1) = if self.flip { (x + w, x) } else { (x, x + w) };
        gfx.quad(self.tex, src, [[x0, y], [x1, y], [x1, y + h], [x0, y + h]], [1.0, 1.0, 1.0, 1.0]);
    }
}
