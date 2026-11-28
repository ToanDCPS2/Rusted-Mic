//! Animated world sprites: a port of the parts of `FlxSprite`, `FunkinSprite` and
//! `Bopper.hx` that stage props and characters rely on (prefix/indices/label
//! animations, per-animation offsets, `updateHitbox` placement, scroll factors,
//! dancing).

use std::collections::HashMap;

use funkin_core::camera::GameCamera;
use funkin_core::stage::AnimationData;

use crate::assets::Assets;
use crate::gfx::animate::{AnimRange, AnimateAtlas, Bounds, Inject};
use crate::gfx::atlas::Atlas;
use crate::gfx::{Affine, Color, Gfx, TexId, VIEW_H, VIEW_W};

enum Seq {
    Sparrow { atlas: usize, frames: Vec<usize> },
    /// `bounds`: the played timeline's bounds, which flixel-animate draws (and
    /// sizes the hitbox) relative to.
    Animate { atlas: usize, range: AnimRange, indices: Vec<u32>, bounds: crate::gfx::animate::Bounds },
}

impl Seq {
    fn len(&self) -> usize {
        match self {
            Seq::Sparrow { frames, .. } => frames.len(),
            Seq::Animate { range, indices, .. } => {
                if indices.is_empty() {
                    range.len as usize
                } else {
                    indices.len()
                }
            }
        }
    }
}

struct AnimDef {
    name: String,
    seq: Seq,
    fps: f32,
    looped: bool,
    offsets: [f32; 2],
    flip_x: bool,
}

/// What an actor draws.
pub enum Body {
    Animated,
    Image(TexId),
    /// Solid colour block of the given size.
    Solid(Color, f32, f32),
    /// `FlxBar`: an empty and a filled part, each a gradient.
    Bar(BarFill),
}

/// `FlxBar`'s look: `fill` covers `frac` of the bar from the `dir` side.
#[derive(Clone, Debug)]
pub struct BarFill {
    pub w: f32,
    pub h: f32,
    /// FlxBarFillDirection: LEFT_TO_RIGHT, RIGHT_TO_LEFT, TOP_TO_BOTTOM, BOTTOM_TO_TOP
    /// (the HORIZONTAL_/VERTICAL_INSIDE_OUT ones fill from the middle).
    pub dir: String,
    /// Gradient stops (one stop is a flat colour).
    pub empty: Vec<Color>,
    pub fill: Vec<Color>,
    /// Gradient angle in degrees: 90 runs top to bottom, 0 left to right.
    pub rotation: f32,
    pub frac: f32,
}

impl BarFill {
    pub fn new(w: f32, h: f32, dir: &str) -> Self {
        Self { w, h, dir: dir.to_owned(), empty: vec![[0.0, 0.0, 0.0, 1.0]], fill: vec![[1.0, 0.0, 0.0, 1.0]], rotation: 180.0, frac: 0.0 }
    }

    /// The filled rectangle (x0, y0, x1, y1) in the bar's own space.
    fn fill_rect(&self) -> [f32; 4] {
        let (w, h, f) = (self.w, self.h, self.frac.clamp(0.0, 1.0));
        match self.dir.as_str() {
            "RIGHT_TO_LEFT" => [w * (1.0 - f), 0.0, w, h],
            "TOP_TO_BOTTOM" => [0.0, 0.0, w, h * f],
            "BOTTOM_TO_TOP" => [0.0, h * (1.0 - f), w, h],
            "HORIZONTAL_INSIDE_OUT" => [w * (1.0 - f) / 2.0, 0.0, w * (1.0 + f) / 2.0, h],
            "VERTICAL_INSIDE_OUT" => [0.0, h * (1.0 - f) / 2.0, w, h * (1.0 + f) / 2.0],
            _ => [0.0, 0.0, w * f, h],
        }
    }
}

/// Fill `[x0, y0, x1, y1]` of a `w`x`h` box with `stops` running along
/// `rotation` (vertical when near 90/270), in strips so the gradient shows.
fn gradient_rect(gfx: &mut Gfx, m: &Affine, r: [f32; 4], w: f32, h: f32, stops: &[Color], rotation: f32, tint: Color) {
    if r[2] <= r[0] || r[3] <= r[1] || stops.is_empty() {
        return;
    }
    let white = gfx.white();
    let mul = |c: Color| [c[0] * tint[0], c[1] * tint[1], c[2] * tint[2], c[3] * tint[3]];
    let vertical = (rotation.rem_euclid(180.0) - 90.0).abs() < 45.0;
    let at = |t: f32| -> Color {
        if stops.len() == 1 {
            return stops[0];
        }
        let x = t.clamp(0.0, 1.0) * (stops.len() - 1) as f32;
        let i = (x.floor() as usize).min(stops.len() - 2);
        let k = x - i as f32;
        let (a, b) = (stops[i], stops[i + 1]);
        [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k, a[3] + (b[3] - a[3]) * k]
    };
    let strips = if stops.len() == 1 { 1 } else { 24 };
    let len = if vertical { h } else { w };
    for s in 0..strips {
        let (a, b) = (len * s as f32 / strips as f32, len * (s + 1) as f32 / strips as f32);
        let c = mul(at((a + b) / 2.0 / len.max(1e-3)));
        let q = if vertical { [r[0], r[1].max(a), r[2], r[3].min(b)] } else { [r[0].max(a), r[1], r[2].min(b), r[3]] };
        if q[2] <= q[0] || q[3] <= q[1] {
            continue;
        }
        let dst = [[q[0], q[1]], [q[2], q[1]], [q[2], q[3]], [q[0], q[3]]].map(|p| m.apply(p));
        gfx.quad(white, [0.0, 0.0, 1.0, 1.0], dst, c);
    }
}

pub struct Actor {
    pub name: String,
    body: Body,
    sparrows: Vec<Atlas>,
    animates: Vec<(AnimateAtlas, Bounds)>,
    anims: Vec<AnimDef>,
    by_name: HashMap<String, usize>,
    cur: Option<usize>,
    time: f32,
    /// Locked by `playAnimation(..., ignoreOther = true)` until it finishes.
    locked: bool,

    pub x: f32,
    pub y: f32,
    pub scale: [f32; 2],
    pub scroll: [f32; 2],
    pub flip_x: bool,
    pub alpha: f32,
    pub angle: f32,
    pub color: Color,
    /// `colorTransform` offsets (0..1), added after `color`.
    pub color_offset: [f32; 4],
    pub z_index: i32,
    pub global_offsets: [f32; 2],
    /// Flixel's `offset` once a script set it (otherwise the `updateHitbox`
    /// centring offset for the current scale).
    pub offset: Option<[f32; 2]>,
    /// `velocity` (px/s), applied each update.
    pub velocity: [f32; 2],
    anim_offsets: [f32; 2],
    /// Unscaled hitbox from the last `updateHitbox` (frame size at that time).
    hitbox: [f32; 2],

    pub dance_every: f64,
    has_danced: bool,
    /// Number of `dance` calls so far (lets scripts react to a bop).
    pub bops: u32,
    pub visible: bool,
    /// Raw main-timeline playback `(start, stop)` (`anim.play("", .., startFrame)`).
    raw: Option<(u32, u32)>,
    /// Pin the current animation to a frame (`curAnim.curFrame = n`).
    pub frame_override: Option<usize>,
    /// Props force-restart their idle on every bop; characters don't.
    pub should_bop: bool,
    /// Dances like a `BaseCharacter`: never over a sing or an unfinished special animation.
    pub char_dance: bool,
    /// `AtlasMenuItem.centered`: (x, y) is the current frame's midpoint.
    pub centered: bool,
    /// `sprite.shader`.
    pub shader: Option<std::rc::Rc<std::cell::RefCell<crate::shaders::ShaderInst>>>,
    /// Drawn as an `FlxTiledSprite`.
    pub tile: Option<Tile>,
    /// `frame.add(new FlxSpriteElement(sprite))` on the frames of symbols named
    /// with this keyword: (keyword, the prop drawn there).
    pub mouth: Vec<(String, usize)>,
    /// Drawn inside another sprite (`FlxSpriteElement.active = false`): its
    /// animation only moves when a script sets the frame.
    pub nested: bool,
    /// `FlxBackdrop`: the graphic repeats along these axes to fill the view.
    pub backdrop: [bool; 2],
    /// `FlxSkewedSprite.transformMatrix` once `matrixExposed`: replaces the rotation.
    pub transform: Option<Affine>,
    /// `blend` (OpenFL `BlendMode`).
    pub blend: crate::gfx::Blend,
    /// Animations the engine played, for a character script that overrides
    /// `playAnimation` (None = not recorded).
    pub anim_log: Option<Vec<(String, bool, bool)>>,
    /// `Bopper.idleSuffix`: dance into `idle<suffix>` / `danceLeft<suffix>`.
    pub idle_suffix: String,
    /// The animation last reported finished (`onAnimationFinished` fires once).
    pub finish_seen: Option<usize>,
    /// `atlasSettings.applyStageMatrix`: an Animate atlas is drawn as placed in
    /// Animate (its stage instance's matrix) instead of from its bounds' corner.
    pub apply_stage_matrix: bool,
}

/// `FlxTiledSprite`: the image repeated over a `w`×`h` area, shifted by `scroll`.
#[derive(Debug, Clone, Copy)]
pub struct Tile {
    pub w: f32,
    pub h: f32,
    pub repeat: [bool; 2],
    /// `scrollX` / `scrollY` (pixels).
    pub scroll: [f32; 2],
}

/// Spans along one axis of a tiled sprite: (from, to) on the sprite and (from, to) in the texture (0..1).
fn tile_spans(size: f32, tex: f32, scroll: f32, repeat: bool) -> Vec<(f32, f32, f32, f32)> {
    let mut out = Vec::new();
    if tex <= 0.0 {
        return out;
    }
    if repeat {
        let u0 = -scroll / tex;
        let u1 = u0 + size / tex;
        let mut k = u0.floor();
        while k < u1 && out.len() < 256 {
            let (a, b) = (u0.max(k), u1.min(k + 1.0));
            if b > a {
                out.push(((a - u0) * tex, (b - u0) * tex, a - k, b - k));
            }
            k += 1.0;
        }
    } else {
        let (p0, p1) = (scroll.clamp(0.0, size), (scroll + tex).clamp(0.0, size));
        if p1 > p0 {
            out.push((p0, p1, (p0 - scroll) / tex, (p1 - scroll) / tex));
        }
    }
    out
}

/// What an [`Actor`] shows at one moment: its animation, time in it and offsets.
#[derive(Debug, Clone, Copy)]
pub struct Pose {
    cur: Option<usize>,
    time: f32,
    anim_offsets: [f32; 2],
    frame_override: Option<usize>,
}

impl Actor {
    fn empty(name: &str, body: Body) -> Self {
        Self {
            name: name.to_owned(),
            body,
            sparrows: Vec::new(),
            animates: Vec::new(),
            anims: Vec::new(),
            by_name: HashMap::new(),
            cur: None,
            time: 0.0,
            locked: false,
            x: 0.0,
            y: 0.0,
            scale: [1.0, 1.0],
            scroll: [1.0, 1.0],
            flip_x: false,
            alpha: 1.0,
            angle: 0.0,
            color: [1.0; 4],
            color_offset: [0.0; 4],
            z_index: 0,
            global_offsets: [0.0; 2],
            offset: None,
            velocity: [0.0; 2],
            anim_offsets: [0.0; 2],
            hitbox: [1.0, 1.0],
            dance_every: 0.0,
            has_danced: false,
            bops: 0,
            visible: true,
            raw: None,
            frame_override: None,
            should_bop: true,
            centered: false,
            shader: None,
            tile: None,
            mouth: Vec::new(),
            nested: false,
            backdrop: [false; 2],
            char_dance: false,
            transform: None,
            blend: crate::gfx::Blend::Normal,
            anim_log: None,
            idle_suffix: String::new(),
            finish_seen: None,
            apply_stage_matrix: false,
        }
    }

    pub fn solid(name: &str, color: Color, w: f32, h: f32) -> Self {
        let mut a = Self::empty(name, Body::Solid(color, w, h));
        a.hitbox = [w, h];
        a
    }

    /// A sprite showing texture `tex` (for example a video's picture).
    pub fn from_texture(name: &str, tex: TexId, w: f32, h: f32) -> Self {
        let mut a = Self::empty(name, Body::Image(tex));
        a.hitbox = [w, h];
        a
    }

    pub fn image(gfx: &mut Gfx, assets: &Assets, name: &str, spec: &str, level: Option<&str>, smooth: bool) -> Result<Self, String> {
        let tex = gfx.load_texture_filtered(&assets.level_image(spec, level, "png"), smooth)?;
        let (w, h) = gfx.texture_size(tex);
        let mut a = Self::empty(name, Body::Image(tex));
        a.hitbox = [w, h];
        Ok(a)
    }

    /// Build an animated actor. `kind` is `sparrow`, `multisparrow`, `animateatlas`
    /// or `multianimateatlas`; per-animation `assetPath`s load extra atlases.
    #[allow(clippy::too_many_arguments)]
    pub fn animated(
        gfx: &mut Gfx,
        assets: &Assets,
        name: &str,
        kind: &str,
        spec: &str,
        level: Option<&str>,
        anims: &[AnimationData],
        smooth: bool,
    ) -> Result<Self, String> {
        let animate = kind.contains("animate");
        let mut a = Self::empty(name, Body::Animated);
        let mut sources: HashMap<String, usize> = HashMap::new();
        let mut load = |a: &mut Actor, spec: &str| -> Result<usize, String> {
            if let Some(&i) = sources.get(spec) {
                return Ok(i);
            }
            let i = if animate {
                let dir = assets.level_image(spec, level, "");
                let atlas = AnimateAtlas::load(gfx, &dir, smooth)?;
                let b = atlas.bounds(atlas.whole().tl);
                a.animates.push((atlas, b));
                a.animates.len() - 1
            } else {
                let base = assets.level_image(spec, level, "png").with_extension("");
                a.sparrows.push(Atlas::load_filtered(gfx, &base, smooth)?);
                a.sparrows.len() - 1
            };
            sources.insert(spec.to_owned(), i);
            Ok(i)
        };
        let main = load(&mut a, spec)?;
        for d in anims {
            let src = match &d.asset_path {
                Some(p) if kind.starts_with("multi") => match load(&mut a, p) {
                    Ok(i) => i,
                    Err(e) => {
                        log::debug!("{name}: {e}");
                        continue;
                    }
                },
                _ => main,
            };
            let seq = if animate {
                let atlas = &a.animates[src].0;
                let symbol = d.anim_type.as_deref() == Some("symbol");
                // `addBySymbol` plays the symbol's own timeline (no main-timeline
                // instance matrix); `addByFrameLabel` a span of the main timeline.
                let range = if symbol {
                    atlas.symbol(&d.prefix)
                } else {
                    atlas.label(&d.prefix).or_else(|| atlas.anim(&d.prefix)).or_else(|| atlas.anim(&d.name))
                };
                let Some(range) = range else {
                    log::debug!("{name}: no animate label/symbol {:?}", d.prefix);
                    continue;
                };
                // `addByFrameLabelIndices` skips indices past the label's end (dad's
                // idle-hold lists frame 12 of 12); symbol indices wrap (`frame % numFrames`).
                let indices: Vec<u32> = if symbol {
                    d.frame_indices.iter().filter(|&&i| i >= 0).map(|&i| i as u32 % range.len.max(1)).collect()
                } else {
                    d.frame_indices.iter().filter(|&&i| i >= 0 && (i as u32) < range.len).map(|&i| i as u32).collect()
                };
                if !d.frame_indices.is_empty() && indices.is_empty() {
                    log::debug!("{name}: no frames in range for {:?}", d.name);
                    continue;
                }
                let bounds = if matches!(range.tl, crate::gfx::animate::TimelineRef::Main) { a.animates[src].1 } else { atlas.bounds(range.tl) };
                Seq::Animate { atlas: src, range, indices, bounds }
            } else {
                let atlas = &a.sparrows[src];
                let frames = if d.frame_indices.is_empty() {
                    atlas.prefix(&d.prefix).frames
                } else {
                    atlas.indices(&d.prefix, &d.frame_indices).frames
                };
                if frames.is_empty() {
                    log::debug!("{name}: no frames for prefix {:?}", d.prefix);
                    continue;
                }
                Seq::Sparrow { atlas: src, frames }
            };
            a.by_name.insert(d.name.clone(), a.anims.len());
            a.anims.push(AnimDef {
                name: d.name.clone(),
                seq,
                fps: d.frame_rate as f32,
                looped: d.looped,
                offsets: [d.offsets[0] as f32, d.offsets[1] as f32],
                flip_x: d.flip_x,
            });
        }
        // `updateHitbox` straight after loading uses the atlas' first frame.
        a.hitbox = if animate {
            let b = a.animates[main].1;
            [b[2] - b[0], b[3] - b[1]]
        } else {
            let f = &a.sparrows[main].frames[0];
            [f.frame_w, f.frame_h]
        };
        Ok(a)
    }

    /// `animation.addByPrefix` / `addByIndices` at runtime (sparrow sprites).
    pub fn add_sparrow_anim(&mut self, name: &str, prefix: &str, indices: &[i32], fps: f32, looped: bool, flip_x: bool) -> bool {
        let Some(atlas) = self.sparrows.first() else { return false };
        let frames = if indices.is_empty() { atlas.prefix(prefix).frames } else { atlas.indices(prefix, indices).frames };
        if frames.is_empty() {
            return false;
        }
        if let Some(&i) = self.by_name.get(name) {
            self.anims[i].seq = Seq::Sparrow { atlas: 0, frames };
            return true;
        }
        self.by_name.insert(name.to_owned(), self.anims.len());
        self.anims.push(AnimDef { name: name.to_owned(), seq: Seq::Sparrow { atlas: 0, frames }, fps, looped, offsets: [0.0; 2], flip_x });
        true
    }

    /// `loadGraphic`, `frames = ...`, `makeSolidColor`: take `o`'s graphics
    /// and animations, keeping this sprite's placement.
    /// An `FlxBar` (`w`x`h`, filling from `dir`).
    pub fn bar(name: &str, w: f32, h: f32, dir: &str) -> Self {
        let mut a = Self::empty(name, Body::Bar(BarFill::new(w, h, dir)));
        a.hitbox = [w, h];
        a
    }

    /// The bar look of an `FlxBar` actor.
    pub fn bar_mut(&mut self) -> Option<&mut BarFill> {
        match &mut self.body {
            Body::Bar(b) => Some(b),
            _ => None,
        }
    }

    pub fn take_graphics(&mut self, o: Actor) {
        self.body = o.body;
        self.sparrows = o.sparrows;
        self.animates = o.animates;
        self.anims = o.anims;
        self.by_name = o.by_name;
        self.cur = None;
        self.time = 0.0;
        self.locked = false;
        self.raw = None;
        self.frame_override = None;
        self.hitbox = o.hitbox;
    }

    /// Per-animation offsets (`addOffset` / `setAnimationOffsets`).
    pub fn set_anim_offsets(&mut self, name: &str, x: f32, y: f32) {
        if let Some(&i) = self.by_name.get(name) {
            self.anims[i].offsets = [x, y];
        }
    }

    pub fn has_anim(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// The current animation's frame rate (`curAnim.frameRate`).
    pub fn frame_rate(&self) -> f32 {
        self.cur.map(|i| self.anims[i].fps).unwrap_or(0.0)
    }

    /// The current animation's frame number (`curAnim.curFrame`).
    pub fn frame_number(&self) -> usize {
        match self.cur {
            None => 0,
            Some(i) => {
                let d = &self.anims[i];
                let n = d.seq.len().max(1);
                let f = (self.time.max(0.0) * d.fps) as usize;
                if d.looped { f % n } else { f.min(n - 1) }
            }
        }
    }

    /// `animation.addBySymbol(name, symbol, fps, looped)`; an unknown symbol
    /// (the atlas' own, `getDefaultSymbol()`) is the main timeline.
    pub fn add_symbol_anim(&mut self, name: &str, symbol: &str, fps: f32, looped: bool) {
        let Some((atlas, whole)) = self.animates.first() else { return };
        let range = atlas.symbol(symbol).unwrap_or_else(|| atlas.whole());
        let bounds = if matches!(range.tl, crate::gfx::animate::TimelineRef::Main) { *whole } else { atlas.bounds(range.tl) };
        let def = AnimDef { name: name.to_owned(), seq: Seq::Animate { atlas: 0, range, indices: vec![], bounds }, fps, looped, offsets: [0.0; 2], flip_x: false };
        match self.by_name.get(name) {
            Some(&i) => self.anims[i] = def,
            None => {
                self.by_name.insert(name.to_owned(), self.anims.len());
                self.anims.push(def);
            }
        }
    }

    /// `FlxAnimationUtil.addTextureAtlasAnimations`: character-data animations on
    /// this sprite's texture atlas (frame labels, or symbols for `animType: symbol`).
    pub fn add_atlas_anims(&mut self, anims: &[funkin_core::stage::AnimationData]) {
        let Some((atlas, whole)) = self.animates.first() else { return };
        let whole = *whole;
        let mut defs = Vec::new();
        for d in anims {
            let symbol = d.anim_type.as_deref() == Some("symbol");
            let range = if symbol { atlas.symbol(&d.prefix) } else { atlas.label(&d.prefix).or_else(|| atlas.anim(&d.prefix)).or_else(|| atlas.anim(&d.name)) };
            // The atlas' own symbol name is its whole main timeline.
            let range = range.or_else(|| (!atlas.main_name.is_empty() && atlas.main_name == d.prefix).then(|| atlas.whole()));
            let Some(range) = range else { continue };
            let indices: Vec<u32> = if symbol {
                d.frame_indices.iter().filter(|&&i| i >= 0).map(|&i| i as u32 % range.len.max(1)).collect()
            } else {
                d.frame_indices.iter().filter(|&&i| i >= 0 && (i as u32) < range.len).map(|&i| i as u32).collect()
            };
            let bounds = if matches!(range.tl, crate::gfx::animate::TimelineRef::Main) { whole } else { atlas.bounds(range.tl) };
            defs.push(AnimDef {
                name: d.name.clone(),
                seq: Seq::Animate { atlas: 0, range, indices, bounds },
                fps: d.frame_rate as f32,
                looped: d.looped,
                offsets: [0.0; 2],
                flip_x: d.flip_x,
            });
        }
        for def in defs {
            match self.by_name.get(&def.name) {
                Some(&i) => self.anims[i] = def,
                None => {
                    self.by_name.insert(def.name.clone(), self.anims.len());
                    self.anims.push(def);
                }
            }
        }
    }

    /// flixel-animate `anim.play("")`: the atlas' default animation, the whole
    /// main timeline under its symbol name. Returns that name.
    pub fn play_default(&mut self, restart: bool) -> String {
        let Some((atlas, whole)) = self.animates.first() else { return String::new() };
        let name = if atlas.main_name.is_empty() { "__main".to_owned() } else { atlas.main_name.clone() };
        if !self.by_name.contains_key(&name) {
            let def = AnimDef { name: name.clone(), seq: Seq::Animate { atlas: 0, range: atlas.whole(), indices: vec![], bounds: *whole }, fps: atlas.fps, looped: false, offsets: [0.0; 2], flip_x: false };
            self.by_name.insert(name.clone(), self.anims.len());
            self.anims.push(def);
        }
        let i = self.by_name[&name];
        if restart || self.cur != Some(i) || self.finished() {
            self.time = 0.0;
        }
        self.raw = None;
        self.cur = Some(i);
        self.anim_offsets = [0.0; 2];
        name
    }

    /// The atlas frame label starting on the current frame (`onFrameLabel`).
    pub fn frame_label(&self) -> Option<String> {
        let i = self.cur?;
        let d = &self.anims[i];
        let Seq::Animate { atlas, range, indices, .. } = &d.seq else { return None };
        if !matches!(range.tl, crate::gfx::animate::TimelineRef::Main) {
            return None;
        }
        let f = self.frame_number();
        let f = if indices.is_empty() { range.start + f as u32 } else { *indices.get(f)? };
        self.animates.get(*atlas)?.0.label_starting_at(f).map(str::to_owned)
    }

    /// The first Animate atlas, for code-driven edits (`scaleElement`).
    /// Hide symbol art in every atlas (see [`AnimateAtlas::hide_symbols`]).
    pub fn hide_symbols(&mut self, name: &str, exact: bool) {
        for (a, _) in &mut self.animates {
            a.hide_symbols(name, exact);
        }
    }

    pub fn atlas_mut(&mut self) -> Option<&mut crate::gfx::animate::AnimateAtlas> {
        self.animates.first_mut().map(|(a, _)| a)
    }

    /// `curAnim.looped = on`.
    pub fn set_looped(&mut self, on: bool) {
        if let Some(i) = self.cur {
            self.anims[i].looped = on;
        }
    }

    /// Loaded from an Animate texture atlas.
    pub fn is_animate(&self) -> bool {
        !self.animates.is_empty()
    }

    /// `curAnim.curFrame = f`.
    pub fn set_frame_number(&mut self, f: usize) {
        if let Some(i) = self.cur {
            let fps = self.anims[i].fps;
            if fps > 0.0 {
                self.time = (f as f32 + 0.01) / fps;
            }
        }
    }

    /// The current animation's frame count (`curAnim.numFrames`).
    pub fn frame_count(&self) -> usize {
        self.cur.map(|i| self.anims[i].seq.len()).unwrap_or(0)
    }

    pub fn current(&self) -> &str {
        self.cur.map(|i| self.anims[i].name.as_str()).unwrap_or("")
    }

    /// The current animation just finished (once per play): its name, for
    /// `onAnimationFinished`.
    pub fn take_finished(&mut self) -> Option<String> {
        let i = self.cur?;
        if !self.finished() {
            if self.finish_seen == Some(i) {
                self.finish_seen = None;
            }
            return None;
        }
        if self.finish_seen == Some(i) {
            return None;
        }
        self.finish_seen = Some(i);
        Some(self.anims[i].name.clone())
    }

    pub fn finished(&self) -> bool {
        match self.cur {
            None => true,
            Some(i) => {
                let d = &self.anims[i];
                !d.looped && self.time * d.fps >= d.seq.len() as f32
            }
        }
    }

    /// `Bopper.correctAnimationName`: strip `-suffix`es until an animation exists.
    fn correct_name<'a>(&self, mut name: &'a str) -> Option<&'a str> {
        loop {
            if self.has_anim(name) {
                return Some(name);
            }
            name = &name[..name.rfind('-')?];
        }
    }

    /// A texture atlas' frame label (or symbol) played by name registers itself.
    fn auto_label(&mut self, name: &str) {
        if self.by_name.contains_key(name) || self.animates.is_empty() {
            return;
        }
        let (atlas, whole) = &self.animates[0];
        let Some(range) = atlas.label(name).or_else(|| atlas.symbol(name)) else { return };
        let bounds = if matches!(range.tl, crate::gfx::animate::TimelineRef::Main) { *whole } else { atlas.bounds(range.tl) };
        let fps = atlas.fps;
        self.by_name.insert(name.to_owned(), self.anims.len());
        self.anims.push(AnimDef { name: name.to_owned(), seq: Seq::Animate { atlas: 0, range, indices: vec![], bounds }, fps, looped: false, offsets: [0.0; 2], flip_x: false });
    }

    /// `Bopper.playAnimation`.
    pub fn play(&mut self, name: &str, restart: bool, ignore_other: bool) {
        if let Some(l) = &mut self.anim_log {
            l.push((name.to_owned(), restart, ignore_other));
        }
        if self.locked && !ignore_other {
            if self.finished() {
                self.locked = false;
            } else {
                return;
            }
        }
        self.auto_label(name);
        let Some(name) = self.correct_name(name) else { return };
        let i = self.by_name[name];
        // FlxAnimationController.play: keep going if already playing and unfinished.
        if restart || self.cur != Some(i) || self.finished() {
            self.time = 0.0;
        }
        self.raw = None;
        if self.time == 0.0 {
            self.finish_seen = None;
        }
        self.cur = Some(i);
        self.locked = ignore_other;
        self.anim_offsets = self.anims[i].offsets;
    }

    /// `Bopper.dance`.
    /// `AtlasSpriteSettings.swfMode` (characters default to on).
    pub fn set_swf_mode(&mut self, on: bool) {
        for (a, _) in &mut self.animates {
            a.swf_mode = on;
        }
    }

    /// Play the main timeline of an Animate actor from `start`, stopping on `stop`
    /// (clamped to the timeline length).
    pub fn play_timeline(&mut self, start: u32, stop: Option<u32>) {
        let len = self.animates.first().map(|(a, _)| a.whole().len).unwrap_or(1);
        self.cur = None;
        self.raw = Some((start.min(len - 1), stop.unwrap_or(len - 1).min(len - 1)));
        self.time = 0.0;
    }

    fn raw_frame(&self) -> u32 {
        match (self.raw, self.animates.first()) {
            (Some((start, stop)), Some((a, _))) => (start + (self.time.max(0.0) * a.fps) as u32).min(stop.max(start)),
            _ => 0,
        }
    }

    pub fn bop(&mut self, force: bool) {
        self.bops += 1;
        let sfx = std::mem::take(&mut self.idle_suffix);
        if self.has_anim("danceLeft") {
            let n = if self.has_danced { "danceRight" } else { "danceLeft" };
            self.play(&format!("{n}{sfx}"), force, false);
            self.has_danced = !self.has_danced;
        } else {
            self.play(&format!("idle{sfx}"), force, false);
        }
        self.idle_suffix = sfx;
    }

    /// `Bopper.onStepHit`.
    pub fn on_step(&mut self, step: i64) {
        if self.dance_every > 0.0 && (step as f64).rem_euclid(self.dance_every * 4.0) == 0.0 {
            if self.char_dance {
                let c = self.current();
                let singing = c.starts_with("sing") && !c.ends_with("-end");
                if singing || (!c.starts_with("dance") && !c.starts_with("idle") && !self.finished()) {
                    return;
                }
            }
            self.bop(self.should_bop);
        }
    }

    pub fn update(&mut self, elapsed: f32) {
        if !self.nested {
            self.time += elapsed;
        }
        self.x += self.velocity[0] * elapsed;
        self.y += self.velocity[1] * elapsed;
        if self.locked && self.finished() {
            self.locked = false;
        }
    }

    /// Re-run `updateHitbox` against the current frame (used after a character's
    /// first dance, which is what positions it on the stage).
    pub fn update_hitbox(&mut self) {
        // `updateHitbox` recomputes `offset` from the new size.
        self.offset = None;
        if let Some(i) = self.cur {
            match &self.anims[i].seq {
                Seq::Sparrow { atlas, frames } => {
                    let f = &self.sparrows[*atlas].frames[frames[0]];
                    self.hitbox = [f.frame_w, f.frame_h];
                }
                Seq::Animate { bounds: b, .. } => {
                    self.hitbox = [b[2] - b[0], b[3] - b[1]];
                }
            }
        }
    }

    /// Scaled size (`width`/`height` after `updateHitbox`).
    /// The texture of a plain image sprite (`sprite.pixels`).
    pub fn image_texture(&self) -> Option<TexId> {
        match self.body {
            Body::Image(t) => Some(t),
            _ => None,
        }
    }

    pub fn size(&self) -> [f32; 2] {
        if let Some(t) = &self.tile {
            return [t.w * self.scale[0].abs(), t.h * self.scale[1].abs()];
        }
        [self.hitbox[0] * self.scale[0].abs(), self.hitbox[1] * self.scale[1].abs()]
    }

    fn frame_index(&self, d: &AnimDef) -> usize {
        self.frame_index_at(d, &self.pose())
    }

    fn frame_index_at(&self, d: &AnimDef, pose: &Pose) -> usize {
        let len = d.seq.len().max(1);
        if let Some(i) = pose.frame_override {
            return i.min(len - 1);
        }
        let i = (pose.time.max(0.0) * d.fps) as usize;
        if d.looped {
            i % len
        } else {
            i.min(len - 1)
        }
    }

    /// `sprite.offset`: subtracted from the drawn position, unscaled.
    pub fn flx_offset(&self) -> [f32; 2] {
        let [hw, hh] = self.hitbox;
        let [sx, sy] = self.scale;
        self.offset.unwrap_or([-0.5 * (hw * sx - hw), -0.5 * (hh * sy - hh)])
    }

    /// Local frame space → canvas, per Flixel's `drawComplex` with `updateHitbox`
    /// origin/offset, Bopper's animation offsets, and the camera.
    fn matrix(&self, cam: &GameCamera, pose: &Pose) -> Affine {
        let [hw, hh] = self.hitbox;
        let [sx, sy] = self.scale;
        let origin = [hw / 2.0, hh / 2.0];
        let offset = self.flx_offset();
        let [cx, cy] = if self.centered { self.frame_size().map(|v| v / 2.0) } else { [0.0, 0.0] };
        let px = -cx + self.x - cam.scroll[0] as f32 * self.scroll[0] - offset[0] - (pose.anim_offsets[0] - self.global_offsets[0]) * sx;
        let py = -cy + self.y - cam.scroll[1] as f32 * self.scroll[1] - offset[1] - (pose.anim_offsets[1] - self.global_offsets[1]) * sy;
        let local = Affine::translate(px + origin[0], py + origin[1])
            .then(&self.transform.unwrap_or_else(|| Affine::rotate_deg(self.angle)))
            .then(&Affine::scale(sx, sy))
            .then(&Affine::translate(-origin[0], -origin[1]));
        Affine::zoom_about(cam.zoom() as f32, VIEW_W / 2.0, VIEW_H / 2.0).then(&local)
    }

    /// Untrimmed size of the frame being shown (unscaled).
    pub fn frame_size(&self) -> [f32; 2] {
        if let Some(i) = self.cur {
            let d = &self.anims[i];
            if let Seq::Sparrow { atlas, frames } = &d.seq {
                let f = &self.sparrows[*atlas].frames[frames[self.frame_index(d)]];
                return [f.frame_w, f.frame_h];
            }
        }
        self.hitbox
    }

    /// The animation state that decides what is drawn (for trails).
    pub fn pose(&self) -> Pose {
        Pose { cur: self.cur, time: self.time, anim_offsets: self.anim_offsets, frame_override: self.frame_override }
    }

    /// Draw with an extra alpha multiplier.
    pub fn draw_alpha(&self, gfx: &mut Gfx, cam: &GameCamera, alpha: f32) {
        self.draw_inner(gfx, cam, self.alpha * alpha, &self.pose(), &[]);
    }

    pub fn draw(&self, gfx: &mut Gfx, cam: &GameCamera) {
        self.draw_inner(gfx, cam, self.alpha, &self.pose(), &[]);
    }

    /// [`Self::draw`] with a sprite injected into some of its atlas symbols.
    pub fn draw_with(&self, gfx: &mut Gfx, cam: &GameCamera, inject: &[(&str, Inject)]) {
        self.draw_inner(gfx, cam, self.alpha, &self.pose(), inject);
    }

    /// The sprite's own rotation, scale and flip (no position): what
    /// flixel-animate's `prepareAnimateMatrix` gives an element's host.
    pub fn linear_matrix(&self) -> Affine {
        let mut r = self.transform.unwrap_or_else(|| Affine::rotate_deg(self.angle));
        r.0[4] = 0.0;
        r.0[5] = 0.0;
        let m = r.then(&Affine::scale(self.scale[0], self.scale[1]));
        if self.flip_x { m.then(&Affine::scale(-1.0, 1.0)) } else { m }
    }

    /// Draw as a `FlxSpriteElement` inside another sprite's atlas, as
    /// flixel-animate places it (`applyObjectTransform`): `parent` is the
    /// symbol's matrix on screen (after a camera zoom of `zoom`) and `host` the
    /// atlas sprite's own [`Self::linear_matrix`]. The sprite sits at the
    /// symbol's origin and turns with the symbol's x axis, but keeps its own
    /// scale and is never mirrored by the symbol (a mirrored symbol reads as a
    /// half turn, which scripts' angles allow for).
    pub fn draw_nested(&self, gfx: &mut Gfx, parent: &Affine, alpha: f32, zoom: f32, host: &Affine) {
        if !self.visible {
            return;
        }
        let pose = self.pose();
        let [hw, hh] = self.hitbox;
        let origin = [hw / 2.0, hh / 2.0];
        let off = self.offset.unwrap_or([0.0, 0.0]);
        let [pa, pb, _, _, ptx, pty] = parent.0;
        let [ma, mb, mc, md, _, _] = host.0;
        let det = ma * md - mb * mc;
        let flip = if det < 0.0 { -1.0 } else { 1.0 };
        let host_scale = det.abs().sqrt();
        // The pivot, through the host's own transform.
        let p = [origin[0] - off[0], origin[1] - off[1]];
        let moved = [ma * p[0] + mc * p[1] - p[0], mb * p[0] + md * p[1] - p[1]];
        let sym_x = (md * pa - mc * pb) * flip;
        let sym_y = (ma * pb - mb * pa) * flip;
        let angle = mb.atan2(ma).to_degrees() + flip * (self.angle + sym_y.atan2(sym_x).to_degrees());
        let [sx, sy] = [self.scale[0] * host_scale, self.scale[1] * host_scale * flip];
        // FlxSprite.draw at that position, then the camera's zoom.
        let m = Affine::translate(ptx + zoom * (moved[0] - off[0] + origin[0]), pty + zoom * (moved[1] - off[1] + origin[1]))
            .then(&Affine::scale(zoom, zoom))
            .then(&Affine::rotate_deg(angle))
            .then(&Affine::scale(sx, sy))
            .then(&Affine::translate(-origin[0], -origin[1]));
        self.draw_body_m(gfx, m, self.alpha * alpha, &pose, &[]);
    }

    /// Draw as it looked at `pose`, with `alpha` (an `FlxTrail` sprite).
    pub fn draw_pose(&self, gfx: &mut Gfx, cam: &GameCamera, pose: &Pose, alpha: f32) {
        self.draw_inner(gfx, cam, alpha, pose, &[]);
    }

    fn draw_inner(&self, gfx: &mut Gfx, cam: &GameCamera, alpha: f32, pose: &Pose, inject: &[(&str, Inject)]) {
        if let Some(s) = &self.shader {
            let mut s = s.borrow_mut();
            if let Some(id) = s.program(gfx) {
                let auto = crate::gfx::shader::AutoFrame { on: s.auto_frame, drop_angle: s.drop_angle() };
                gfx.set_shader(Some((id, &s.params, auto)));
            }
        }
        gfx.set_blend(self.blend);
        self.draw_body(gfx, cam, alpha, pose, inject);
        gfx.set_blend(crate::gfx::Blend::Normal);
        if self.shader.is_some() {
            gfx.set_shader(None);
        }
    }

    fn draw_body(&self, gfx: &mut Gfx, cam: &GameCamera, alpha: f32, pose: &Pose, inject: &[(&str, Inject)]) {
        if alpha <= 0.0 || !self.visible {
            return;
        }
        let m = self.matrix(cam, pose);
        self.draw_body_m(gfx, m, alpha, pose, inject);
    }

    fn draw_body_m(&self, gfx: &mut Gfx, m: Affine, alpha: f32, pose: &Pose, inject: &[(&str, Inject)]) {
        if alpha <= 0.0 {
            return;
        }
        if self.color_offset != [0.0; 4] {
            let prev = gfx.color_offset;
            gfx.color_offset = self.color_offset;
            self.draw_body_inner(gfx, m, alpha, pose, inject);
            gfx.color_offset = prev;
        } else {
            self.draw_body_inner(gfx, m, alpha, pose, inject);
        }
    }

    fn draw_body_inner(&self, gfx: &mut Gfx, m: Affine, alpha: f32, pose: &Pose, inject: &[(&str, Inject)]) {
        let color = [self.color[0], self.color[1], self.color[2], self.color[3] * alpha];
        match &self.body {
            Body::Bar(b) => {
                gradient_rect(gfx, &m, [0.0, 0.0, b.w, b.h], b.w, b.h, &b.empty, b.rotation, color);
                gradient_rect(gfx, &m, b.fill_rect(), b.w, b.h, &b.fill, b.rotation, color);
            }
            Body::Solid(c, w, h) => {
                let dst = [[0.0, 0.0], [*w, 0.0], [*w, *h], [0.0, *h]].map(|p| m.apply(p));
                let white = gfx.white();
                gfx.quad(white, [0.0, 0.0, 1.0, 1.0], dst, [c[0] * color[0], c[1] * color[1], c[2] * color[2], c[3] * color[3]]);
            }
            Body::Image(t) if self.tile.is_some() => {
                let tile = self.tile.unwrap();
                let (w, h) = gfx.texture_size(*t);
                for (y0, y1, v0, v1) in tile_spans(tile.h, h, tile.scroll[1], tile.repeat[1]) {
                    for (x0, x1, u0, u1) in tile_spans(tile.w, w, tile.scroll[0], tile.repeat[0]) {
                        let dst = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]].map(|p| m.apply(p));
                        gfx.quad_uv(*t, [[u0, v0], [u1, v0], [u1, v1], [u0, v1]], dst, color);
                    }
                }
            }
            Body::Image(t) if self.backdrop != [false; 2] => {
                let (w, h) = gfx.texture_size(*t);
                // Copies k*w apart, enough to cover the view.
                let range = |origin: f32, step: f32, view: f32, on: bool| -> (i32, i32) {
                    if !on || step.abs() < 1e-3 {
                        return (0, 0);
                    }
                    let (a, b) = ((0.0 - origin) / step, (view - origin) / step);
                    let (lo, hi) = (a.min(b).floor() as i32 - 1, a.max(b).ceil() as i32 + 1);
                    (lo.max(-64), hi.min(64))
                };
                let o = m.apply([0.0, 0.0]);
                let (kx0, kx1) = range(o[0], m.0[0] * w, VIEW_W, self.backdrop[0]);
                let (ky0, ky1) = range(o[1], m.0[3] * h, VIEW_H, self.backdrop[1]);
                for ky in ky0..=ky1 {
                    for kx in kx0..=kx1 {
                        let (x0, y0) = (kx as f32 * w, ky as f32 * h);
                        let corners = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]];
                        gfx.quad(*t, [0.0, 0.0, w, h], corners.map(|p| m.apply(p)), color);
                    }
                }
            }
            Body::Image(t) => {
                let (w, h) = gfx.texture_size(*t);
                let mut corners = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]];
                if self.flip_x {
                    for c in &mut corners {
                        c[0] = w - c[0];
                    }
                }
                gfx.quad(*t, [0.0, 0.0, w, h], corners.map(|p| m.apply(p)), color);
            }
            Body::Animated => {
                let Some(i) = pose.cur else {
                    // No animation played yet: Flixel shows the atlas' first frame.
                    if let Some(a) = self.sparrows.first() {
                        a.draw_xf(gfx, 0, &m, self.flip_x, color);
                    } else if let Some((a, b)) = self.animates.first() {
                        let pre = if self.apply_stage_matrix { a.stage } else { Affine::translate(-b[0], -b[1]) };
                        a.draw(gfx, a.whole().tl, self.raw_frame(), &m.then(&pre), color);
                    }
                    return;
                };
                let d = &self.anims[i];
                let fi = self.frame_index_at(d, pose);
                let flip = self.flip_x ^ d.flip_x;
                match &d.seq {
                    Seq::Sparrow { atlas, frames } => self.sparrows[*atlas].draw_xf(gfx, frames[fi], &m, flip, color),
                    Seq::Animate { atlas, range, indices, bounds: b } => {
                        let (atlas, _) = &self.animates[*atlas];
                        let f = range.start + if indices.is_empty() { fi as u32 } else { indices[fi] };
                        // flixel-animate draws relative to the timeline bounds.
                        let pre = if self.apply_stage_matrix {
                            atlas.stage
                        } else if flip {
                            Affine([-1.0, 0.0, 0.0, 1.0, b[2], -b[1]])
                        } else {
                            Affine::translate(-b[0], -b[1])
                        };
                        if inject.is_empty() {
                            atlas.draw(gfx, range.tl, f, &m.then(&pre), color);
                        } else {
                            let list: Vec<(Vec<usize>, Inject)> = inject.iter().map(|(kw, f_in)| (atlas.symbols_with(kw), *f_in)).collect();
                            let opts = crate::gfx::animate::DrawOpts { inject: &list, ..Default::default() };
                            atlas.draw_opts(gfx, range.tl, f, &m.then(&pre), color, opts);
                        }
                    }
                }
            }
        }
    }
}
