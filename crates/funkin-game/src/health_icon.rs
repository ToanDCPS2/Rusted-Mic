//! Port of `HealthIcon` (the character faces riding the health bar).

use crate::assets::Assets;
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{Affine, Gfx, TexId};

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

/// A Sparrow icon (`icon-<id>.xml` next to the image): `HealthIcon.loadAnimationNew`.
struct Sheet {
    atlas: Atlas,
    anims: std::collections::HashMap<&'static str, Anim>,
    /// The animation playing and how long it has played.
    cur: &'static str,
    t: f32,
}

/// `HealthIcon`'s animations: the loops, then the one-shot transitions.
const LOOPS: [&str; 3] = ["idle", "winning", "losing"];
const TRANSITIONS: [&str; 4] = ["toWinning", "toLosing", "fromWinning", "fromLosing"];

pub struct HealthIcon {
    tex: TexId,
    /// Frame size in the legacy grid (150, or 32 for pixel icons).
    frame: f32,
    /// The frame's height (the legacy grid's frames are square).
    frame_h: f32,
    sheet: Option<Sheet>,
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
    /// `bopEvery` (steps; 0 = never) and `bopAngle` (degrees), set by scripts.
    pub bop_every: i64,
    pub bop_angle: f32,
    /// `angle`: a bop's tilt, easing back to 0.
    angle: f32,
    state: State,
    /// The icon's most common colour (Psych Engine's `CoolUtil.dominantColor`),
    /// used for the Psych Engine health bar.
    pub color: [f32; 3],
}

/// `CoolUtil.dominantColor`: the most common colour in the first frame,
/// ignoring fully transparent pixels and pure black.
fn dominant_color(path: &std::path::Path, frame: u32) -> [f32; 3] {
    let fallback = [161.0 / 255.0; 3];
    let Some(img) = funkin_core::vfs::read(path).ok().and_then(|b| image::load_from_memory(&b).ok()) else {
        return fallback;
    };
    let img = img.to_rgba8();
    let mut counts: std::collections::HashMap<[u8; 4], u32> = std::collections::HashMap::new();
    for y in 0..frame.min(img.height()) {
        for x in 0..frame.min(img.width()) {
            let p = img.get_pixel(x, y).0;
            if p[3] != 0 && p != [0, 0, 0, 255] {
                *counts.entry(p).or_default() += 1;
            }
        }
    }
    // Ties go to the lowest colour value, so the pick doesn't vary between runs.
    match counts.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))) {
        Some((c, _)) => [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0],
        None => fallback,
    }
}

/// `healthIcon` from `data/characters/<id>.json`, with `CharacterData` defaults.
pub struct IconData {
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
    // Characters without their own `healthIcon.isPixel` (Spirit) go by the
    // character's `isPixel`.
    d.is_pixel = v.get("isPixel").and_then(|x| x.as_bool()).unwrap_or(false);
    let Some(h) = v.get("healthIcon") else { return d };
    if let Some(id) = h.get("id").and_then(|x| x.as_str()) {
        d.id = id.to_owned();
    }
    if let Some(s) = h.get("scale").and_then(|x| x.as_f64()) {
        d.scale = s as f32;
    }
    d.flip_x = h.get("flipX").and_then(|x| x.as_bool()).unwrap_or(false);
    d.is_pixel = h.get("isPixel").and_then(|x| x.as_bool()).unwrap_or(d.is_pixel);
    d.should_bop = h.get("shouldBop").and_then(|x| x.as_bool()).unwrap_or(true);
    if let Some(o) = h.get("offsets").and_then(|x| x.as_array()).filter(|o| o.len() == 2) {
        d.offsets = [o[0].as_f64().unwrap_or(0.0) as f32, o[1].as_f64().unwrap_or(0.0) as f32];
    }
    d
}

impl IconData {
    /// `toggleOldIcon`'s `bf-old`.
    pub fn old_bf() -> Self {
        Self { id: "bf-old".into(), scale: 1.0, flip_x: false, is_pixel: false, offsets: [0.0, 0.0], should_bop: true }
    }

    /// `SetHealthIconSongEvent`'s value (`id`, `scale`, `flipX`, `isPixel`, `offsetX/Y`, `shouldBop`).
    pub fn from_event(v: &serde_json::Map<String, serde_json::Value>) -> Self {
        let f = |k: &str, d: f64| v.get(k).and_then(|x| x.as_f64()).unwrap_or(d) as f32;
        let b = |k: &str, d: bool| v.get(k).and_then(|x| x.as_bool()).unwrap_or(d);
        Self {
            id: v.get("id").and_then(|x| x.as_str()).unwrap_or("bf").to_owned(),
            scale: f("scale", 1.0),
            flip_x: b("flipX", false),
            is_pixel: b("isPixel", false),
            offsets: [f("offsetX", 0.0), f("offsetY", 0.0)],
            should_bop: b("shouldBop", true),
        }
    }
}

impl HealthIcon {
    /// `new HealthIcon(...)` + `BaseCharacter.initHealthIcon`.
    pub fn load(gfx: &mut Gfx, assets: &Assets, character: &str, player: bool) -> Option<Self> {
        Self::load_data(gfx, assets, icon_data(assets, character), player)
    }

    /// `HealthIcon.configure` with explicit data.
    pub fn load_data(gfx: &mut Gfx, assets: &Assets, d: IconData, player: bool) -> Option<Self> {
        let path = |id: &str| assets.image_path(&format!("default:icons/icon-{id}"));
        let mut p = path(&d.id);
        let mut is_pixel = d.is_pixel;
        if !funkin_core::vfs::is_file(&p) {
            log::warn!("no health icon for {}; using face", d.id);
            p = path("face");
            is_pixel = false;
        }
        // An animated icon: Sparrow frames named idle / winning / losing / toWinning ...
        if funkin_core::vfs::is_file(&p.with_extension("xml")) {
            match crate::gfx::atlas::Atlas::load_filtered(gfx, &p.with_extension(""), !is_pixel) {
                Ok(atlas) => {
                    let anims: std::collections::HashMap<&'static str, Anim> = LOOPS.iter().chain(TRANSITIONS.iter()).map(|n| (*n, atlas.prefix(n))).filter(|(_, a)| !a.frames.is_empty()).collect();
                    if let Some(first) = anims.get("idle").and_then(|a| a.frames.first()).map(|&i| &atlas.frames[i]) {
                        let (fw, fh) = (first.frame_w, first.frame_h);
                        let flip = if player { !d.flip_x } else { d.flip_x };
                        let frames = if anims.contains_key("winning") { 3 } else { 2 };
                        return Some(Self {
                            tex: atlas.texture,
                            frame: fw,
                            frame_h: fh,
                            sheet: Some(Sheet { atlas, anims, cur: "idle", t: 0.0 }),
                            frames,
                            cols: 1,
                            size: d.scale,
                            flip,
                            offset: d.offsets,
                            should_bop: d.should_bop,
                            player,
                            // snapToTargetSize: a tall icon is sized by its height.
                            width: if fw > fh { ICON_SIZE * d.scale } else { (ICON_SIZE * d.scale * fw / fh).floor() },
                            bop: None,
                            bop_every: 4,
                            bop_angle: 0.0,
                            angle: 0.0,
                            state: State::Idle,
                            color: dominant_color(&p, fw.min(fh) as u32),
                        });
                    }
                    log::warn!("health icon {}: no idle animation", d.id);
                }
                Err(e) => log::warn!("health icon: {e}"),
            }
        }
        let mut tex = gfx.load_texture_filtered(&p, !is_pixel).map_err(|e| log::warn!("health icon: {e}")).ok()?;
        // A legacy sheet too small for 150 px frames can only be a 32 px pixel icon.
        if !is_pixel && gfx.texture_size(tex).1 < ICON_SIZE {
            is_pixel = true;
            tex = gfx.load_texture_filtered(&p, false).map_err(|e| log::warn!("health icon: {e}")).ok()?;
        }
        let (tw, th) = gfx.texture_size(tex);
        let frame = if is_pixel { PIXEL_ICON_SIZE } else { ICON_SIZE };
        let color = dominant_color(&p, frame as u32);
        let cols = ((tw / frame).floor() as usize).max(1);
        let rows = ((th / frame).floor() as usize).max(1);
        // BF looks the other way.
        let flip = if player { !d.flip_x } else { d.flip_x };
        Some(Self {
            tex,
            frame,
            frame_h: frame,
            sheet: None,
            frames: cols * rows,
            cols,
            size: d.scale,
            flip,
            offset: d.offsets,
            should_bop: d.should_bop,
            player,
            width: ICON_SIZE * d.scale,
            bop: None,
            bop_every: 4,
            bop_angle: 0.0,
            angle: 0.0,
            state: State::Idle,
            color,
        })
    }

    fn base_width(&self) -> f32 {
        ICON_SIZE * self.size
    }

    /// `onStepHit`: bop every beat.
    /// `bop_scale`: `HealthIcon.BOP_SCALE` (a script may change it).
    pub fn on_step(&mut self, step: i64, step_ms: f64, bop_scale: Option<f32>) {
        if self.bop_every == 0 || !self.should_bop || step.rem_euclid(self.bop_every) != 0 {
            return;
        }
        self.angle += self.bop_angle * if self.player { 1.0 } else { -1.0 };
        let from = self.width + self.base_width() * bop_scale.unwrap_or(BOP_SCALE);
        self.width = from.floor();
        self.bop = Some((from, 0.0, (step_ms as f32 * 0.002).min(0.175)));
    }

    /// `update` / `updateHealthIcon`: tween the bop and pick the face.
    /// `auto_update` off (a script's `autoUpdate = false`): the face stays as set.
    pub fn update(&mut self, dt: f32, health: f64, auto_update: bool) {
        // MathUtil.smoothLerpPrecision(angle, 0, elapsed, HEALTH_ICON_ANGLE_HALF_LIFE).
        if self.bop_every != 0 && self.angle != 0.0 && dt > 0.0 {
            self.angle *= 0.01f32.powf(dt / 0.512);
        }
        let base = self.base_width();
        if let Some((from, t, dur)) = &mut self.bop {
            *t += dt;
            let k = if *dur > 0.0 { (*t / *dur).min(1.0) } else { 1.0 };
            self.width = (*from + (base - *from) * k).floor();
            if k >= 1.0 {
                self.bop = None;
            }
        }
        if !auto_update {
            self.advance(dt);
            return;
        }
        let h = if self.player { health } else { 2.0 - health };
        let can_win = self.frames >= 3;
        let next = match self.state {
            State::Idle if h < LOSING_THRESHOLD => State::Losing,
            State::Idle if h > WINNING_THRESHOLD && can_win => State::Winning,
            State::Winning if h < WINNING_THRESHOLD => State::Idle,
            State::Losing if h > LOSING_THRESHOLD => State::Idle,
            s => s,
        };
        self.set_state(next);
        self.advance(dt);
    }

    fn loop_name(s: State) -> &'static str {
        match s {
            State::Idle => "idle",
            State::Winning => "winning",
            State::Losing => "losing",
        }
    }

    /// Change face: an animated icon plays `toLosing` etc. first when it has it.
    fn set_state(&mut self, next: State) {
        if next == self.state {
            return;
        }
        let prev = self.state;
        self.state = next;
        let Some(sh) = &mut self.sheet else { return };
        let via = match (prev, next) {
            (_, State::Winning) => "toWinning",
            (_, State::Losing) => "toLosing",
            (State::Winning, State::Idle) => "fromWinning",
            (State::Losing, State::Idle) => "fromLosing",
            _ => "",
        };
        sh.cur = if sh.anims.contains_key(via) { TRANSITIONS.into_iter().find(|n| *n == via).unwrap_or("idle") } else { Self::loop_name(next) };
        sh.t = 0.0;
    }

    /// Run an animated icon's clock; a finished transition goes on to the loop.
    fn advance(&mut self, dt: f32) {
        let state = self.state;
        let Some(sh) = &mut self.sheet else { return };
        sh.t += dt;
        if TRANSITIONS.contains(&sh.cur) && sh.anims.get(sh.cur).is_none_or(|a| sh.t >= a.duration(24.0)) {
            sh.cur = Self::loop_name(state);
            sh.t = 0.0;
        }
    }

    /// Where `updatePosition` puts the icon on a bar whose fill area is at
    /// `(bar_x, bar_y)`, `bar_w` wide, showing `value` (0..2, `healthLerp`):
    /// (x, y, width, height).
    pub fn place(&self, bar_x: f32, bar_y: f32, bar_w: f32, value: f64) -> [f32; 4] {
        let scale = self.width / self.frame;
        let (w, h) = (self.width, self.frame_h * scale);
        let along = bar_w * ((1.0 - value / 2.0) as f32);
        let x = if self.player { bar_x + along - POSITION_OFFSET } else { bar_x + along - (w - POSITION_OFFSET) };
        let y = bar_y - h / 2.0;
        [x - self.offset[0], y - self.offset[1], w, h]
    }

    /// `animation.play(name)` from a script.
    pub fn set_face(&mut self, name: &str) {
        let s = match name {
            "losing" => State::Losing,
            "winning" if self.frames >= 3 => State::Winning,
            _ => State::Idle,
        };
        self.set_state(s);
    }

    /// Draw with the top-left at (x, y); `flip` overrides the character's.
    pub fn draw_at(&self, gfx: &mut Gfx, x: f32, y: f32, alpha: f32, flip: Option<bool>) {
        let scale = self.width / self.frame;
        let (w, h) = (self.width, self.frame_h * scale);
        // `angle` turns the sprite about its centre.
        let spin = |m: Affine| -> Affine {
            if self.angle == 0.0 {
                return m;
            }
            Affine::translate(x + w / 2.0, y + h / 2.0).then(&Affine::rotate_deg(self.angle)).then(&Affine::translate(-x - w / 2.0, -y - h / 2.0)).then(&m)
        };
        if let Some(sh) = &self.sheet {
            let looped = LOOPS.contains(&sh.cur);
            let anim = sh.anims.get(sh.cur).or_else(|| sh.anims.get(Self::loop_name(self.state))).or_else(|| sh.anims.get("idle"));
            if let Some(f) = anim.and_then(|a| a.frame_at(sh.t, 24.0, looped).or_else(|| a.frames.last().copied())) {
                let m = spin(Affine::translate(x, y).then(&Affine::scale(scale, scale)));
                sh.atlas.draw_xf(gfx, f, &m, flip.unwrap_or(self.flip), [1.0, 1.0, 1.0, alpha]);
            }
            return;
        }
        let i = match self.state {
            State::Idle => 0,
            State::Losing => 1,
            State::Winning => 2,
        }
        .min(self.frames - 1);
        let src = [(i % self.cols) as f32 * self.frame, (i / self.cols) as f32 * self.frame, self.frame, self.frame];
        let (x0, x1) = if flip.unwrap_or(self.flip) { (x + w, x) } else { (x, x + w) };
        let m = spin(Affine::IDENTITY);
        let p = |px: f32, py: f32| m.apply([px, py]);
        gfx.quad(self.tex, src, [p(x0, y), p(x1, y), p(x1, y + h), p(x0, y + h)], [1.0, 1.0, 1.0, alpha]);
    }
}
