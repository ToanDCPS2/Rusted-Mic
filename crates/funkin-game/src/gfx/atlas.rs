//! Sparrow v2 texture atlases (`.png` + `.xml`), as used by HaxeFlixel's
//! `FlxAtlasFrames.fromSparrow`, plus prefix-based animations.

use std::path::Path;

use super::{Affine, Color, Gfx, TexId};

#[derive(Debug, Clone)]
pub struct Frame {
    pub name: String,
    /// Region in the texture.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Trim offset (Sparrow's `frameX/frameY` are ≤ 0).
    pub frame_x: f32,
    pub frame_y: f32,
    /// Untrimmed size.
    pub frame_w: f32,
    pub frame_h: f32,
    pub rotated: bool,
}

pub struct Atlas {
    pub texture: TexId,
    pub frames: Vec<Frame>,
}

impl Atlas {
    /// Load `<path>.png` + `<path>.xml`.
    pub fn load(gfx: &mut Gfx, base: &Path) -> Result<Self, String> {
        Self::load_filtered(gfx, base, true)
    }

    pub fn load_filtered(gfx: &mut Gfx, base: &Path, smooth: bool) -> Result<Self, String> {
        let xml_path = base.with_extension("xml");
        let texture = gfx.load_texture_filtered(&base.with_extension("png"), smooth)?;
        let text = funkin_core::vfs::read_to_string(&xml_path)
            .map_err(|e| format!("{}: {e}", xml_path.display()))?;
        let frames = parse_sparrow(&text);
        if frames.is_empty() {
            return Err(format!("{}: no SubTexture entries", xml_path.display()));
        }
        Ok(Self { texture, frames })
    }

    /// A plain image as a one-frame atlas.
    pub fn single(texture: TexId, w: f32, h: f32) -> Self {
        let frame = Frame { name: String::new(), x: 0.0, y: 0.0, w, h, frame_x: 0.0, frame_y: 0.0, frame_w: w, frame_h: h, rotated: false };
        Self { texture, frames: vec![frame] }
    }

    /// Frames whose name starts with `prefix`, in name order (Flixel's `addByPrefix`).
    pub fn prefix(&self, prefix: &str) -> Anim {
        let mut idx: Vec<usize> = (0..self.frames.len())
            .filter(|&i| self.frames[i].name.starts_with(prefix))
            .collect();
        idx.sort_by(|&a, &b| self.frames[a].name.cmp(&self.frames[b].name));
        Anim { frames: idx }
    }

    /// Draw a frame with its untrimmed top-left at `(x, y)`.
    pub fn draw(&self, gfx: &mut Gfx, frame: usize, x: f32, y: f32, scale: f32, color: Color) {
        let f = &self.frames[frame];
        let dx = x - f.frame_x * scale;
        let dy = y - f.frame_y * scale;
        let (dw, dh) = (f.w * scale, f.h * scale);
        let dst = [[dx, dy], [dx + dw, dy], [dx + dw, dy + dh], [dx, dy + dh]];
        draw_region(gfx, self.texture, f, dst, color);
    }

    /// Frames for `addByIndices(prefix, indices)`: the frame whose number after
    /// `prefix` equals each index.
    pub fn indices(&self, prefix: &str, indices: &[i32]) -> Anim {
        let numbered: Vec<(i32, usize)> = (0..self.frames.len())
            .filter_map(|i| {
                let rest = self.frames[i].name.strip_prefix(prefix)?;
                let digits: String = rest.chars().filter(|c| c.is_ascii_digit()).collect();
                digits.parse().ok().map(|n| (n, i))
            })
            .collect();
        Anim {
            frames: indices
                .iter()
                .filter_map(|&want| numbered.iter().find(|(n, _)| *n == want).map(|&(_, i)| i))
                .collect(),
        }
    }

    /// Draw a frame through `m`, which maps the frame's untrimmed local space
    /// (`0..frame_w`, `0..frame_h`) to the canvas. `flip_x` mirrors within `frame_w`.
    pub fn draw_xf(&self, gfx: &mut Gfx, frame: usize, m: &Affine, flip_x: bool, color: Color) {
        let f = &self.frames[frame];
        let (x0, y0) = (-f.frame_x, -f.frame_y);
        let (x1, y1) = (x0 + f.w, y0 + f.h);
        let mut corners = [[x0, y0], [x1, y0], [x1, y1], [x0, y1]];
        if flip_x {
            for c in &mut corners {
                c[0] = f.frame_w - c[0];
            }
        }
        draw_region(gfx, self.texture, f, corners.map(|c| m.apply(c)), color);
    }

    /// Draw a frame centred on `(cx, cy)`.
    pub fn draw_centered(
        &self,
        gfx: &mut Gfx,
        frame: usize,
        cx: f32,
        cy: f32,
        scale: f32,
        color: Color,
    ) {
        let f = &self.frames[frame];
        self.draw(
            gfx,
            frame,
            cx - f.frame_w * scale / 2.0,
            cy - f.frame_h * scale / 2.0,
            scale,
            color,
        );
    }
}

/// Draw a frame's texture region onto `dst` (TL, TR, BR, BL of the unrotated image).
pub fn draw_region(gfx: &mut Gfx, texture: TexId, f: &Frame, dst: [[f32; 2]; 4], color: Color) {
    if f.rotated {
        // Stored rotated 90° clockwise: region is h×w in the texture.
        let (tw, th) = gfx.texture_size(texture);
        let (u0, v0) = (f.x / tw, f.y / th);
        let (u1, v1) = ((f.x + f.h) / tw, (f.y + f.w) / th);
        gfx.quad_uv(texture, [[u1, v0], [u1, v1], [u0, v1], [u0, v0]], dst, color);
    } else {
        gfx.quad(texture, [f.x, f.y, f.w, f.h], dst, color);
    }
}

/// Lenient Sparrow parser. Official atlases contain attribute values that a
/// strict XML parser rejects (e.g. `name="<0000"` in `alphabet.xml`), which
/// Haxe's parser accepts, so this scans `<SubTexture .../>` tags directly.
fn parse_sparrow(text: &str) -> Vec<Frame> {
    let mut frames = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<SubTexture") {
        rest = &rest[start + "<SubTexture".len()..];
        let mut attrs: Vec<(&str, String)> = Vec::new();
        let mut s = rest;
        // Attributes: key="value" pairs until `/>` or `>` outside quotes.
        loop {
            s = s.trim_start();
            if s.starts_with("/>") || s.starts_with('>') || s.is_empty() {
                break;
            }
            let Some(eq) = s.find('=') else { break };
            let key = s[..eq].trim();
            let after = s[eq + 1..].trim_start();
            let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else {
                break;
            };
            let body = &after[1..];
            let Some(end) = body.find(q) else { break };
            attrs.push((key, unescape(&body[..end])));
            s = &body[end + 1..];
        }
        rest = s;
        let get = |k: &str| attrs.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str());
        let num = |k: &str| get(k).and_then(|v| v.trim().parse::<f32>().ok());
        let rotated = get("rotated") == Some("true");
        // `width`/`height` give the region as stored; a rotated frame is h x w upright.
        let (sw, sh) = (num("width").unwrap_or(0.0), num("height").unwrap_or(0.0));
        let (w, h) = if rotated { (sh, sw) } else { (sw, sh) };
        frames.push(Frame {
            name: get("name").unwrap_or_default().to_owned(),
            x: num("x").unwrap_or(0.0),
            y: num("y").unwrap_or(0.0),
            w,
            h,
            frame_x: num("frameX").unwrap_or(0.0),
            frame_y: num("frameY").unwrap_or(0.0),
            frame_w: num("frameWidth").unwrap_or(w),
            frame_h: num("frameHeight").unwrap_or(h),
            rotated,
        });
    }
    frames
}

fn unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// A list of frame indices played at a fixed frame rate.
#[derive(Debug, Clone, Default)]
pub struct Anim {
    pub frames: Vec<usize>,
}

impl Anim {
    /// Frame for `elapsed` seconds into the animation (clamps to the last frame).
    pub fn frame_at(&self, elapsed: f32, fps: f32, looped: bool) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        let i = (elapsed.max(0.0) * fps) as usize;
        let i = if looped {
            i % self.frames.len()
        } else {
            i.min(self.frames.len() - 1)
        };
        Some(self.frames[i])
    }

    pub fn duration(&self, fps: f32) -> f32 {
        self.frames.len() as f32 / fps
    }
}
