//! Port of the `BoyfriendCard` backing-card script: the six `BGScrollingText`
//! lines behind the DJ, and the beat glow.

use crate::assets::Assets;
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Blend, Color, Gfx, TexId};

use super::Fonts;

/// `BGScrollingText.placementOffset`.
const PLACEMENT: f32 = 20.0;
/// `FlxText` pads its field by 2 px on each side.
const GUTTER: f32 = 4.0;

struct Line {
    y: f32,
    /// 1 → `text1`, 2 → `text2`, 3 → `text3`.
    text: usize,
    width_shit: f32,
    bold: bool,
    size: f32,
    color: Color,
    speed: f32,
}


fn rgb(c: u32) -> Color {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0]
}

pub struct BfCard {
    texts: [String; 3],
    special: bool,
    lines: Vec<Line>,
    glow: Option<TexId>,
    /// When the last beat glow started (`None` = never: both sprites at alpha 1).
    glow_at: Option<f32>,
    last_beat: i64,
}

impl BfCard {
    pub fn new(gfx: &mut Gfx, assets: &Assets) -> Self {
        let player = funkin_core::vfs::read_to_string(&assets.root.join("data").join("players").join("bf.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<serde_json::Value>(t.trim_start_matches('\u{feff}')).ok());
        let text = |i: usize, fallback: &str| {
            player
                .as_ref()
                .and_then(|p| p.get("freeplayDJ")?.get(format!("text{i}"))?.as_str().map(str::to_owned))
                .unwrap_or_else(|| fallback.to_owned())
        };
        let texts = [text(1, "BOYFRIEND"), text(2, "HOT BLOODED IN MORE WAYS THAN ONE"), text(3, "PROTECT YO NUTS")];
        let l = |y, text, width_shit, bold, size, color, speed| Line { y, text, width_shit, bold, size, color, speed };
        let white = [1.0, 1.0, 1.0, 1.0];
        // Draw order: moreWays, funnyScroll, txtNuts, funnyScroll2, moreWays2, funnyScroll3.
        let lines = vec![
            l(160.0, 2, 1280.0, true, 43.0, rgb(0xFFF383), 6.8),
            l(220.0, 1, 640.0, false, 60.0, rgb(0xFF9963), -3.8),
            l(285.0, 3, 640.0, true, 43.0, white, 3.5),
            l(335.0, 1, 640.0, false, 60.0, rgb(0xFF9963), -3.8),
            l(397.0, 2, 1280.0, true, 43.0, rgb(0xFFF383), 6.8),
            // orangeBackShit.y + 10
            l(450.0, 1, 640.0, false, 60.0, rgb(0xFEA400), -3.8),
        ];
        let glow = gfx.load_texture(&assets.image_path("default:freeplay/beatglow")).ok();
        Self { texts, special: false, lines, glow, glow_at: None, last_beat: i64::MIN }
    }

    /// `onCapsuleSelected`: the sserafim level swaps every line to "SPECIAL".
    pub fn set_level(&mut self, level: Option<&str>) {
        self.special = level == Some("sserafim");
    }

    fn line_text(&self, l: &Line) -> &str {
        match (self.special, l.text) {
            (true, 1) => "SPECIAL SPECIAL",
            (true, _) => "SPECIAL",
            (false, i) => &self.texts[i - 1],
        }
    }

    /// `beatHit` with the conductor at `music_ms` and `bpm`.
    pub fn update(&mut self, time: f32, music_ms: f64, bpm: f64, playing: bool) {
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

    /// Draw the texts and glow; `t` = seconds since the scene began.
    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts, t: f32) {
        for l in &self.lines {
            let font: Option<&TtfFont> = if l.bold { fonts.five_by_seven_bold.as_ref().or(fonts.five_by_seven.as_ref()) } else { fonts.five_by_seven.as_ref() };
            let Some(font) = font else { continue };
            let text = self.line_text(l);
            let fw = font.width(gfx, text, l.size) + GUTTER;
            let step = fw + PLACEMENT;
            let n = (l.width_shit / fw).ceil() as usize + 1;
            let period = n as f32 * step;
            let shift = -l.speed * 60.0 * t;
            for i in 0..n {
                let base = i as f32 * step + shift;
                let x = if l.speed > 0.0 {
                    (base + fw).rem_euclid(period) - fw
                } else {
                    2.0 * fw - (2.0 * fw - base).rem_euclid(period)
                };
                font.draw(gfx, text, x, l.y, l.size, l.color);
            }
        }
        if let Some(tex) = self.glow {
            let (dark, bright) = match self.glow_at {
                None => (1.0, 1.0),
                Some(at) => {
                    let e = t - at;
                    let q = |d: f32| funkin_core::camera::ease("quartOut", (e / d).clamp(0.0, 1.0) as f64) as f32;
                    (0.6 * q(18.0 / 24.0), 0.8 * (1.0 - q(16.0 / 24.0)))
                }
            };
            let (x, y) = (-300.0, 330.0);
            gfx.set_blend(Blend::Multiply);
            gfx.sprite(tex, x, y, 1.0, [1.0, 1.0, 1.0, dark]);
            gfx.set_blend(Blend::Add);
            gfx.sprite(tex, x, y, 1.0, [1.0, 1.0, 1.0, bright]);
            gfx.set_blend(Blend::Normal);
        }
    }
}
