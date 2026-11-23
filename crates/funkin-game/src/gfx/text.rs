//! Port of `AtlasText.hx`: text drawn with one of the game's Sparrow fonts
//! (`images/fonts/bold`, `default`, `freeplay-clear`), loaded from the user's
//! assets like everything else. Glyphs animate at 24 fps like `AtlasChar`.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::Path;

use super::atlas::Atlas;
use super::{Color, Gfx};

const SPACE_W: f32 = 40.0;

pub struct Font {
    atlas: Atlas,
    glyphs: HashMap<char, Vec<usize>>,
    /// `Case.Upper` fonts (bold) only have capitals.
    upper: bool,
    /// Tallest glyph region; the line height and baseline.
    pub max_height: f32,
    /// Animation clock for the glyphs (seconds).
    time: Cell<f32>,
}

/// `AtlasChar.getAnimPrefix`.
fn anim_prefix(c: char) -> String {
    match c {
        '&' => "-andpersand-",
        '😠' => "-angry faic-",
        '\'' => "-apostraphie-",
        '\\' => "-back slash-",
        ',' => "-comma-",
        '-' => "-dash-",
        '↓' => "-down arrow-",
        '”' => "-end quote-",
        '!' => "-exclamation point-",
        '/' => "-forward slash-",
        '>' => "-greater than-",
        '♥' | '♡' => "-heart-",
        '←' => "-left arrow-",
        '<' => "-less than-",
        '*' => "-multiply x-",
        '.' => "-period-",
        '?' => "-question mark-",
        '→' => "-right arrow-",
        '“' => "-start quote-",
        '↑' => "-up arrow-",
        _ => return c.to_string(),
    }
    .to_owned()
}

impl Font {
    pub fn load(gfx: &mut Gfx, base: &Path, upper: bool) -> Result<Self, String> {
        let atlas = Atlas::load(gfx, base)?;
        let max_height = atlas.frames.iter().map(|f| f.h).fold(0.0, f32::max);
        Ok(Self { atlas, glyphs: HashMap::new(), upper, max_height, time: Cell::new(0.0) })
            .map(|mut f| {
                let chars: Vec<char> = (' '..='~').chain("“”←↑→↓♥".chars()).collect();
                for c in chars {
                    let frames = f.atlas.prefix(&anim_prefix(c)).frames;
                    // Prefix matching: a one-letter name like "A" must not pick up "A..." words.
                    let frames: Vec<usize> = frames
                        .into_iter()
                        .filter(|&i| {
                            let n = &f.atlas.frames[i].name;
                            n.trim_end_matches(|d: char| d.is_ascii_digit()) == anim_prefix(c) || c.is_ascii_digit()
                        })
                        .collect();
                    if !frames.is_empty() {
                        f.glyphs.insert(c, frames);
                    }
                }
                f
            })
    }

    /// Advance the glyph animation clock.
    pub fn set_time(&self, t: f32) {
        self.time.set(t);
    }

    fn glyph(&self, c: char) -> Option<&Vec<usize>> {
        let c = if self.upper { c.to_ascii_uppercase() } else { c };
        self.glyphs.get(&c).or_else(|| self.glyphs.get(&c.to_ascii_uppercase()))
    }

    /// `AtlasText.getWidth` (widest line) at `scale`.
    pub fn width(&self, text: &str, scale: f32) -> f32 {
        text.split('\n')
            .map(|line| {
                line.chars()
                    .map(|c| match self.glyph(c) {
                        Some(g) if c != ' ' => self.atlas.frames[g[0]].frame_w,
                        _ => SPACE_W,
                    })
                    .sum::<f32>()
            })
            .fold(0.0, f32::max)
            * scale
    }

    /// Draw `text` with its top-left at `(x, y)`. Returns the width drawn.
    pub fn draw(&self, gfx: &mut Gfx, text: &str, x: f32, y: f32, scale: f32, color: Color) -> f32 {
        let tick = (self.time.get() * 24.0) as usize;
        let (mut cx, mut cy, mut widest) = (x, y, 0.0f32);
        for c in text.chars() {
            match c {
                '\n' => {
                    widest = widest.max(cx - x);
                    cx = x;
                    cy += self.max_height * scale;
                }
                ' ' => cx += SPACE_W * scale,
                _ => match self.glyph(c) {
                    Some(g) => {
                        let first = &self.atlas.frames[g[0]];
                        let gy = cy + (self.max_height - first.frame_h) * scale;
                        self.atlas.draw(gfx, g[tick % g.len()], cx, gy, scale, color);
                        cx += first.frame_w * scale;
                    }
                    None => cx += SPACE_W * scale,
                },
            }
        }
        widest.max(cx - x)
    }

    pub fn draw_centered(&self, gfx: &mut Gfx, text: &str, cx: f32, y: f32, scale: f32, color: Color) {
        let w = self.width(text, scale);
        self.draw(gfx, text, cx - w / 2.0, y, scale, color);
    }
}
