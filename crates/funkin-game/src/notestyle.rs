//! Port of `NoteStyle`: the per-song note skin (`data/notestyles/<id>.json`,
//! e.g. `funkin` or Week 6's `pixel`), with its `fallback` chain.

use std::sync::Arc;

use serde_json::Value;

use crate::assets::Assets;
use crate::audio::Pcm;
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{Gfx, TexId};

/// A note style's JSON and its fallback style.
struct Style {
    assets: Value,
    fallback: Option<Box<Style>>,
}

impl Style {
    fn load(a: &Assets, id: &str, depth: u32) -> Option<Self> {
        let p = a.root().join("data").join("notestyles").join(format!("{id}.json"));
        let v: Value = serde_json::from_str(&funkin_core::vfs::read_to_string(&p).ok()?).ok()?;
        let fallback = v
            .get("fallback")
            .and_then(|f| f.as_str())
            .filter(|f| *f != id && depth < 8)
            .and_then(|f| Style::load(a, f, depth + 1))
            .map(Box::new);
        Some(Self { assets: v.get("assets").cloned().unwrap_or(Value::Null), fallback })
    }

    /// This style's own `assets.<key>.<path...>` (JSON null counts as missing).
    fn own(&self, key: &str, path: &[&str]) -> Option<&Value> {
        let mut v = self.assets.get(key)?;
        for p in path {
            v = v.get(p)?;
        }
        (!v.is_null()).then_some(v)
    }

    /// `own ?? fallback?.get(...)`, as most `NoteStyle` getters do.
    fn get(&self, key: &str, path: &[&str]) -> Option<&Value> {
        self.own(key, path).or_else(|| self.fallback.as_ref()?.get(key, path))
    }

    fn str(&self, key: &str, path: &[&str]) -> Option<&str> {
        self.get(key, path)?.as_str()
    }

    fn scale(&self, key: &str) -> f32 {
        self.get(key, &["scale"]).and_then(|v| v.as_f64()).unwrap_or(1.0) as f32
    }

    fn offsets(&self, key: &str) -> [f32; 2] {
        match self.get(key, &["offsets"]).and_then(|v| v.as_array()) {
            Some(o) if o.len() == 2 => [o[0].as_f64().unwrap_or(0.0) as f32, o[1].as_f64().unwrap_or(0.0) as f32],
            _ => [0.0, 0.0],
        }
    }

    /// `isPixel` is read from the style itself only (`_data.assets.x?.isPixel ?? false`).
    fn is_pixel(&self, key: &str) -> bool {
        self.own(key, &["isPixel"]).and_then(|v| v.as_bool()).unwrap_or(false)
    }

    /// The asset path of an element: this style's own entry, else the fallback's.
    fn asset_path(&self, key: &str) -> Option<&str> {
        self.str(key, &["assetPath"]).filter(|s| !s.is_empty())
    }

    /// `getHoldCoverDirectionAssetPath`: the direction's path, then this
    /// style's root path, then the fallback's resolution.
    fn cover_path(&self, dir: &str) -> Option<&str> {
        self.own("holdNoteCover", &["data", dir, "assetPath"])
            .and_then(|v| v.as_str())
            .or_else(|| self.own("holdNoteCover", &["assetPath"]).and_then(|v| v.as_str()))
            .filter(|s| !s.is_empty())
            .or_else(|| self.fallback.as_ref()?.cover_path(dir))
    }
}

pub const DIRS: [&str; 4] = ["left", "down", "up", "right"];
const COLORS: [&str; 4] = ["Purple", "Blue", "Green", "Red"];

/// Graphics and sounds for one note style. Loaded once per style and reused.
pub struct NoteSkin {
    pub id: String,
    pub notes: Atlas,
    pub note_frames: [usize; 4],
    pub note_scale: f32,
    /// Each lane's note height after scaling (its hitbox).
    pub note_h: [f32; 4],
    pub strum: Atlas,
    pub statics: [Anim; 4],
    pub presses: [Anim; 4],
    pub confirms: [Anim; 4],
    pub strum_scale: f32,
    pub strum_off: [f32; 2],
    /// The receptor hitbox: the atlas's first frame, scaled.
    pub strum_size: (f32, f32),
    pub hold: TexId,
    pub hold_scale: f32,
    pub hold_off: [f32; 2],
    pub hold_pixel: bool,
    pub ratings: [TexId; 4],
    pub rating_sizes: [(f32, f32); 4],
    pub rating_scale: f32,
    pub digits: [TexId; 10],
    pub digit_scale: f32,
    /// Popups fade in two steps (`EaseUtil.stepped(2)`).
    pub popup_pixel: bool,
    pub health_bar: TexId,
    pub countdown: [Option<TexId>; 3],
    pub countdown_scale: f32,
    /// The countdown fades in eight steps instead of `cubeInOut`.
    pub countdown_pixel: bool,
    pub countdown_sfx: [Option<Arc<Pcm>>; 4],
    pub miss_sfx: Vec<Arc<Pcm>>,
    /// Hold covers per lane: atlas and (start, hold, end) animations.
    pub covers: Vec<(Atlas, [Anim; 3])>,
    pub cover_scale: f32,
    pub cover_off: [f32; 2],
}

impl NoteSkin {
    /// A receptor centred on `(cx, cy)`: `state` 0 static, 1 pressed, 2 confirm (`t` seconds in).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_receptor(&self, gfx: &mut Gfx, lane: usize, state: u8, t: f32, cx: f32, cy: f32, alpha: f32) {
        let (anim, t) = match state {
            0 => (&self.statics[lane], 0.0),
            1 => (&self.presses[lane], 0.0),
            _ => (&self.confirms[lane], t),
        };
        if let Some(f) = anim.frame_at(t, 24.0, false) {
            self.strum.draw_centered(gfx, f, cx, cy, self.strum_scale, [1.0, 1.0, 1.0, alpha]);
        }
    }

    /// A note centred on `(cx, cy)`.
    pub fn draw_note(&self, gfx: &mut Gfx, lane: usize, cx: f32, cy: f32, alpha: f32) {
        self.notes.draw_centered(gfx, self.note_frames[lane], cx, cy, self.note_scale, [1.0, 1.0, 1.0, alpha]);
    }

    /// Load note style `id` (falling back to `funkin` if it doesn't exist).
    pub fn load(gfx: &mut Gfx, a: &Assets, id: &str) -> Result<Self, String> {
        let (id, style) = match Style::load(a, id, 0) {
            Some(s) => (id.to_owned(), s),
            None => {
                log::warn!("note style {id} not found; using funkin");
                ("funkin".to_owned(), Style::load(a, "funkin", 0).ok_or("note style funkin not found")?)
            }
        };
        let atlas = |gfx: &mut Gfx, key: &str| -> Result<Atlas, String> {
            let path = style.asset_path(key).ok_or_else(|| format!("note style {id}: no {key}"))?;
            a.atlas_filtered(gfx, path, !style.is_pixel(key))
        };
        let tex = |gfx: &mut Gfx, key: &str| -> Option<TexId> {
            let path = style.asset_path(key)?;
            a.texture_filtered(gfx, path, !style.is_pixel(key)).map_err(|e| log::warn!("{e}")).ok()
        };

        // Notes.
        let notes = atlas(gfx, "note")?;
        let note_scale = style.scale("note");
        let note_frames = DIRS.map(|d| {
            let prefix = style.str("note", &["data", d, "prefix"]).unwrap_or("");
            notes.prefix(prefix).frames.first().copied().unwrap_or(0)
        });
        let note_h = note_frames.map(|f| notes.frames.get(f).map(|f| f.frame_h).unwrap_or(0.0) * note_scale);

        // Receptors.
        let strum = atlas(gfx, "noteStrumline")?;
        let strum_scale = style.scale("noteStrumline");
        let anim = |d: &str, kind: &str| strum.prefix(style.str("noteStrumline", &["data", &format!("{d}{kind}"), "prefix"]).unwrap_or("\u{0}"));
        let statics = DIRS.map(|d| anim(d, "Static"));
        let presses = DIRS.map(|d| anim(d, "Press"));
        let confirms = DIRS.map(|d| anim(d, "Confirm"));
        let strum_size = strum.frames.first().map(|f| (f.frame_w * strum_scale, f.frame_h * strum_scale)).unwrap_or((0.0, 0.0));

        // Hold trails.
        let hold = tex(gfx, "holdNote").ok_or_else(|| format!("note style {id}: no hold note graphic"))?;

        // Hold covers.
        let mut covers = Vec::new();
        let covers_on = style.get("holdNoteCover", &["data", "enabled"]).and_then(|v| v.as_bool()).unwrap_or(false);
        if covers_on {
            let smooth = !style.is_pixel("holdNoteCover");
            for (d, color) in DIRS.iter().zip(COLORS) {
                let Some(path) = style.cover_path(d) else { break };
                let prefix = |part: &str, default: String| {
                    style.str("holdNoteCover", &["data", d, part, "prefix"]).map(str::to_owned).unwrap_or(default)
                };
                match a.atlas_filtered(gfx, path, smooth) {
                    Ok(at) => {
                        let anims = [
                            at.prefix(&prefix("start", format!("holdCoverStart{color}"))),
                            at.prefix(&prefix("hold", format!("holdCover{color}"))),
                            at.prefix(&prefix("end", format!("holdCoverEnd{color}"))),
                        ];
                        covers.push((at, anims));
                    }
                    Err(e) => log::warn!("hold cover: {e}"),
                }
            }
            if covers.len() != 4 {
                covers.clear();
            }
        }

        // Popups.
        let judgement = |gfx: &mut Gfx, n: &str| tex(gfx, &format!("judgement{n}")).ok_or_else(|| format!("note style {id}: no {n} graphic"));
        let ratings = [judgement(gfx, "Sick")?, judgement(gfx, "Good")?, judgement(gfx, "Bad")?, judgement(gfx, "Shit")?];
        let rating_sizes = ratings.map(|t| gfx.texture_size(t));
        let mut digits = [ratings[0]; 10];
        for (i, d) in digits.iter_mut().enumerate() {
            *d = tex(gfx, &format!("comboNumber{i}")).ok_or_else(|| format!("note style {id}: no combo number {i}"))?;
        }

        // Countdown.
        let steps = ["countdownThree", "countdownTwo", "countdownOne", "countdownGo"];
        let countdown = [tex(gfx, steps[1]), tex(gfx, steps[2]), tex(gfx, steps[3])];
        let countdown_sfx = steps.map(|k| style.str(k, &["data", "audioPath"]).and_then(|p| a.sound(p)));

        Ok(Self {
            note_scale,
            note_frames,
            note_h,
            statics,
            presses,
            confirms,
            strum_scale,
            strum_off: style.offsets("noteStrumline"),
            strum_size,
            hold,
            hold_scale: style.scale("holdNote"),
            hold_off: style.offsets("holdNote"),
            hold_pixel: style.is_pixel("holdNote"),
            ratings,
            rating_sizes,
            rating_scale: style.scale("judgementSick"),
            digits,
            digit_scale: style.scale("comboNumber0"),
            popup_pixel: style.is_pixel("judgementSick"),
            health_bar: a.texture(gfx, "shared:healthBar")?,
            countdown,
            countdown_scale: style.scale("countdownGo"),
            countdown_pixel: style.is_pixel("countdownGo"),
            countdown_sfx,
            miss_sfx: (1..=3).filter_map(|i| a.sound(&format!("shared:missnote{i}"))).collect(),
            covers,
            cover_scale: style.scale("holdNoteCover"),
            cover_off: style.offsets("holdNoteCover"),
            notes,
            strum,
            id,
        })
    }
}
