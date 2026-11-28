//! Game camera logic from `PlayState.hx` plus the camera song events
//! (`FocusCamera`, `ZoomCamera`, `SetCameraBop`), and `FlxEase` curves.
//!
//! Pure math: the frontend reads [`GameCamera::scroll`] / [`GameCamera::zoom`]
//! and [`GameCamera::hud_zoom`] each frame.

use crate::constants::STEPS_PER_BEAT;

pub const DEFAULT_BOP_INTENSITY: f64 = 1.015;
pub const DEFAULT_ZOOM_RATE: f64 = 4.0;
pub const DEFAULT_CAMERA_FOLLOW_RATE: f64 = 0.04;
/// Camera bops are skipped when the HUD is already zoomed past this.
const MAX_RELATIVE_CAM_ZOOM: f64 = 1.35;

/// Port of the `FlxEase` functions used by song events.
pub fn ease(name: &str, t: f64) -> f64 {
    use std::f64::consts::PI;
    let t = t.clamp(0.0, 1.0);
    let bounce_out = |t: f64| {
        const B1: f64 = 1.0 / 2.75;
        const B2: f64 = 2.0 / 2.75;
        const B3: f64 = 1.5 / 2.75;
        const B4: f64 = 2.5 / 2.75;
        const B5: f64 = 2.25 / 2.75;
        const B6: f64 = 2.625 / 2.75;
        if t < B1 {
            7.5625 * t * t
        } else if t < B2 {
            7.5625 * (t - B3) * (t - B3) + 0.75
        } else if t < B4 {
            7.5625 * (t - B5) * (t - B5) + 0.9375
        } else {
            7.5625 * (t - B6) * (t - B6) + 0.984375
        }
    };
    let in_out = |f: &dyn Fn(f64) -> f64, t: f64| if t < 0.5 { f(t * 2.0) / 2.0 } else { 1.0 - f((1.0 - t) * 2.0) / 2.0 };
    let pow_in = |p: i32| move |t: f64| t.powi(p);
    const ELASTIC_A: f64 = 1.0;
    const ELASTIC_P: f64 = 0.4;
    let elastic_in = |t: f64| {
        if t <= 0.0 || t >= 1.0 {
            return t;
        }
        let s = ELASTIC_P / (2.0 * PI) * (1.0 / ELASTIC_A).asin();
        -(ELASTIC_A * 2f64.powf(10.0 * (t - 1.0)) * ((t - 1.0 - s) * (2.0 * PI) / ELASTIC_P).sin())
    };
    let back_in = |t: f64| t * t * (2.70158 * t - 1.70158);
    let expo_in = |t: f64| if t <= 0.0 { 0.0 } else { 2f64.powf(10.0 * (t - 1.0)) };
    let circ_in = |t: f64| -((1.0 - t * t).sqrt() - 1.0);
    let sine_in = |t: f64| -(PI / 2.0 * t).cos() + 1.0;
    let smooth = |t: f64| t * t * (3.0 - 2.0 * t);
    let smoother = |t: f64| t * t * t * (t * (t * 6.0 - 15.0) + 10.0);

    let base: Box<dyn Fn(f64) -> f64> = match name.trim_end_matches("InOut").trim_end_matches("In").trim_end_matches("Out") {
        "quad" => Box::new(pow_in(2)),
        "cube" => Box::new(pow_in(3)),
        "quart" => Box::new(pow_in(4)),
        "quint" => Box::new(pow_in(5)),
        "sine" => Box::new(sine_in),
        "circ" => Box::new(circ_in),
        "expo" => Box::new(expo_in),
        "back" => Box::new(back_in),
        "elastic" => Box::new(elastic_in),
        "bounce" => Box::new(move |t| 1.0 - bounce_out(1.0 - t)),
        "smoothStep" => return smooth_variant(name, t, smooth),
        "smootherStep" => return smooth_variant(name, t, smoother),
        _ => return t, // linear / unknown
    };
    if name.ends_with("InOut") {
        in_out(&*base, t)
    } else if name.ends_with("Out") {
        1.0 - base(1.0 - t)
    } else {
        base(t)
    }
}

fn smooth_variant(name: &str, t: f64, f: impl Fn(f64) -> f64) -> f64 {
    if name.ends_with("InOut") {
        f(t)
    } else if name.ends_with("Out") {
        2.0 * f(t / 2.0 + 0.5) - 1.0
    } else {
        2.0 * f(t / 2.0)
    }
}

#[derive(Debug, Clone)]
struct Tween {
    from: [f64; 2],
    to: [f64; 2],
    elapsed: f64,
    duration: f64,
    ease: String,
}

impl Tween {
    fn value(&self) -> [f64; 2] {
        let t = ease(&self.ease, if self.duration > 0.0 { self.elapsed / self.duration } else { 1.0 });
        [self.from[0] + (self.to[0] - self.from[0]) * t, self.from[1] + (self.to[1] - self.from[1]) * t]
    }
    fn done(&self) -> bool {
        self.elapsed >= self.duration
    }
}

/// Port of PlayState's game/HUD camera state.
#[derive(Debug, Clone)]
pub struct GameCamera {
    pub view_w: f64,
    pub view_h: f64,
    /// Point the camera follows (world coordinates of the screen centre).
    pub follow_point: [f64; 2],
    /// Top-left of the view in world space (`FlxCamera.scroll`).
    pub scroll: [f64; 2],
    pub stage_zoom: f64,
    pub current_zoom: f64,
    pub bop_multiplier: f64,
    pub bop_intensity: f64,
    pub hud_zoom: f64,
    pub hud_zoom_intensity: f64,
    pub zoom_rate: f64,
    pub zoom_rate_offset: f64,
    follow_tween: Option<Tween>,
    zoom_tween: Option<Tween>,
    /// `FlxCamera.shake`: (intensity, seconds left).
    shake: Option<(f64, f64)>,
    /// This frame's shake offset in screen pixels.
    pub shake_offset: [f64; 2],
    /// `FlxCamera.targetOffset`: added to the follow point.
    pub target_offset: [f64; 2],
    rng: u64,
}

impl GameCamera {
    pub fn new(stage_zoom: f64, view_w: f64, view_h: f64) -> Self {
        Self {
            view_w,
            view_h,
            follow_point: [view_w / 2.0, view_h / 2.0],
            scroll: [0.0, 0.0],
            stage_zoom,
            current_zoom: stage_zoom,
            bop_multiplier: 1.0,
            bop_intensity: DEFAULT_BOP_INTENSITY,
            hud_zoom: 1.0,
            hud_zoom_intensity: 0.015 * 2.0,
            zoom_rate: DEFAULT_ZOOM_RATE,
            zoom_rate_offset: 0.0,
            follow_tween: None,
            zoom_tween: None,
            shake: None,
            shake_offset: [0.0; 2],
            target_offset: [0.0; 2],
            rng: 0x9E37_79B9_7F4A_7C15,
        }
    }

    /// Effective game camera zoom (`currentCameraZoom * cameraBopMultiplier`).
    pub fn zoom(&self) -> f64 {
        self.current_zoom * self.bop_multiplier
    }

    fn target_scroll(&self) -> [f64; 2] {
        [self.follow_point[0] + self.target_offset[0] - self.view_w / 2.0, self.follow_point[1] + self.target_offset[1] - self.view_h / 2.0]
    }

    /// Jump straight to the follow point.
    pub fn snap(&mut self) {
        self.follow_tween = None;
        self.scroll = self.target_scroll();
    }

    /// `FocusCamera`: `duration_sec == None` means CLASSIC (just move the follow point).
    pub fn focus(&mut self, x: f64, y: f64, duration_sec: Option<f64>, ease_name: &str) {
        self.follow_point = [x, y];
        match duration_sec {
            None => self.follow_tween = None,
            Some(d) if d <= 0.0 => self.snap(),
            Some(d) => {
                self.follow_tween =
                    Some(Tween { from: self.scroll, to: self.target_scroll(), elapsed: 0.0, duration: d, ease: ease_name.to_owned() })
            }
        }
    }

    /// `ZoomCamera`: `direct` zooms relative to 1.0, otherwise relative to the stage zoom.
    pub fn zoom_to(&mut self, zoom: f64, direct: bool, duration_sec: f64, ease_name: &str) {
        let target = zoom * if direct { 1.0 } else { self.stage_zoom };
        if duration_sec <= 0.0 {
            self.zoom_tween = None;
            self.current_zoom = target;
        } else {
            self.zoom_tween = Some(Tween {
                from: [self.current_zoom, 0.0],
                to: [target, 0.0],
                elapsed: 0.0,
                duration: duration_sec,
                ease: ease_name.to_owned(),
            });
        }
    }

    /// `SetCameraBop`.
    pub fn set_bop(&mut self, rate: f64, offset: f64, intensity: f64) {
        self.bop_intensity = (DEFAULT_BOP_INTENSITY - 1.0) * intensity + 1.0;
        self.hud_zoom_intensity = (DEFAULT_BOP_INTENSITY - 1.0) * intensity * 2.0;
        self.zoom_rate = rate;
        self.zoom_rate_offset = offset;
    }

    /// Called on every conductor step (port of the bop in `PlayState.onStepHit`/`onBeatHit`).
    pub fn on_step(&mut self, step: i64) {
        if self.zoom_rate <= 0.0 || self.hud_zoom >= MAX_RELATIVE_CAM_ZOOM {
            return;
        }
        let period = self.zoom_rate * STEPS_PER_BEAT;
        let s = step as f64 + self.zoom_rate_offset * STEPS_PER_BEAT;
        if s.rem_euclid(period) == 0.0 {
            self.bop_multiplier = self.bop_intensity;
            self.hud_zoom += self.hud_zoom_intensity;
        }
    }

    /// `FlxCamera.shake(intensity, duration)`: jitter by up to `intensity` of the view size.
    pub fn shake(&mut self, intensity: f64, duration: f64) {
        self.shake = Some((intensity, duration));
    }

    fn rand_signed(&mut self) -> f64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }

    /// Advance `shake` and pick this frame's offset.
    pub fn update_shake(&mut self, elapsed: f64) {
        self.shake_offset = [0.0; 2];
        if let Some((i, left)) = self.shake {
            if left <= 0.0 {
                self.shake = None;
            } else {
                self.shake = Some((i, left - elapsed));
                let z = self.zoom();
                self.shake_offset = [self.rand_signed() * i * self.view_w * z, self.rand_signed() * i * self.view_h * z];
            }
        }
    }

    /// Per-frame update (decay of bops, follow lerp, tweens).
    pub fn update(&mut self, elapsed: f64) {
        self.update_shake(elapsed);
        let dt = elapsed * 60.0;
        let decay = 0.95f64.powf(dt);
        if self.zoom_rate > 0.0 {
            self.bop_multiplier = 1.0 + (self.bop_multiplier - 1.0) * decay;
            self.hud_zoom = 1.0 + (self.hud_zoom - 1.0) * decay;
        }
        if let Some(t) = &mut self.zoom_tween {
            t.elapsed += elapsed;
            self.current_zoom = t.value()[0];
            if t.done() {
                self.zoom_tween = None;
            }
        }
        self.update_follow(elapsed, DEFAULT_CAMERA_FOLLOW_RATE);
    }

    /// Move towards the follow point: its tween if one is running, else
    /// FlxCamera's LOCKON follow with lerp `rate` (per 60 fps frame).
    pub fn update_follow(&mut self, elapsed: f64, rate: f64) {
        if let Some(t) = &mut self.follow_tween {
            t.elapsed += elapsed;
            self.scroll = t.value();
            if t.done() {
                self.follow_tween = None;
            }
        } else {
            let target = self.target_scroll();
            let k = 1.0 - (1.0 - rate).powf(elapsed * 60.0);
            self.scroll[0] += (target[0] - self.scroll[0]) * k;
            self.scroll[1] += (target[1] - self.scroll[1]) * k;
        }
    }

    /// World → screen for an object with the given scroll factor (Flixel camera rules:
    /// scroll is applied first, then zoom about the screen centre).
    pub fn world_to_screen(&self, x: f64, y: f64, scroll_factor: [f64; 2]) -> [f64; 2] {
        let z = self.zoom();
        let sx = x - self.scroll[0] * scroll_factor[0];
        let sy = y - self.scroll[1] * scroll_factor[1];
        [
            self.view_w / 2.0 + (sx - self.view_w / 2.0) * z + self.shake_offset[0],
            self.view_h / 2.0 + (sy - self.view_h / 2.0) * z + self.shake_offset[1],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eases() {
        for n in ["linear", "quadIn", "quadOut", "expoInOut", "sineOut", "backIn", "elasticOut", "bounceInOut", "smoothStepIn"] {
            assert!((ease(n, 0.0)).abs() < 1e-6, "{n} at 0");
            assert!((ease(n, 1.0) - 1.0).abs() < 1e-6, "{n} at 1");
        }
        assert!(ease("quadIn", 0.5) < 0.5 && ease("quadOut", 0.5) > 0.5);
    }

    #[test]
    fn bops_and_focus() {
        let mut c = GameCamera::new(1.1, 1280.0, 720.0);
        c.on_step(16);
        assert!(c.zoom() > 1.1);
        for _ in 0..600 {
            c.update(1.0 / 60.0);
        }
        assert!((c.zoom() - 1.1).abs() < 1e-3);
        c.focus(1000.0, 500.0, Some(0.0), "linear");
        assert_eq!(c.scroll, [360.0, 140.0]);
        let p = c.world_to_screen(1000.0, 500.0, [1.0, 1.0]);
        assert!((p[0] - 640.0).abs() < 1e-9 && (p[1] - 360.0).abs() < 1e-9);
    }
}
