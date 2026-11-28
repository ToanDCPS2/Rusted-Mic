//! Port of the `PicoCard` backing-card script: Pico's scrolling weapon
//! strips, the blue bar and the beat glow (the confirm animation is not ported).

use crate::assets::Assets;
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{Blend, Gfx, TexId, VIEW_W};

/// `FlxBackdrop` repeating on X.
struct Backdrop {
    tex: TexId,
    y: f32,
    spacing: f32,
    velocity: f32,
    alpha: f32,
    flip: bool,
}

pub struct PicoCard {
    back: Option<Backdrop>,
    lower: Option<Backdrop>,
    middle: Option<Backdrop>,
    blue_bar: Option<TexId>,
    top: Option<(Atlas, Vec<(&'static str, Anim)>)>,
    /// (animation index, started at).
    top_anim: (usize, f32),
    glow: Option<TexId>,
    glow_at: Option<f32>,
    last_beat: i64,
}

fn tile(gfx: &mut Gfx, b: &Backdrop, t: f32, mul: f32) {
    let (w, h) = gfx.texture_size(b.tex);
    let period = w + b.spacing;
    let mut x = (b.velocity * t).rem_euclid(period) - period;
    let src = if b.flip { [w, 0.0, -w, h] } else { [0.0, 0.0, w, h] };
    while x < VIEW_W {
        gfx.image(b.tex, src, x, b.y, w, h, [1.0, 1.0, 1.0, b.alpha * mul]);
        x += period;
    }
}

impl PicoCard {
    pub fn new(gfx: &mut Gfx, assets: &Assets) -> Self {
        let tex = |gfx: &mut Gfx, p: &str| gfx.load_texture(&assets.image_path(&format!("default:freeplay/backingCards/pico/{p}"))).ok();
        let lower = tex(gfx, "lowerLoop");
        let back = lower.map(|tex| Backdrop { tex, y: 200.0, spacing: 20.0, velocity: 110.0, alpha: 0.39, flip: true });
        let lower = lower.map(|tex| Backdrop { tex, y: 406.0, spacing: 20.0, velocity: -110.0, alpha: 1.0, flip: false });
        let middle = tex(gfx, "middleLoop").map(|tex| Backdrop { tex, y: 346.0, spacing: 15.0, velocity: 220.0, alpha: 1.0, flip: false });
        let top = assets.atlas(gfx, "default:freeplay/backingCards/pico/topLoop").ok().map(|a| {
            let anims = ["base", "uzi info", "sniper info", "rifle info", "rocket launcher info"]
                .into_iter()
                .map(|n| (n, a.prefix(n)))
                .collect();
            (a, anims)
        });
        Self {
            back,
            lower,
            middle,
            blue_bar: tex(gfx, "blueBar"),
            top,
            top_anim: (0, 0.0),
            glow: tex(gfx, "glow"),
            glow_at: None,
            last_beat: i64::MIN,
        }
    }

    /// `beatHit` and the top strip's weapon labels.
    pub fn update(&mut self, time: f32, music_ms: f64, bpm: f64, playing: bool) {
        if let Some((a, anims)) = &self.top {
            let (i, at) = self.top_anim;
            let cur = &anims[i].1;
            if time - at >= cur.duration(24.0) {
                let w = a.frames.first().map(|f| f.frame_w).unwrap_or(2109.0);
                let progress = (-220.0 * time).rem_euclid(w + 20.0);
                let want = match progress {
                    p if (500.0..=700.0).contains(&p) => Some("sniper info"),
                    p if (700.0..=1300.0).contains(&p) => Some("rifle info"),
                    p if (1450.0..=2000.0).contains(&p) => Some("rocket launcher info"),
                    p if (0.0..=300.0).contains(&p) => Some("uzi info"),
                    _ => None,
                };
                if let Some(n) = want.and_then(|w| anims.iter().position(|(k, _)| *k == w)) {
                    if n != i {
                        self.top_anim = (n, time);
                    }
                }
            }
        }
        if !playing || bpm <= 0.0 {
            return;
        }
        let beat = (music_ms.max(0.0) / (60000.0 / bpm)).floor() as i64;
        if beat != self.last_beat {
            self.last_beat = beat;
            let freq = [1, 2, 4, 8][((bpm / 140.0).floor() as usize).min(3)];
            if beat % freq == 0 {
                self.glow_at = Some(time);
            }
        }
    }

    /// Draw the strips (shown once the intro is done); `t` = scene time.
    pub fn draw(&self, gfx: &mut Gfx, t: f32) {
        if let Some(b) = &self.back {
            tile(gfx, b, t, 1.0);
        }
        if let Some(b) = &self.lower {
            tile(gfx, b, t, 1.0);
        }
        if let Some(bar) = self.blue_bar {
            let (w, h) = gfx.texture_size(bar);
            gfx.set_blend(Blend::Multiply);
            gfx.image(bar, [0.0, 0.0, w, h], 0.0, 239.0, w, h, [1.0, 1.0, 1.0, 0.4]);
            gfx.set_blend(Blend::Normal);
        }
        if let Some((a, anims)) = &self.top {
            let (i, at) = self.top_anim;
            if let Some(f) = anims[i].1.frame_at(t - at, 24.0, false).or_else(|| anims[0].1.frames.first().copied()) {
                let w = a.frames[f].frame_w;
                let period = w + 20.0;
                let mut x = (-220.0 * t).rem_euclid(period) - period;
                while x < VIEW_W {
                    a.draw(gfx, f, x, 80.0, 1.0, [1.0; 4]);
                    x += period;
                }
            }
        }
        if let Some(b) = &self.middle {
            tile(gfx, b, t, 1.0);
        }
        if let Some(tex) = self.glow {
            let (dark, bright) = match self.glow_at {
                None => (1.0, 1.0),
                Some(at) => {
                    let e = t - at;
                    let q = |d: f32| funkin_core::camera::ease("quartOut", (e / d).clamp(0.0, 1.0) as f64) as f32;
                    (q(18.0 / 24.0), 1.0 - q(16.0 / 24.0))
                }
            };
            gfx.set_blend(Blend::Multiply);
            gfx.sprite(tex, -300.0, 330.0, 1.0, [1.0, 1.0, 1.0, dark]);
            gfx.set_blend(Blend::Add);
            gfx.sprite(tex, -300.0, 330.0, 1.0, [1.0, 1.0, 1.0, bright]);
            gfx.set_blend(Blend::Normal);
        }
    }
}
