//! Adobe Animate texture atlases (`Animation.json` + `spritemapN.json/png`), the
//! format most Funkin' characters use (rendered in the original by `flixel-animate`).
//!
//! Both export flavours are supported: the compressed one (`AN`/`SD`/`MD`, `SI`/`ASI`,
//! `MX`/`M3D`) and the long-form one (`ANIMATION`/`SYMBOL_DICTIONARY`, `Matrix3D`).

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::atlas::{draw_region, Frame};
use super::{Affine, Color, Gfx, MaskOp, TexId};

#[derive(Debug, Clone, Copy)]
enum LoopMode {
    Loop,
    PlayOnce,
    Single,
}

#[derive(Debug)]
enum Element {
    Symbol {
        name: String,
        idx: Option<usize>,
        first: u32,
        mode: LoopMode,
        /// Movie clip: shows frame 0 unless the atlas is in `swfMode`.
        mc: bool,
        m: Affine,
        alpha: f32,
        /// Movie clip filters' bounds growth `[left, top, right, bottom]` (flixel-animate's `expandFilterBounds`).
        ext: [f32; 4],
    },
    Sprite {
        name: String,
        idx: Option<usize>,
        m: Affine,
    },
}

#[derive(Debug)]
struct KeyFrame {
    index: u32,
    duration: u32,
    label: Option<String>,
    elements: Vec<Element>,
}

#[derive(Debug, Default)]
struct Layer {
    name: String,
    /// A mask layer ("Clp"): not drawn itself, only clips others.
    is_mask: bool,
    /// The mask layer clipping this one (an index into the timeline's layers).
    mask: Option<usize>,
    /// Sorted by `index`.
    frames: Vec<KeyFrame>,
}

impl Layer {
    fn at(&self, frame: u32) -> Option<&KeyFrame> {
        let i = self.frames.partition_point(|k| k.index <= frame);
        let k = self.frames.get(i.checked_sub(1)?)?;
        (frame < k.index + k.duration.max(1)).then_some(k)
    }
}

#[derive(Debug, Default)]
pub struct Timeline {
    /// Bottom layer first (draw order).
    layers: Vec<Layer>,
    pub len: u32,
}

struct Sprite {
    tex: TexId,
    frame: Frame,
    /// `replaceSymbolGraphic`: a whole texture stretched over the frame's area.
    replaced: Option<TexId>,
}

/// Which timeline an animation plays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TimelineRef {
    Main,
    Symbol(usize),
}

/// A playable range of a timeline (a frame label or a whole symbol).
#[derive(Debug, Clone, Copy)]
pub struct AnimRange {
    pub tl: TimelineRef,
    pub start: u32,
    pub len: u32,
}

/// Axis-aligned bounds `[min_x, min_y, max_x, max_y]`.
pub type Bounds = [f32; 4];

pub struct AnimateAtlas {
    sprites: Vec<Sprite>,
    symbols: Vec<Timeline>,
    symbol_idx: HashMap<String, usize>,
    main: Timeline,
    pub fps: f32,
    /// flixel-animate `swfMode` (on for characters): movie clips play like graphics.
    pub swf_mode: bool,
    labels: Vec<(String, u32, u32)>,
    /// `applyStageMatrix`: the stage instance's matrix (`AN.STI`).
    pub stage: Affine,
    /// Symbol index → forced frame (`SymbolInstance.firstFrame` set from code).
    frame_overrides: HashMap<usize, u32>,
    /// The main timeline's symbol name (`AN.SN`): flixel-animate's default animation.
    pub main_name: String,
    /// Symbols whose own art is hidden (a script cleared their frames' elements);
    /// sprites injected into them still draw.
    hidden: std::collections::HashSet<usize>,
}

/// What the walker visits: a sprite (with its matrix, alpha and layer tint),
/// or a step of drawing through a mask.
enum Visit<'s> {
    Sprite(&'s Sprite, &'s Affine, f32, Option<Color>),
    Mask(MaskOp),
    /// A symbol instance was drawn (index, matrix, alpha): where injected sprites go.
    Symbol(usize, &'s Affine, f32),
}

/// Draws a sprite inside the atlas, through a symbol's matrix (`FlxSpriteElement`).
pub type Inject<'a> = &'a dyn Fn(&mut Gfx, &Affine, f32);

type Visitor<'a> = dyn FnMut(Visit<'_>) + 'a;

/// Extra per-draw options.
#[derive(Default, Clone, Copy)]
pub struct DrawOpts<'a> {
    /// Elements on layers with this name are filled with this colour
    /// (`setColorTransform(0, 0, 0, 1, r, g, b, 0)`).
    pub tint_layer: Option<(&'a str, Color)>,
    /// After each instance of these symbols, draw this (`frame.add(element)`).
    pub inject: &'a [(Vec<usize>, Inject<'a>)],
}

fn get<'a>(v: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter().find_map(|k| v.get(*k))
}
fn get_str(v: &Value, keys: &[&str]) -> Option<String> {
    get(v, keys).and_then(|x| x.as_str()).map(str::to_owned)
}
fn get_f(v: &Value, keys: &[&str]) -> Option<f64> {
    get(v, keys).and_then(|x| x.as_f64())
}

fn read_json(path: &Path) -> Result<Value, String> {
    let text = funkin_core::vfs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| format!("{}: {e}", path.display()))
}

fn parse_matrix(v: &Value) -> Affine {
    if let Some(a) = get(v, &["MX"]).and_then(|x| x.as_array()) {
        let n = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Affine([n(0), n(1), n(2), n(3), n(4), n(5)]);
    }
    if let Some(a) = get(v, &["M3D"]).and_then(|x| x.as_array()) {
        let n = |i: usize| a.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Affine([n(0), n(1), n(4), n(5), n(12), n(13)]);
    }
    if let Some(m) = get(v, &["Matrix3D"]) {
        let n = |k: &str| m.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Affine([n("m00"), n("m01"), n("m10"), n("m11"), n("m30"), n("m31")]);
    }
    if let Some(m) = get(v, &["Matrix"]) {
        let n = |k: &str| m.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        return Affine([n("a"), n("b"), n("c"), n("d"), n("tx"), n("ty")]);
    }
    Affine::IDENTITY
}

/// `FilterRenderer.expandFilterBounds`: blur and glow grow by `ceil(blur)` per
/// side; drop shadows also by their offset. Filters add up.
fn filter_extension(si: &Value) -> [f32; 4] {
    let mut e = [0.0f32; 4];
    let list = get(si, &["F", "filters"]).and_then(|f| f.as_array()).cloned().unwrap_or_default();
    for f in &list {
        let n = |k: &[&str]| get_f(f, k).unwrap_or(0.0) as f32;
        let ceil = |v: f32| if v > 0.0 { v.ceil() } else { 0.0 };
        let (bx, by) = (n(&["BLX", "blurX"]), n(&["BLY", "blurY"]));
        let inner = f.get("IN").or_else(|| f.get("inner")).and_then(|v| v.as_bool()).unwrap_or(false);
        let add = match get_str(f, &["N", "name"]).as_deref() {
            Some("BLF" | "blurFilter" | "BlurFilter") => [ceil(bx), ceil(by), ceil(bx), ceil(by)],
            Some("GF" | "glowFilter" | "GlowFilter") if !inner => [ceil(bx), ceil(by), ceil(bx), ceil(by)],
            Some("DSF" | "dropShadowFilter" | "DropShadowFilter") => {
                let (d, a) = (n(&["D", "distance"]), n(&["AL", "angle"]).to_radians());
                let (ox, oy) = ((d * a.cos()) as i32 as f32, (d * a.sin()) as i32 as f32);
                [(ox.min(0.0).abs() + bx).ceil(), (oy.min(0.0).abs() + by).ceil(), (ox.max(0.0) + bx).ceil(), (oy.max(0.0) + by).ceil()]
            }
            _ => [0.0; 4],
        };
        for i in 0..4 {
            e[i] += add[i];
        }
    }
    e
}

fn parse_element(e: &Value) -> Option<Element> {
    if let Some(si) = get(e, &["SI", "SYMBOL_Instance"]) {
        let is_mc = matches!(get_str(si, &["ST", "symbolType"]).as_deref(), Some("MC" | "movieclip"));
        let mode = match get_str(si, &["LP", "loop"]).as_deref() {
            Some("PO" | "playonce") => LoopMode::PlayOnce,
            Some("SF" | "singleframe") => LoopMode::Single,
            _ => LoopMode::Loop,
        };
        let alpha = get(si, &["C", "color"])
            .and_then(|c| get_f(c, &["AM", "alphaMultiplier"]))
            .unwrap_or(1.0) as f32;
        return Some(Element::Symbol {
            name: get_str(si, &["SN", "SYMBOL_name"])?,
            idx: None,
            first: get_f(si, &["FF", "firstFrame"]).unwrap_or(0.0) as u32,
            mode,
            mc: is_mc,
            m: parse_matrix(si),
            alpha,
            ext: if is_mc { filter_extension(si) } else { [0.0; 4] },
        });
    }
    if let Some(asi) = get(e, &["ASI", "ATLAS_SPRITE_instance"]) {
        return Some(Element::Sprite { name: get_str(asi, &["N", "name"])?, idx: None, m: parse_matrix(asi) });
    }
    None
}

fn parse_timeline(tl: &Value) -> Timeline {
    let mut out = Timeline::default();
    let layers = get(tl, &["L", "LAYERS"]).and_then(|l| l.as_array()).cloned().unwrap_or_default();
    // Layers are listed top-first; draw bottom-first.
    let mut clipped_by = Vec::new();
    for l in layers.iter().rev() {
        // Folders have no frames.
        let kind = get_str(l, &["LT", "Layer_type"]);
        if matches!(kind.as_deref(), Some("Fld" | "Folder")) {
            continue;
        }
        let mut layer = Layer {
            name: get_str(l, &["LN", "Layer_name"]).unwrap_or_default(),
            is_mask: matches!(kind.as_deref(), Some("Clp" | "Clipper")),
            ..Default::default()
        };
        clipped_by.push(get_str(l, &["Clpb", "Clipped_by"]));
        for f in get(l, &["FR", "Frames"]).and_then(|x| x.as_array()).into_iter().flatten() {
            let index = get_f(f, &["I", "index"]).unwrap_or(0.0) as u32;
            let duration = get_f(f, &["DU", "duration"]).unwrap_or(1.0) as u32;
            out.len = out.len.max(index + duration);
            layer.frames.push(KeyFrame {
                index,
                duration,
                label: get_str(f, &["N", "name"]).filter(|s| !s.is_empty()),
                elements: get(f, &["E", "elements"])
                    .and_then(|x| x.as_array())
                    .into_iter()
                    .flatten()
                    .filter_map(parse_element)
                    .collect(),
            });
        }
        layer.frames.sort_by_key(|k| k.index);
        out.layers.push(layer);
    }
    for (i, by) in clipped_by.into_iter().enumerate() {
        if let Some(by) = by {
            out.layers[i].mask = out.layers.iter().position(|l| l.is_mask && l.name == by);
        }
    }
    out
}

impl AnimateAtlas {
    /// Load the atlas in folder `dir`.
    pub fn load(gfx: &mut Gfx, dir: &Path, smooth: bool) -> Result<Self, String> {
        let anim = read_json(&dir.join("Animation.json"))?;

        // Spritemaps.
        let mut sprites = Vec::new();
        let mut sprite_idx = HashMap::new();
        let mut maps: Vec<std::path::PathBuf> =
            (1..64).map(|i| dir.join(format!("spritemap{i}.json"))).take_while(|p| funkin_core::vfs::exists(p)).collect();
        if maps.is_empty() && funkin_core::vfs::exists(&dir.join("spritemap.json")) {
            maps.push(dir.join("spritemap.json"));
        }
        if maps.is_empty() {
            return Err(format!("{}: no spritemap", dir.display()));
        }
        for map in maps {
            let v = read_json(&map)?;
            let image = v
                .get("meta")
                .and_then(|m| m.get("image"))
                .and_then(|x| x.as_str())
                .map(|s| dir.join(s))
                .unwrap_or_else(|| map.with_extension("png"));
            let tex = gfx.load_texture_filtered(&image, smooth)?;
            let list = v.get("ATLAS").and_then(|a| a.get("SPRITES")).and_then(|s| s.as_array());
            for s in list.into_iter().flatten() {
                let s = s.get("SPRITE").unwrap_or(s);
                let n = |k: &str| s.get(k).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let rotated = s.get("rotated").and_then(|x| x.as_bool()).unwrap_or(false);
                // Listed size is the stored region; a rotated sprite is h×w unrotated.
                let (w, h) = if rotated { (n("h"), n("w")) } else { (n("w"), n("h")) };
                let name = s.get("name").and_then(|x| x.as_str()).unwrap_or_default().to_owned();
                sprite_idx.insert(name.clone(), sprites.len());
                sprites.push(Sprite {
                    tex,
                    frame: Frame {
                        name,
                        x: n("x"),
                        y: n("y"),
                        w,
                        h,
                        frame_x: 0.0,
                        frame_y: 0.0,
                        frame_w: w,
                        frame_h: h,
                        rotated,
                    },
                    replaced: None,
                });
            }
        }

        let an = get(&anim, &["AN", "ANIMATION"]).ok_or("Animation.json: no AN")?;
        let mut main = get(an, &["TL", "TIMELINE"]).map(parse_timeline).unwrap_or_default();
        let mut symbols = Vec::new();
        let mut symbol_idx = HashMap::new();
        let dict = get(&anim, &["SD", "SYMBOL_DICTIONARY"]).and_then(|d| get(d, &["S", "Symbols"])).and_then(|s| s.as_array());
        for s in dict.into_iter().flatten() {
            let Some(name) = get_str(s, &["SN", "SYMBOL_name"]) else { continue };
            symbol_idx.insert(name, symbols.len());
            symbols.push(get(s, &["TL", "TIMELINE"]).map(parse_timeline).unwrap_or_default());
        }
        let fps = get(&anim, &["MD", "metadata"]).and_then(|m| get_f(m, &["FRT", "framerate"])).unwrap_or(24.0) as f32;

        // Resolve names to indices.
        let resolve = |tl: &mut Timeline| {
            for l in &mut tl.layers {
                for k in &mut l.frames {
                    for e in &mut k.elements {
                        match e {
                            Element::Symbol { name, idx, .. } => *idx = symbol_idx.get(name.as_str()).copied(),
                            Element::Sprite { name, idx, .. } => *idx = sprite_idx.get(name.as_str()).copied(),
                        }
                    }
                }
            }
        };
        resolve(&mut main);
        for s in &mut symbols {
            resolve(s);
        }

        let mut labels = Vec::new();
        for l in &main.layers {
            for k in &l.frames {
                if let Some(n) = &k.label {
                    labels.push((n.clone(), k.index, k.duration.max(1)));
                }
            }
        }
        let stage = get(an, &["STI", "StageInstance"]).and_then(|s| get(s, &["SI", "SYMBOL_Instance"])).map(parse_matrix).unwrap_or(Affine::IDENTITY);
        let main_name = get_str(an, &["SN", "SYMBOL_name"]).unwrap_or_default();
        Ok(Self { sprites, symbols, symbol_idx, main, fps, swf_mode: false, labels, stage, frame_overrides: HashMap::new(), main_name, hidden: Default::default() })
    }

    fn timeline(&self, r: TimelineRef) -> &Timeline {
        match r {
            TimelineRef::Main => &self.main,
            TimelineRef::Symbol(i) => &self.symbols[i],
        }
    }

    /// Resolve an animation prefix: a main-timeline frame label, else a symbol name.
    pub fn anim(&self, name: &str) -> Option<AnimRange> {
        if let Some((_, start, len)) = self.labels.iter().find(|(n, ..)| n == name) {
            return Some(AnimRange { tl: TimelineRef::Main, start: *start, len: *len });
        }
        let i = *self.symbol_idx.get(name)?;
        Some(AnimRange { tl: TimelineRef::Symbol(i), start: 0, len: self.symbols[i].len.max(1) })
    }

    /// A symbol's own timeline (`addBySymbol`).
    pub fn symbol(&self, name: &str) -> Option<AnimRange> {
        let i = *self.symbol_idx.get(name)?;
        Some(AnimRange { tl: TimelineRef::Symbol(i), start: 0, len: self.symbols[i].len.max(1) })
    }

    /// A frame label on the main timeline (`addByFrameLabel`).
    pub fn label(&self, name: &str) -> Option<AnimRange> {
        let (_, start, len) = self.labels.iter().find(|(n, ..)| n == name)?;
        Some(AnimRange { tl: TimelineRef::Main, start: *start, len: *len })
    }

    /// Whole timeline as one animation.
    pub fn whole(&self) -> AnimRange {
        AnimRange { tl: TimelineRef::Main, start: 0, len: self.main.len.max(1) }
    }

    /// The main-timeline frame label that starts on `frame`, if any.
    pub fn label_starting_at(&self, frame: u32) -> Option<&str> {
        self.labels.iter().find(|(_, start, _)| *start == frame).map(|(n, ..)| n.as_str())
    }

    /// Does `symbol` have an element on its first frame (`getFirstElement`)?
    pub fn has_first_element(&self, symbol: &str) -> bool {
        self.first_element_mut_idx(symbol).is_some()
    }

    fn first_element_mut_idx(&self, symbol: &str) -> Option<(usize, usize, usize)> {
        let &si = self.symbol_idx.get(symbol)?;
        // flixel-animate walks layers top first; ours are stored bottom first.
        let tl = &self.symbols[si];
        tl.layers.iter().enumerate().rev().find_map(|(li, l)| {
            let ki = l.frames.iter().position(|k| k.index == 0 && !k.elements.is_empty())?;
            Some((si, li, ki))
        })
    }

    /// `FunkinSprite.scaleElement(getFirstElement(symbol), scale, offset, everything)`.
    pub fn scale_first_element(&mut self, symbol: &str, scale: f32, offset: f32, everything: bool) {
        let Some((si, li, ki)) = self.first_element_mut_idx(symbol) else { return };
        let el = &mut self.symbols[si].layers[li].frames[ki].elements[0];
        let m = match el {
            Element::Symbol { m, .. } | Element::Sprite { m, .. } => m,
        };
        if everything {
            for v in &mut m.0 {
                *v *= scale;
            }
            return;
        }
        // The frame converted to a fresh symbol has its transformation point at 0,0.
        m.0[0] += scale;
        m.0[3] += scale;
        m.0[4] -= offset;
        m.0[5] -= offset;
    }

    /// Force symbol `name` to show frame `f` wherever it is placed (`None` clears).
    pub fn set_symbol_frame(&mut self, name: &str, f: Option<u32>) {
        let Some(&i) = self.symbol_idx.get(name) else { return };
        match f {
            Some(f) => self.frame_overrides.insert(i, f),
            None => self.frame_overrides.remove(&i),
        };
    }

    /// Hide the art of the symbols named `name` (or containing it, unless `exact`).
    pub fn hide_symbols(&mut self, name: &str, exact: bool) {
        let ids: Vec<usize> = self.symbol_idx.iter().filter(|(n, _)| if exact { n.as_str() == name } else { n.contains(name) }).map(|(_, &i)| i).collect();
        self.hidden.extend(ids);
    }

    /// Does the atlas have this frame label or symbol?
    /// Symbols whose names contain `keyword` (`getFramesWithKeyword`).
    pub fn symbols_with(&self, keyword: &str) -> Vec<usize> {
        self.symbol_idx.iter().filter(|(n, _)| n.contains(keyword)).map(|(_, &i)| i).collect()
    }

    pub fn has(&self, name: &str) -> bool {
        self.anim(name).is_some()
    }

    /// Visit every sprite drawn (masks ignored: clipped layers count whole).
    #[allow(clippy::too_many_arguments)]
    fn walk_inner(
        &self,
        tl: &Timeline,
        frame: u32,
        m: &Affine,
        alpha: f32,
        depth: u32,
        tint: Option<Color>,
        tint_layer: Option<(&str, Color)>,
        masks: bool,
        out: &mut Visitor,
    ) {
        if depth > 32 {
            return;
        }
        for layer in &tl.layers {
            if layer.is_mask {
                continue;
            }
            // A mask with nothing on this frame doesn't clip (as flixel-animate).
            let mask = layer.mask.filter(|_| masks).map(|i| &tl.layers[i]).filter(|l| l.at(frame).is_some_and(|k| !k.elements.is_empty()));
            if let Some(ml) = mask {
                out(Visit::Mask(MaskOp::Begin));
                self.walk_layer(ml, frame, m, alpha, depth, tint, tint_layer, masks, out);
                out(Visit::Mask(MaskOp::Content));
            }
            self.walk_layer(layer, frame, m, alpha, depth, tint, tint_layer, masks, out);
            if let Some(ml) = mask {
                out(Visit::Mask(MaskOp::End));
                self.walk_layer(ml, frame, m, alpha, depth, tint, tint_layer, masks, out);
                out(Visit::Mask(MaskOp::Done));
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn walk_layer(
        &self,
        layer: &Layer,
        frame: u32,
        m: &Affine,
        alpha: f32,
        depth: u32,
        tint: Option<Color>,
        tint_layer: Option<(&str, Color)>,
        masks: bool,
        out: &mut Visitor,
    ) {
        {
            let Some(k) = layer.at(frame) else { return };
            let tint = match tint_layer {
                Some((n, c)) if layer.name == n => Some(c),
                _ => tint,
            };
            for e in &k.elements {
                match e {
                    Element::Sprite { idx: Some(i), m: em, .. } => out(Visit::Sprite(&self.sprites[*i], &m.then(em), alpha, tint)),
                    Element::Symbol { idx: Some(s), first, mode, mc, m: em, alpha: ea, .. } => {
                        let sym = &self.symbols[*s];
                        let len = sym.len.max(1);
                        let local = frame - k.index;
                        let f = match mode {
                            _ if self.frame_overrides.contains_key(s) => self.frame_overrides[s].min(len - 1),
                            _ if *mc && !self.swf_mode => 0,
                            LoopMode::Single => *first,
                            LoopMode::Loop => (first + local) % len,
                            LoopMode::PlayOnce => (first + local).min(len - 1),
                        };
                        let sm = m.then(em);
                        if !self.hidden.contains(s) {
                            self.walk_inner(sym, f, &sm, alpha * ea, depth + 1, tint, tint_layer, masks, out);
                        }
                        out(Visit::Symbol(*s, &sm, alpha * ea));
                    }
                    _ => {}
                }
            }
        }
    }

    /// Draw frame `frame` of timeline `tl` through `m`.
    pub fn draw(&self, gfx: &mut Gfx, tl: TimelineRef, frame: u32, m: &Affine, color: Color) {
        self.draw_opts(gfx, tl, frame, m, color, DrawOpts::default());
    }

    /// [`Self::draw`] with [`DrawOpts`].
    pub fn draw_opts(&self, gfx: &mut Gfx, tl: TimelineRef, frame: u32, m: &Affine, color: Color, opts: DrawOpts) {
        self.walk_inner(self.timeline(tl), frame, m, 1.0, 0, None, opts.tint_layer, true, &mut |v| {
            let (s, sm, a, tint) = match v {
                Visit::Sprite(s, sm, a, tint) => (s, sm, a, tint),
                Visit::Mask(op) => return gfx.mask(op),
                Visit::Symbol(i, sm, a) => {
                    for (syms, f) in opts.inject {
                        if syms.contains(&i) {
                            f(gfx, sm, color[3] * a);
                        }
                    }
                    return;
                }
            };
            let (w, h) = (s.frame.w, s.frame.h);
            let dst = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|p| sm.apply(p));
            let c = match tint {
                Some(t) => super::fill([t[0], t[1], t[2], color[3] * a]),
                None => [color[0], color[1], color[2], color[3] * a],
            };
            match s.replaced {
                Some(t) => gfx.quad_uv(t, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], dst, c),
                None => draw_region(gfx, s.tex, &s.frame, dst, c),
            }
        });
    }

    /// `FunkinSprite.replaceSymbolGraphic` (with `adjustScale`): swap the sprites on
    /// frame 0 of `symbol` for `tex`, scaled to their size; `None` restores them.
    pub fn replace_symbol_graphic(&mut self, symbol: &str, tex: Option<TexId>) {
        let Some(&i) = self.symbol_idx.get(symbol) else { return };
        let mut hits = Vec::new();
        for l in &self.symbols[i].layers {
            if let Some(k) = l.at(0) {
                for e in &k.elements {
                    if let Element::Sprite { idx: Some(si), .. } = e {
                        hits.push(*si);
                    }
                }
            }
        }
        for si in hits {
            self.sprites[si].replaced = tex;
        }
    }

    /// Bounds of one frame of `tl`, filters included (`Timeline.getBounds`).
    pub fn frame_bounds(&self, tl: TimelineRef, frame: u32) -> Bounds {
        let b = self.tl_bounds(self.timeline(tl), frame, &Affine::IDENTITY, 0);
        if b[0] > b[2] {
            [0.0; 4]
        } else {
            b
        }
    }

    /// Bounds of frame `frame` of `tl` through `m`; empty is `[MAX, MAX, MIN, MIN]`.
    /// A movie clip's filters grow its bounds after its transform.
    fn tl_bounds(&self, tl: &Timeline, frame: u32, m: &Affine, depth: u32) -> Bounds {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        if depth > 32 {
            return b;
        }
        let grow = |b: &mut Bounds, o: Bounds| {
            *b = [b[0].min(o[0]), b[1].min(o[1]), b[2].max(o[2]), b[3].max(o[3])];
        };
        for layer in &tl.layers {
            if layer.is_mask {
                continue;
            }
            let Some(k) = layer.at(frame) else { continue };
            for e in &k.elements {
                match e {
                    Element::Sprite { idx: Some(i), m: em, .. } => {
                        let s = &self.sprites[*i];
                        let sm = m.then(em);
                        for p in [[0.0, 0.0], [s.frame.w, 0.0], [s.frame.w, s.frame.h], [0.0, s.frame.h]] {
                            let q = sm.apply(p);
                            grow(&mut b, [q[0], q[1], q[0], q[1]]);
                        }
                    }
                    Element::Symbol { idx: Some(si), first, mode, mc, m: em, ext, .. } => {
                        let sym = &self.symbols[*si];
                        let len = sym.len.max(1);
                        let local = frame - k.index;
                        let f = match mode {
                            _ if self.frame_overrides.contains_key(si) => self.frame_overrides[si].min(len - 1),
                            _ if *mc && !self.swf_mode => 0,
                            LoopMode::Single => *first,
                            LoopMode::Loop => (first + local) % len,
                            LoopMode::PlayOnce => (first + local).min(len - 1),
                        };
                        let mut c = self.tl_bounds(sym, f, &m.then(em), depth + 1);
                        if c[0] <= c[2] {
                            c = [c[0] - ext[0], c[1] - ext[1], c[2] + ext[2], c[3] + ext[3]];
                            grow(&mut b, c);
                        }
                    }
                    _ => {}
                }
            }
        }
        b
    }

    /// Union of the bounds of every frame of `tl` (flixel-animate's timeline bounds).
    pub fn bounds(&self, tl: TimelineRef) -> Bounds {
        let t = self.timeline(tl);
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for f in 0..t.len.max(1) {
            let o = self.tl_bounds(t, f, &Affine::IDENTITY, 0);
            b = [b[0].min(o[0]), b[1].min(o[1]), b[2].max(o[2]), b[3].max(o[3])];
        }
        if b[0] > b[2] {
            [0.0; 4]
        } else {
            b
        }
    }
}
