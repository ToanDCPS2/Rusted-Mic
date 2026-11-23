//! `FlxText` with the game's TrueType fonts (`5by7`, `VCR OSD Mono`), which are
//! extracted from the user's own `Funkin.exe` at import. Glyphs are rasterised
//! with `fontdue` on first use and cached as white alpha textures per size.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use super::{Color, Gfx, TexId};

/// `FlxText` pads its text field by 2 px on each side.
const GUTTER: f32 = 2.0;

struct Glyph {
    tex: Option<TexId>,
    w: f32,
    h: f32,
    xmin: f32,
    ymin: f32,
    advance: f32,
}

pub struct TtfFont {
    font: fontdue::Font,
    glyphs: RefCell<HashMap<(char, u32), Glyph>>,
}

impl TtfFont {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = funkin_core::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { font, glyphs: RefCell::new(HashMap::new()) })
    }

    /// Height of one line (ascent + descent + gap), as OpenFL spaces lines.
    pub fn line_height(&self, size: f32) -> f32 {
        self.font.horizontal_line_metrics(size).map(|m| m.new_line_size).unwrap_or(size * 1.15)
    }

    /// `FlxText.height` for `lines` lines: the text plus the 2 px gutters.
    pub fn text_height(&self, size: f32, lines: usize) -> f32 {
        self.line_height(size) * lines.max(1) as f32 + 2.0 * GUTTER
    }

    fn ascent(&self, size: f32) -> f32 {
        self.font.horizontal_line_metrics(size).map(|m| m.ascent).unwrap_or(size * 0.8)
    }

    fn with_glyph<R>(&self, gfx: &mut Gfx, c: char, size: f32, f: impl FnOnce(&Glyph) -> R) -> R {
        let key = (c, size.round() as u32);
        let mut cache = self.glyphs.borrow_mut();
        let g = cache.entry(key).or_insert_with(|| {
            let (m, cov) = self.font.rasterize(c, key.1 as f32);
            let tex = (m.width > 0 && m.height > 0).then(|| {
                let rgba: Vec<u8> = cov.iter().flat_map(|&a| [255, 255, 255, a]).collect();
                gfx.upload_rgba(m.width as u32, m.height as u32, &rgba)
            });
            Glyph { tex, w: m.width as f32, h: m.height as f32, xmin: m.xmin as f32, ymin: m.ymin as f32, advance: m.advance_width }
        });
        f(g)
    }

    /// Width of `text` at `size` px (without the gutter).
    pub fn width(&self, gfx: &mut Gfx, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.with_glyph(gfx, c, size, |g| g.advance)).sum()
    }

    /// Draw like an `FlxText` whose top-left is `(x, y)`; `sx`/`sy` scale about
    /// that corner. Returns the drawn width.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_scaled(&self, gfx: &mut Gfx, text: &str, x: f32, y: f32, size: f32, sx: f32, sy: f32, color: Color) -> f32 {
        let base = GUTTER + self.ascent(size);
        let mut pen = GUTTER;
        for c in text.chars() {
            let (tex, gx, gy, w, h, adv) = self.with_glyph(gfx, c, size, |g| (g.tex, pen + g.xmin, base - g.ymin - g.h, g.w, g.h, g.advance));
            if let Some(t) = tex {
                gfx.image(t, [0.0, 0.0, w, h], x + gx * sx, y + gy * sy, w * sx, h * sy, color);
            }
            pen += adv;
        }
        (pen - GUTTER) * sx
    }

    pub fn draw(&self, gfx: &mut Gfx, text: &str, x: f32, y: f32, size: f32, color: Color) -> f32 {
        self.draw_scaled(gfx, text, x, y, size, 1.0, 1.0, color)
    }
}
