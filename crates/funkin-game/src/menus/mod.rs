//! Menu screens: title (`TitleState`), main menu (`MainMenuState`) and
//! freeplay (`FreeplayState`), plus small helpers they share (tweens,
//! menu music with fades, flashes).

mod album_roll;
mod bf_card;
pub mod freeplay;
pub mod main_menu;
pub mod options;
pub mod title;

use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use funkin_core::camera::{ease, GameCamera};
use funkin_core::stage::AnimationData;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::text::Font;
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Gfx, VIEW_H, VIEW_W};

/// The fonts menus draw with (`AtlasFont.BOLD`, `DEFAULT`, `FREEPLAY_CLEAR`).
pub struct Fonts {
    pub bold: Font,
    pub default: Font,
    pub clear: Font,
    /// `VCR OSD Mono` and `5by7`, if the import extracted them.
    pub vcr: Option<TtfFont>,
    pub five_by_seven: Option<TtfFont>,
    pub five_by_seven_bold: Option<TtfFont>,
}

impl Fonts {
    pub fn set_time(&self, t: f32) {
        self.bold.set_time(t);
        self.default.set_time(t);
        self.clear.set_time(t);
    }
}

/// What a menu wants the app to do next.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuAction {
    None,
    ToMainMenu,
    ToTitle,
    /// After the fade-out.
    ToTitleNow,
    OpenFreeplay,
    CloseFreeplay,
    /// Fade out, then open the options menu.
    OpenOptions,
    OpenOptionsNow,
    /// Fade out of options back to the main menu.
    CloseOptions,
    /// Save data was cleared: restart from the title (`InitState`).
    Restart,
    Play(funkin_core::state::PlayRequest),
    Quit,
}

/// A one-shot `FlxTween` on a single value.
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    pub from: f32,
    pub to: f32,
    pub start: f32,
    pub dur: f32,
    pub ease: &'static str,
}

impl Tween {
    pub fn new(from: f32, to: f32, start: f32, dur: f32, ease: &'static str) -> Self {
        Self { from, to, start, dur, ease }
    }
    pub fn hold(v: f32) -> Self {
        Self::new(v, v, 0.0, 0.0, "linear")
    }
    pub fn at(&self, t: f32) -> f32 {
        if self.dur <= 0.0 || t >= self.start + self.dur {
            return self.to;
        }
        if t <= self.start {
            return self.from;
        }
        let k = ease(self.ease, ((t - self.start) / self.dur) as f64) as f32;
        self.from + (self.to - self.from) * k
    }
}

/// `MathUtil.smoothLerpPrecision(base, target, dt, duration, 1/100)`.
pub fn smooth_lerp(base: f32, target: f32, dt: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return target;
    }
    base + (target - base) * (1.0 - 0.01f32.powf(dt / duration))
}

/// Flixel `followLerp` (a per-60fps-frame factor), made frame-rate independent.
pub fn follow_lerp(base: f32, target: f32, dt: f32, lerp: f32) -> f32 {
    base + (target - base) * (1.0 - (1.0 - lerp).powf(dt * 60.0))
}

/// `FlxFlicker`: visible on/off every `interval` seconds while running.
pub fn flicker_visible(elapsed: f32, interval: f32) -> bool {
    (elapsed / interval) as i64 % 2 == 1
}

/// A camera at the given scroll with no zoom, for screen-space actors.
pub fn screen_camera(scroll_x: f32, scroll_y: f32) -> GameCamera {
    let mut c = GameCamera::new(1.0, VIEW_W as f64, VIEW_H as f64);
    c.scroll = [scroll_x as f64, scroll_y as f64];
    c
}

/// `FlxCamera.flash`: a white overlay fading out over `dur` seconds.
pub fn draw_flash(gfx: &mut Gfx, since: f32, dur: f32) {
    if since >= 0.0 && since < dur {
        gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [1.0, 1.0, 1.0, 1.0 - since / dur]);
    }
}

/// Animation data for a Sparrow prefix (or Animate frame label).
pub fn anim(name: &str, prefix: &str, fps: f64, looped: bool, indices: &[i32]) -> AnimationData {
    serde_json::from_value(serde_json::json!({
        "name": name, "prefix": prefix, "frameRate": fps, "looped": looped, "frameIndices": indices
    }))
    .expect("static animation data")
}

/// Small xorshift PRNG for `FlxG.random` picks.
pub fn random() -> f32 {
    thread_local!(static S: Cell<u64> = Cell::new(
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1
    ));
    S.with(|s| {
        let mut x = s.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        s.set(x);
        (x >> 40) as f32 / (1u64 << 24) as f32
    })
}

/// `FlxG.sound.music`: one looping track with volume fades.
pub struct Music {
    pub id: String,
    track: Option<Arc<Track>>,
    /// Loop region (ms) and the position playback started at.
    loop_ms: (f64, f64),
    volume: f32,
    fade: Option<(f32, f32, Instant, f32)>,
    /// Song-preview envelope: fade in/out over 2 s at each end of the loop.
    preview: Option<f32>,
    cache: HashMap<String, Arc<Track>>,
}

impl Default for Music {
    fn default() -> Self {
        Self { id: String::new(), track: None, loop_ms: (0.0, 0.0), volume: 0.0, fade: None, preview: None, cache: HashMap::new() }
    }
}

impl Music {
    pub fn playing(&self) -> bool {
        self.track.is_some()
    }

    /// `FunkinSound.playMusic(id)` for a menu track in `music/<id>/<id>.ogg`,
    /// looped whole. Returns false if it could not be loaded.
    pub fn play_menu(&mut self, audio: &Audio, assets: &Assets, id: &str, restart: bool) -> bool {
        if !restart && self.id == id && self.track.is_some() {
            return true;
        }
        let track = match self.cache.get(id) {
            Some(t) => t.clone(),
            None => match Pcm::load_ogg(&assets.music_path(id)) {
                Ok(p) => {
                    let t = Track::new(p);
                    self.cache.insert(id.to_owned(), t.clone());
                    t
                }
                Err(e) => {
                    log::warn!("music: {e}");
                    return false;
                }
            },
        };
        let len = track.pcm.duration_ms();
        self.start(audio, id, track, 0.0, (0.0, len));
        self.preview = None;
        true
    }

    /// Play `track` from `start` ms, looping `[a, b)`, at volume 0.
    pub fn start(&mut self, audio: &Audio, id: &str, track: Arc<Track>, start: f64, (a, b): (f64, f64)) {
        track.set_volume(0.0);
        audio.play_looped(vec![track.clone()], start, a, b);
        self.id = id.to_owned();
        self.track = Some(track);
        self.loop_ms = (a, b);
        self.volume = 0.0;
        self.fade = None;
        self.preview = None;
    }

    /// A freeplay song preview: loops `[a, b)` with 2 s fades at either end.
    pub fn start_preview(&mut self, audio: &Audio, id: &str, track: Arc<Track>, (a, b): (f64, f64), peak: f32) {
        self.start(audio, id, track, a, (a, b));
        self.preview = Some(peak);
    }

    /// Loop `main` with extra synced layers (whose volumes the caller controls).
    pub fn start_layered(&mut self, audio: &Audio, id: &str, main: Arc<Track>, layers: Vec<Arc<Track>>) {
        let len = main.pcm.duration_ms();
        main.set_volume(0.0);
        let mut all = vec![main.clone()];
        all.extend(layers);
        audio.play_looped(all, 0.0, 0.0, len);
        self.id = id.to_owned();
        self.track = Some(main);
        self.loop_ms = (0.0, len);
        self.volume = 0.0;
        self.fade = None;
        self.preview = None;
    }

    /// Length of the loop region (ms).
    pub fn loop_len_ms(&self) -> f64 {
        self.loop_ms.1 - self.loop_ms.0
    }

    /// The current volume (before any fade this frame).
    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn stop(&mut self, audio: &Audio) {
        if self.track.take().is_some() {
            audio.stop();
        }
        self.id.clear();
        self.preview = None;
        self.fade = None;
    }

    /// `fadeIn(duration, from, to)`.
    pub fn fade(&mut self, from: f32, to: f32, dur: f32) {
        self.volume = from;
        self.fade = Some((from, to, Instant::now(), dur));
    }

    /// The music position (ms, counting up through loops) audible now.
    pub fn time_ms(&self, audio: &Audio) -> f64 {
        audio.song_time_at(Instant::now())
    }

    /// Apply fades; call once per frame.
    pub fn update(&mut self, audio: &Audio) {
        let Some(t) = &self.track else { return };
        if let Some((from, to, at, dur)) = self.fade {
            let k = (at.elapsed().as_secs_f32() / dur.max(1e-3)).min(1.0);
            self.volume = from + (to - from) * k;
            if k >= 1.0 {
                self.fade = None;
            }
        }
        let mut v = self.volume;
        if let Some(peak) = self.preview {
            let (a, b) = self.loop_ms;
            let len = (b - a).max(1.0);
            let local = (self.time_ms(audio) - a).rem_euclid(len);
            let fade_in = (local / 2000.0).min(1.0) as f32;
            let fade_out = ((len - local) / 2000.0).min(1.0) as f32;
            v = peak * fade_in.min(fade_out);
        }
        t.set_volume(v);
    }
}
