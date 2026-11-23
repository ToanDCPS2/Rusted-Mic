//! The Rusted Mic logo, painted in code (no image files): a tilted, rusted
//! microphone with "RUSTED MIC" in corroded steel lettering across it, plus a
//! dim, rust-tinted backdrop for the setup screen.

use std::path::PathBuf;

pub const LOGO_W: u32 = 760;
pub const LOGO_H: u32 = 460;

// ---------- small math helpers ----------

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = clamp01((x - a) / (b - a));
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(c: [f32; 3], k: f32) -> [f32; 3] {
    [c[0] * k, c[1] * k, c[2] * k]
}

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841) ^ seed.wrapping_mul(0xcb1a_b31f);
    h ^= h >> 13;
    h = h.wrapping_mul(0x5bd1_e995);
    h ^= h >> 15;
    (h & 0xffff) as f32 / 65535.0
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash(xi, yi, seed);
    let b = hash(xi + 1, yi, seed);
    let c = hash(xi, yi + 1, seed);
    let d = hash(xi + 1, yi + 1, seed);
    let top = a + (b - a) * sx;
    let bot = c + (d - c) * sx;
    top + (bot - top) * sy
}

fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut f, mut norm) = (0.0, 0.5, 1.0, 0.0);
    for o in 0..5 {
        sum += amp * value_noise(x * f, y * f, seed + o);
        norm += amp;
        amp *= 0.5;
        f *= 2.03;
    }
    sum / norm
}

/// Rust over metal: `shade` is the lighting (0..1+), `(x, y)` the surface
/// position for the noise, `amount` biases how corroded it is.
fn rusty(base: [f32; 3], shade: f32, x: f32, y: f32, amount: f32) -> [f32; 3] {
    let n = fbm(x * 0.018, y * 0.018, 7) * 0.75 + fbm(x * 0.07, y * 0.07, 11) * 0.25;
    let r = smoothstep(0.52 - amount, 0.68 - amount, n);
    let tone = fbm(x * 0.05 + 40.0, y * 0.05, 3);
    let rust = mix([0.30, 0.11, 0.04], [0.80, 0.38, 0.12], tone);
    let rust = mix(rust, [0.55, 0.20, 0.06], smoothstep(0.6, 0.9, fbm(x * 0.2, y * 0.2, 5)) * 0.6);
    // Pitting: tiny dark specks in the rust.
    let pit = if hash((x * 0.7) as i32, (y * 0.7) as i32, 9) > 0.975 { 0.55 } else { 1.0 };
    let metal = scale(base, shade);
    let corroded = scale(rust, (0.35 + 0.75 * shade) * pit);
    mix(metal, corroded, r)
}

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: u32, h: u32) -> Self {
        Self { w: w as usize, h: h as usize, px: vec![[0.0; 4]; (w * h) as usize] }
    }

    /// Alpha-composite `c` with coverage `a` over pixel `(x, y)`.
    fn over(&mut self, x: usize, y: usize, c: [f32; 3], a: f32) {
        if a <= 0.0 {
            return;
        }
        let p = &mut self.px[y * self.w + x];
        let out_a = a + p[3] * (1.0 - a);
        if out_a <= 0.0 {
            return;
        }
        for i in 0..3 {
            p[i] = (c[i] * a + p[i] * p[3] * (1.0 - a)) / out_a;
        }
        p[3] = out_a;
    }

    fn to_rgba(&self) -> Vec<u8> {
        self.px
            .iter()
            .flat_map(|p| [(clamp01(p[0]) * 255.0) as u8, (clamp01(p[1]) * 255.0) as u8, (clamp01(p[2]) * 255.0) as u8, (clamp01(p[3]) * 255.0) as u8])
            .collect()
    }
}

// ---------- the microphone ----------

/// Signed distance to a rounded trapezoid along v: half-widths `w0` at `v0`, `w1` at `v1`.
fn sd_taper(u: f32, v: f32, v0: f32, v1: f32, w0: f32, w1: f32, round: f32) -> f32 {
    let t = clamp01((v - v0) / (v1 - v0));
    let hw = w0 + (w1 - w0) * t - round;
    let dx = u.abs() - hw;
    let dy = (v - (v0 + v1) / 2.0).abs() - ((v1 - v0) / 2.0 - round);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - round
}

fn paint_mic(c: &mut Canvas, cx: f32, cy: f32, angle_deg: f32, s: f32) {
    let (sin, cos) = angle_deg.to_radians().sin_cos();
    let light = [-0.55f32, -0.55, 0.63];
    let head_r = 100.0 * s;
    for y in 0..c.h {
        for x in 0..c.w {
            // Into mic space: u across, v down the handle (head centre at 0,0).
            let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            let u = dx * cos + dy * sin;
            let v = -dx * sin + dy * cos;
            let (wx, wy) = (x as f32, y as f32);

            // Handle: tapers away from the head, a cylinder lit from the upper left.
            let d = sd_taper(u, v, 95.0 * s, 360.0 * s, 52.0 * s, 34.0 * s, 10.0 * s);
            if d < 1.0 {
                let hw = 52.0 * s + (34.0 - 52.0) * s * clamp01((v - 95.0 * s) / (265.0 * s));
                let nx = (u / hw).clamp(-1.0, 1.0);
                let nz = (1.0 - nx * nx).max(0.0).sqrt();
                let lit = (nx * light[0] + nz * light[2]).max(0.0);
                let spec = (-(nx + 0.45).powi(2) * 40.0).exp() * 0.5;
                let shade = 0.25 + 0.8 * lit + spec;
                // A couple of grip rings.
                let ring = ((v / s - 250.0).abs() < 5.0 || (v / s - 275.0).abs() < 5.0) as i32 as f32;
                let col = rusty([0.62, 0.62, 0.66], shade * (1.0 - 0.35 * ring), wx, wy, 0.06);
                c.over(x, y, col, clamp01(0.5 - d));
            }

            // Collar between head and handle.
            let d = sd_taper(u, v, 78.0 * s, 118.0 * s, 74.0 * s, 64.0 * s, 8.0 * s);
            if d < 1.0 {
                let nx = (u / (72.0 * s)).clamp(-1.0, 1.0);
                let nz = (1.0 - nx * nx).max(0.0).sqrt();
                let lit = (nx * light[0] + nz * light[2]).max(0.0);
                let groove = (((v / s) - 98.0).abs() < 3.0) as i32 as f32;
                let col = rusty([0.70, 0.66, 0.58], (0.3 + 0.85 * lit) * (1.0 - 0.5 * groove), wx + 300.0, wy, 0.12);
                c.over(x, y, col, clamp01(0.5 - d));
            }

            // Head: a sphere wrapped in a wire grille.
            let r = (u * u + v * v).sqrt();
            let d = r - head_r;
            if d < 1.0 {
                let (nx, ny) = (u / head_r, v / head_r);
                let nz = (1.0 - nx * nx - ny * ny).max(0.0).sqrt();
                let lit = (nx * light[0] + ny * light[1] + nz * light[2]).max(0.0);
                let spec = lit.powf(24.0) * 0.6;
                // Grille: diagonal wires, foreshortened toward the rim.
                let k = 1.0 / (0.35 + 0.65 * nz);
                let (gu, gv) = (u * k / s, v * k / s);
                let cell = 13.0;
                let tri = |t: f32| {
                    let f = t.rem_euclid(1.0);
                    f.min(1.0 - f)
                };
                let (w1, w2) = (tri((gu + gv) / cell), tri((gu - gv) / cell));
                let wire = 1.0 - smoothstep(0.08, 0.16, w1.min(w2));
                let hole = 0.10 + 0.08 * lit;
                let shade = hole + (0.25 + 0.85 * lit + spec - hole) * wire;
                let mut col = rusty([0.66, 0.66, 0.70], shade, wx, wy, 0.02);
                // Rim band around the grille.
                let rim = smoothstep(head_r - 9.0 * s, head_r - 6.0 * s, r);
                col = mix(col, rusty([0.6, 0.58, 0.55], 0.25 + 0.6 * lit, wx, wy + 99.0, 0.1), rim);
                c.over(x, y, col, clamp01(0.5 - d));
            }
        }
    }
}

// ---------- lettering ----------

/// A system font for the logo's lettering, heaviest first.
pub fn system_font(candidates: &[&str]) -> Option<fontdue::Font> {
    let dir = std::env::var_os("WINDIR").map(|w| PathBuf::from(w).join("Fonts")).unwrap_or_else(|| PathBuf::from(r"C:\Windows\Fonts"));
    candidates.iter().find_map(|f| {
        let bytes = std::fs::read(dir.join(f)).ok()?;
        fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()
    })
}

/// Coverage mask of `text` at `size`, centred on `(cx, baseline)`.
#[allow(clippy::too_many_arguments)]
fn text_mask(font: &fontdue::Font, text: &str, size: f32, cx: f32, baseline: f32, w: usize, h: usize, tracking: f32) -> Vec<f32> {
    let mut mask = vec![0.0f32; w * h];
    let glyphs: Vec<_> = text.chars().map(|ch| font.rasterize(ch, size)).collect();
    let total: f32 = glyphs.iter().map(|(m, _)| m.advance_width + tracking).sum::<f32>() - tracking;
    let mut pen = cx - total / 2.0;
    for (m, cov) in &glyphs {
        let gx = (pen + m.xmin as f32).round() as i32;
        let gy = (baseline - m.ymin as f32 - m.height as f32).round() as i32;
        for j in 0..m.height {
            for i in 0..m.width {
                let (x, y) = (gx + i as i32, gy + j as i32);
                if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                    let a = cov[j * m.width + i] as f32 / 255.0;
                    let p = &mut mask[y as usize * w + x as usize];
                    *p = p.max(a);
                }
            }
        }
        pen += m.advance_width + tracking;
    }
    mask
}

/// Grow a mask by `r` px with a round brush.
fn dilate(mask: &[f32], w: usize, h: usize, r: i32) -> Vec<f32> {
    let offsets: Vec<(i32, i32)> = (-r..=r).flat_map(|dy| (-r..=r).map(move |dx| (dx, dy))).filter(|(dx, dy)| dx * dx + dy * dy <= r * r).collect();
    let mut out = vec![0.0f32; w * h];
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let mut m = 0.0f32;
            for &(dx, dy) in &offsets {
                let (sx, sy) = (x + dx, y + dy);
                if sx >= 0 && sy >= 0 && sx < w as i32 && sy < h as i32 {
                    m = m.max(mask[sy as usize * w + sx as usize]);
                    if m >= 1.0 {
                        break;
                    }
                }
            }
            out[y as usize * w + x as usize] = m;
        }
    }
    out
}

fn paint_title(c: &mut Canvas, font: &fontdue::Font, text: &str, size: f32, cx: f32, baseline: f32) {
    let (w, h) = (c.w, c.h);
    let mask = text_mask(font, text, size, cx, baseline, w, h, 3.0);
    let outline = dilate(&mask, w, h, 7);
    let shadow = dilate(&mask, w, h, 9);
    let at = |m: &[f32], x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
            0.0
        } else {
            m[y as usize * w + x as usize]
        }
    };
    // Cap height range for the vertical steel gradient.
    let top = baseline - size * 0.75;
    for y in 0..h {
        for x in 0..w {
            let (xi, yi) = (x as i32, y as i32);
            // Drop shadow, then a dark rusty outline.
            let sh = at(&shadow, xi - 6, yi - 8);
            c.over(x, y, [0.0, 0.0, 0.0], sh * 0.55);
            let o = outline[y * w + x];
            if o > 0.0 {
                let edge = rusty([0.16, 0.10, 0.07], 1.0, x as f32 * 2.0, y as f32 * 2.0, 0.05);
                c.over(x, y, edge, o);
            }
            let m = mask[y * w + x];
            if m > 0.0 {
                let t = clamp01((y as f32 - top) / (baseline - top));
                // Brushed steel: bright at the top, darker at the bottom, with a horizon line.
                let mut shade = 1.05 - 0.55 * t;
                if t > 0.48 {
                    shade -= 0.18;
                }
                shade += (value_noise(x as f32 * 0.9, y as f32 * 0.05, 21) - 0.5) * 0.12;
                // Bevel: light on upper-left edges, dark on lower-right.
                let bevel = m - at(&mask, xi - 2, yi - 2);
                let under = m - at(&mask, xi + 2, yi + 2);
                shade += 0.45 * bevel.max(0.0) - 0.4 * under.max(0.0);
                let col = rusty([0.78, 0.79, 0.83], shade, x as f32 + 1000.0, y as f32, 0.08);
                // Rust streaks running down from the letters' tops.
                let streak = smoothstep(0.55, 0.85, value_noise(x as f32 * 0.25, y as f32 * 0.012, 31)) * t;
                let col = mix(col, scale([0.55, 0.22, 0.07], 0.5 + 0.5 * shade), streak * 0.7);
                c.over(x, y, col, m);
            }
        }
    }
}

/// The logo as RGBA pixels (`LOGO_W`×`LOGO_H`).
pub fn render_logo() -> Vec<u8> {
    let mut c = Canvas::new(LOGO_W, LOGO_H);
    paint_mic(&mut c, LOGO_W as f32 * 0.5 + 30.0, 140.0, -30.0, 1.05);
    if let Some(font) = system_font(&["impact.ttf", "ariblk.ttf", "arialbd.ttf", "segoeuib.ttf"]) {
        paint_title(&mut c, &font, "RUSTED MIC", 128.0, LOGO_W as f32 / 2.0, 395.0);
    }
    c.to_rgba()
}

/// A dark, warm backdrop with a soft glow behind the logo (small; drawn scaled up).
pub fn render_backdrop(w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            let glow = (-((u - 0.5).powi(2) * 3.0 + (v - 0.33).powi(2) * 6.0) * 2.5).exp();
            let grain = fbm(x as f32 * 0.08, y as f32 * 0.08, 77);
            let base = mix([0.035, 0.025, 0.022], [0.20, 0.09, 0.035], glow * 0.85);
            let col = scale(base, 0.85 + 0.3 * grain);
            out.extend([(col[0] * 255.0) as u8, (col[1] * 255.0) as u8, (col[2] * 255.0) as u8, 255]);
        }
    }
    out
}
