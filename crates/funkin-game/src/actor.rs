//! Animated world sprites: a port of the parts of `FlxSprite`, `FunkinSprite` and
//! `Bopper.hx` that stage props and characters rely on (prefix/indices/label
//! animations, per-animation offsets, `updateHitbox` placement, scroll factors,
//! dancing).

use std::collections::HashMap;

use funkin_core::camera::GameCamera;
use funkin_core::stage::AnimationData;

use crate::assets::Assets;
use crate::gfx::animate::{AnimRange, AnimateAtlas, Bounds};
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
    pub z_index: i32,
    pub global_offsets: [f32; 2],
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
    /// `AtlasMenuItem.centered`: (x, y) is the current frame's midpoint.
    pub centered: bool,
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
            z_index: 0,
            global_offsets: [0.0; 2],
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
        }
    }

    pub fn solid(name: &str, color: Color, w: f32, h: f32) -> Self {
        let mut a = Self::empty(name, Body::Solid(color, w, h));
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

    pub fn has_anim(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
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

    /// The current animation's frame count (`curAnim.numFrames`).
    pub fn frame_count(&self) -> usize {
        self.cur.map(|i| self.anims[i].seq.len()).unwrap_or(0)
    }

    pub fn current(&self) -> &str {
        self.cur.map(|i| self.anims[i].name.as_str()).unwrap_or("")
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

    /// `Bopper.playAnimation`.
    pub fn play(&mut self, name: &str, restart: bool, ignore_other: bool) {
        if self.locked && !ignore_other {
            if self.finished() {
                self.locked = false;
            } else {
                return;
            }
        }
        let Some(name) = self.correct_name(name) else { return };
        let i = self.by_name[name];
        // FlxAnimationController.play: keep going if already playing and unfinished.
        if restart || self.cur != Some(i) || self.finished() {
            self.time = 0.0;
        }
        self.raw = None;
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
        if self.has_anim("danceLeft") {
            let n = if self.has_danced { "danceRight" } else { "danceLeft" };
            self.play(n, force, false);
            self.has_danced = !self.has_danced;
        } else {
            self.play("idle", force, false);
        }
    }

    /// `Bopper.onStepHit`.
    pub fn on_step(&mut self, step: i64) {
        if self.dance_every > 0.0 && (step as f64).rem_euclid(self.dance_every * 4.0) == 0.0 {
            self.bop(self.should_bop);
        }
    }

    pub fn update(&mut self, elapsed: f32) {
        self.time += elapsed;
        if self.locked && self.finished() {
            self.locked = false;
        }
    }

    /// Re-run `updateHitbox` against the current frame (used after a character's
    /// first dance, which is what positions it on the stage).
    pub fn update_hitbox(&mut self) {
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
    pub fn size(&self) -> [f32; 2] {
        [self.hitbox[0] * self.scale[0].abs(), self.hitbox[1] * self.scale[1].abs()]
    }

    fn frame_index(&self, d: &AnimDef) -> usize {
        let len = d.seq.len().max(1);
        if let Some(i) = self.frame_override {
            return i.min(len - 1);
        }
        let i = (self.time.max(0.0) * d.fps) as usize;
        if d.looped {
            i % len
        } else {
            i.min(len - 1)
        }
    }

    /// Local frame space → canvas, per Flixel's `drawComplex` with `updateHitbox`
    /// origin/offset, Bopper's animation offsets, and the camera.
    fn matrix(&self, cam: &GameCamera) -> Affine {
        let [hw, hh] = self.hitbox;
        let [sx, sy] = self.scale;
        let origin = [hw / 2.0, hh / 2.0];
        let offset = [-0.5 * (hw * sx - hw), -0.5 * (hh * sy - hh)];
        let [cx, cy] = if self.centered { self.frame_size().map(|v| v / 2.0) } else { [0.0, 0.0] };
        let px = -cx + self.x - cam.scroll[0] as f32 * self.scroll[0] - offset[0] - (self.anim_offsets[0] - self.global_offsets[0]) * sx;
        let py = -cy + self.y - cam.scroll[1] as f32 * self.scroll[1] - offset[1] - (self.anim_offsets[1] - self.global_offsets[1]) * sy;
        let local = Affine::translate(px + origin[0], py + origin[1])
            .then(&Affine::rotate_deg(self.angle))
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

    /// Draw with an extra alpha multiplier.
    pub fn draw_alpha(&self, gfx: &mut Gfx, cam: &GameCamera, alpha: f32) {
        self.draw_inner(gfx, cam, self.alpha * alpha);
    }

    pub fn draw(&self, gfx: &mut Gfx, cam: &GameCamera) {
        self.draw_inner(gfx, cam, self.alpha);
    }

    fn draw_inner(&self, gfx: &mut Gfx, cam: &GameCamera, alpha: f32) {
        if alpha <= 0.0 || !self.visible {
            return;
        }
        let m = self.matrix(cam);
        let color = [self.color[0], self.color[1], self.color[2], self.color[3] * alpha];
        match &self.body {
            Body::Solid(c, w, h) => {
                let dst = [[0.0, 0.0], [*w, 0.0], [*w, *h], [0.0, *h]].map(|p| m.apply(p));
                let white = gfx.white();
                gfx.quad(white, [0.0, 0.0, 1.0, 1.0], dst, [c[0], c[1], c[2], c[3] * alpha]);
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
                let Some(i) = self.cur else {
                    // No animation played yet: Flixel shows the atlas' first frame.
                    if let Some(a) = self.sparrows.first() {
                        a.draw_xf(gfx, 0, &m, self.flip_x, color);
                    } else if let Some((a, b)) = self.animates.first() {
                        let pre = Affine::translate(-b[0], -b[1]);
                        a.draw(gfx, a.whole().tl, self.raw_frame(), &m.then(&pre), color);
                    }
                    return;
                };
                let d = &self.anims[i];
                let fi = self.frame_index(d);
                let flip = self.flip_x ^ d.flip_x;
                match &d.seq {
                    Seq::Sparrow { atlas, frames } => self.sparrows[*atlas].draw_xf(gfx, frames[fi], &m, flip, color),
                    Seq::Animate { atlas, range, indices, bounds: b } => {
                        let (atlas, _) = &self.animates[*atlas];
                        let f = range.start + if indices.is_empty() { fi as u32 } else { indices[fi] };
                        // flixel-animate draws relative to the timeline bounds.
                        let pre = if flip {
                            Affine([-1.0, 0.0, 0.0, 1.0, b[2], -b[1]])
                        } else {
                            Affine::translate(-b[0], -b[1])
                        };
                        atlas.draw(gfx, range.tl, f, &m.then(&pre), color);
                    }
                }
            }
        }
    }
}
