//! Adobe Animate texture atlases (`Animation.json` + `spritemapN.json/png`), the
//! format most Funkin' characters use (rendered in the original by `flixel-animate`).
//!
//! Both export flavours are supported: the compressed one (`AN`/`SD`/`MD`, `SI`/`ASI`,
//! `MX`/`M3D`) and the long-form one (`ANIMATION`/`SYMBOL_DICTIONARY`, `Matrix3D`).

use std::collections::HashMap;
use std::path::Path;

use serde_json::Value;

use super::atlas::{draw_region, Frame};
use super::{Affine, Color, Gfx, TexId};

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
    for l in layers.iter().rev() {
        // Mask layers ("Clp") are not drawn; folders have no frames.
        if matches!(get_str(l, &["LT", "Layer_type"]).as_deref(), Some("Clp" | "Clipper" | "Fld" | "Folder")) {
            continue;
        }
        let mut layer = Layer::default();
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
        Ok(Self { sprites, symbols, symbol_idx, main, fps, swf_mode: false, labels })
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

    fn walk(&self, tl: &Timeline, frame: u32, m: &Affine, alpha: f32, depth: u32, out: &mut dyn FnMut(&Sprite, &Affine, f32)) {
        if depth > 32 {
            return;
        }
        for layer in &tl.layers {
            let Some(k) = layer.at(frame) else { continue };
            for e in &k.elements {
                match e {
                    Element::Sprite { idx: Some(i), m: em, .. } => out(&self.sprites[*i], &m.then(em), alpha),
                    Element::Symbol { idx: Some(s), first, mode, mc, m: em, alpha: ea, .. } => {
                        let sym = &self.symbols[*s];
                        let len = sym.len.max(1);
                        let local = frame - k.index;
                        let f = match mode {
                            _ if *mc && !self.swf_mode => 0,
                            LoopMode::Single => *first,
                            LoopMode::Loop => (first + local) % len,
                            LoopMode::PlayOnce => (first + local).min(len - 1),
                        };
                        self.walk(sym, f, &m.then(em), alpha * ea, depth + 1, out);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Draw frame `frame` of timeline `tl` through `m`.
    pub fn draw(&self, gfx: &mut Gfx, tl: TimelineRef, frame: u32, m: &Affine, color: Color) {
        self.walk(self.timeline(tl), frame, m, 1.0, 0, &mut |s, sm, a| {
            let (w, h) = (s.frame.w, s.frame.h);
            let dst = [[0.0, 0.0], [w, 0.0], [w, h], [0.0, h]].map(|p| sm.apply(p));
            let c = [color[0], color[1], color[2], color[3] * a];
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

    /// Union of the bounds of every frame of `tl` (flixel-animate's timeline bounds).
    pub fn bounds(&self, tl: TimelineRef) -> Bounds {
        let t = self.timeline(tl);
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for f in 0..t.len.max(1) {
            self.walk(t, f, &Affine::IDENTITY, 1.0, 0, &mut |s, sm, _| {
                for p in [[0.0, 0.0], [s.frame.w, 0.0], [s.frame.w, s.frame.h], [0.0, s.frame.h]] {
                    let q = sm.apply(p);
                    b[0] = b[0].min(q[0]);
                    b[1] = b[1].min(q[1]);
                    b[2] = b[2].max(q[0]);
                    b[3] = b[3].max(q[1]);
                }
            });
        }
        if b[0] > b[2] {
            [0.0; 4]
        } else {
            b
        }
    }
}
