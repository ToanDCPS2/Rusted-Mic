//! Port of `StickerSubState`: leaving a song covers the screen in stickers
//! (`regenStickers`), switches state underneath, then peels them off again
//! (`degenStickers`) over the new state.

use std::path::PathBuf;
use std::sync::Arc;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::gfx::{Gfx, TexId};
use crate::menus::random;

const WIDTH: f32 = 1280.0;
const HEIGHT: f32 = 720.0;

struct Sticker {
    tex: TexId,
    w: f32,
    h: f32,
    x: f32,
    y: f32,
    angle: f32,
    scale: f32,
    timing: f32,
    visible: bool,
    /// Delay (s) after showing before the scale "pop" (`frameTimer / 24`).
    pop: Option<f32>,
}

#[derive(PartialEq)]
enum Phase {
    /// Stickers going on; the state switch happens when the last one pops.
    In,
    /// Switched: waiting for the caller to swap states.
    Switch,
    /// Stickers coming off over the new state.
    Out,
}

pub struct Stickers {
    stickers: Vec<Sticker>,
    sounds: Vec<Arc<Pcm>>,
    phase: Phase,
    t: f32,
}

fn rand_range(lo: f32, hi: f32) -> f32 {
    lo + random() * (hi - lo)
}

fn rand_int(lo: i32, hi: i32) -> i32 {
    (lo + (random() * (hi - lo + 1) as f32) as i32).min(hi)
}

fn pick<T>(v: &[T]) -> Option<&T> {
    if v.is_empty() {
        None
    } else {
        v.get(((random() * v.len() as f32) as usize).min(v.len() - 1))
    }
}

/// The sticker pack for a song: the chart's `stickerPack`, else the owning
/// playable character's (`PlayerRegistry.getCharacterOwnerId`), else `default`.
pub fn resolve_pack(assets: &Assets, chart_pack: Option<&str>, player: &str) -> String {
    if let Some(p) = chart_pack.filter(|p| !p.is_empty()) {
        return p.to_owned();
    }
    let dir = assets.root().join("data").join("players");
    let mut owner_pack = None;
    let mut default_pack = None;
    if let Ok(entries) = funkin_core::vfs::read_dir(&dir) {
        for e in entries.iter().filter(|e| e.name.ends_with(".json")) {
            let Ok(s) = funkin_core::vfs::read_to_string(&e.path) else { continue };
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else { continue };
            let pack = v.get("stickerPack").and_then(|p| p.as_str()).unwrap_or("default").to_owned();
            let owns = v.get("ownedChars").and_then(|o| o.as_array()).is_some_and(|o| o.iter().any(|c| c.as_str() == Some(player)));
            if owns {
                owner_pack = Some(pack);
            } else if e.name == "bf.json" {
                default_pack = Some(pack);
            }
        }
    }
    owner_pack.or(default_pack).unwrap_or_else(|| "default".into())
}

fn pack_stickers(assets: &Assets, id: &str) -> Vec<String> {
    let read = |id: &str| -> Option<Vec<String>> {
        let p = assets.root().join("data").join("stickerpacks").join(format!("{id}.json"));
        let v: serde_json::Value = serde_json::from_str(&funkin_core::vfs::read_to_string(&p).ok()?).ok()?;
        let list: Vec<String> = v.get("stickers")?.as_array()?.iter().filter_map(|s| s.as_str().map(str::to_owned)).collect();
        (!list.is_empty()).then_some(list)
    };
    read(id).or_else(|| read("default")).unwrap_or_default()
}

/// One random folder of `shared/sounds/stickersounds/`, all its sounds.
fn load_sounds(assets: &Assets) -> Vec<Arc<Pcm>> {
    let dir: PathBuf = assets.root().join("shared").join("sounds").join("stickersounds");
    let Ok(sets) = funkin_core::vfs::read_dir(&dir) else { return Vec::new() };
    let mut sets: Vec<_> = sets.into_iter().filter(|e| e.is_dir).collect();
    sets.sort_by(|a, b| a.name.cmp(&b.name));
    let Some(set) = pick(&sets) else { return Vec::new() };
    let Ok(files) = funkin_core::vfs::read_dir(&set.path) else { return Vec::new() };
    let mut files: Vec<_> = files.into_iter().filter(|e| e.name.ends_with(".ogg")).collect();
    files.sort_by(|a, b| a.name.cmp(&b.name));
    files.iter().filter_map(|f| Pcm::load_ogg(&f.path).ok().map(Arc::new)).collect()
}

impl Stickers {
    /// Start covering the screen with stickers from `pack`.
    pub fn new(gfx: &mut Gfx, assets: &Assets, pack: &str) -> Option<Self> {
        let paths = pack_stickers(assets, pack);
        let mut textures = Vec::new();
        for p in &paths {
            match assets.texture(gfx, p) {
                Ok(t) => {
                    let (w, h) = gfx.texture_size(t);
                    textures.push((t, w, h));
                }
                Err(e) => log::warn!("sticker {p}: {e}"),
            }
        }
        if textures.is_empty() {
            return None;
        }
        let new_sticker = |&(tex, w, h): &(TexId, f32, f32)| Sticker {
            tex,
            w,
            h,
            x: 0.0,
            y: 0.0,
            angle: 0.0,
            scale: 1.0,
            timing: 0.0,
            visible: false,
            pop: None,
        };
        let mut stickers = Vec::new();
        let (mut x, mut y) = (-100.0f32, -100.0f32);
        while x <= WIDTH {
            let mut s = new_sticker(pick(&textures).unwrap());
            s.x = x;
            s.y = y;
            x += (s.w * 0.5).max(1.0);
            if x >= WIDTH && y <= HEIGHT {
                x = -100.0;
                y += rand_range(70.0, 120.0);
            }
            s.angle = rand_int(-60, 70) as f32;
            stickers.push(s);
        }
        // FlxG.random.shuffle
        for i in (1..stickers.len()).rev() {
            let j = ((random() * (i + 1) as f32) as usize).min(i);
            stickers.swap(i, j);
        }
        let mut last = new_sticker(pick(&textures).unwrap());
        last.x = ((WIDTH - last.w) / 2.0).floor();
        last.y = ((HEIGHT - last.h) / 2.0).floor();
        stickers.push(last);
        let n = stickers.len() as f32;
        for (i, s) in stickers.iter_mut().enumerate() {
            s.timing = i as f32 / n * 0.9;
        }
        Some(Self { stickers, sounds: load_sounds(assets), phase: Phase::In, t: 0.0 })
    }

    fn click(&self, audio: &Audio) {
        if let Some(s) = pick(&self.sounds) {
            audio.play_sfx(s.clone(), 1.0);
        }
    }

    /// Stickers are still going on: the state underneath takes no input.
    pub fn blocking(&self) -> bool {
        self.phase != Phase::Out
    }

    /// Advance. Returns true once, when the state underneath should switch
    /// (then call [`Stickers::switched`]); [`Stickers::done`] says when it's over.
    pub fn update(&mut self, dt: f32, audio: &Audio) -> bool {
        if self.phase == Phase::Switch {
            return false;
        }
        self.t += dt;
        let t = self.t;
        let n = self.stickers.len();
        let mut clicks = 0;
        let mut switch = false;
        match self.phase {
            Phase::In => {
                for (i, s) in self.stickers.iter_mut().enumerate() {
                    if !s.visible && t >= s.timing {
                        s.visible = true;
                        clicks += 1;
                        let frames = if i == n - 1 { 2 } else { rand_int(0, 2) };
                        s.pop = Some(s.timing + frames as f32 / 24.0);
                    }
                    if let Some(at) = s.pop {
                        if t >= at {
                            s.pop = None;
                            s.scale = rand_range(0.97, 1.02);
                            if i == n - 1 {
                                switch = true;
                            }
                        }
                    }
                }
            }
            Phase::Out => {
                for s in self.stickers.iter_mut() {
                    if s.visible && t >= s.timing {
                        s.visible = false;
                        clicks += 1;
                    }
                }
            }
            Phase::Switch => {}
        }
        for _ in 0..clicks.min(3) {
            self.click(audio);
        }
        if switch {
            self.phase = Phase::Switch;
        }
        switch
    }

    /// The new state is in place: start peeling the stickers off.
    pub fn switched(&mut self) {
        self.phase = Phase::Out;
        self.t = 0.0;
    }

    pub fn done(&self) -> bool {
        self.phase == Phase::Out && self.stickers.iter().all(|s| !s.visible)
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        for s in self.stickers.iter().filter(|s| s.visible) {
            let (cx, cy) = (s.x + s.w / 2.0, s.y + s.h / 2.0);
            let (hw, hh) = (s.w / 2.0 * s.scale, s.h / 2.0 * s.scale);
            let (sin, cos) = s.angle.to_radians().sin_cos();
            let p = |dx: f32, dy: f32| [cx + dx * cos - dy * sin, cy + dx * sin + dy * cos];
            gfx.quad(s.tex, [0.0, 0.0, s.w, s.h], [p(-hw, -hh), p(hw, -hh), p(hw, hh), p(-hw, hh)], [1.0, 1.0, 1.0, 1.0]);
        }
    }
}
