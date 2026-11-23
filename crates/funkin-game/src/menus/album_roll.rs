//! Port of `AlbumRoll` and `DifficultyStars`: the album art (an Animate atlas
//! whose placeholder sprite is swapped for the album's cover), the album title,
//! and the star rating with its flames for ratings above 10.

use std::collections::HashMap;

use crate::assets::Assets;
use crate::gfx::animate::{AnimRange, AnimateAtlas, Bounds};
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{Affine, Gfx, TexId, VIEW_W, WHITE};

const ALBUM_ART_SYMBOL: &str = "album art placeholder";
const ART_X: f32 = VIEW_W - 360.0;
const ART_Y: f32 = 220.0;
const STARS_X: f32 = VIEW_W - 330.0;
const STARS_Y: f32 = 209.0;
const TITLE_X: f32 = VIEW_W - 355.0;
const TITLE_Y: f32 = 500.0;
/// `FreeplayFlames`: five flames, 29/6 px apart, each 0.25 s after the last.
const FLAME_X: f32 = VIEW_W - 367.0;
const FLAME_Y: f32 = 91.0;
const FLAME_DELAY: f32 = 0.25;

struct Album {
    art: TexId,
    title: Option<(Atlas, Anim, Anim)>,
    offsets: [f32; 2],
}

#[derive(Clone, Copy, PartialEq)]
enum ArtAnim {
    Intro,
    Switch,
    Idle,
}

pub struct AlbumRoll {
    albums: HashMap<String, Option<Album>>,
    art: AnimateAtlas,
    art_bounds: Bounds,
    art_anims: [Option<AnimRange>; 3],
    stars: AnimateAtlas,
    stars_bounds: Bounds,
    flame: Option<(Atlas, Anim)>,

    /// `albumId`: `None` hides the roll; an unknown id keeps the last graphics.
    album_id: Option<String>,
    /// The album whose graphics are showing.
    shown: Option<String>,
    visible: bool,
    art_visible: bool,
    title_visible: bool,
    stars_visible: bool,
    art_anim: ArtAnim,
    art_since: f32,
    /// Title: playing `switch` since this time (else `idle`).
    title_switch_at: Option<f32>,
    /// `playIntro`'s 0.75 s timer.
    reveal_at: Option<f32>,
    difficulty: i32,
    stars_since: f32,
    /// When each flame was told to appear.
    flames_at: [Option<f32>; 5],
    time: f32,
}

impl AlbumRoll {
    pub fn new(gfx: &mut Gfx, assets: &Assets, album_ids: &[String]) -> Result<Self, String> {
        let art = AnimateAtlas::load(gfx, &assets.image_path("default:freeplay/albumRoll/freeplayAlbum").with_extension(""), true)?;
        let art_bounds = art.bounds(art.whole().tl);
        let art_anims = [art.anim("intro"), art.anim("switch"), art.anim("idle")];
        let stars = AnimateAtlas::load(gfx, &assets.image_path("default:freeplay/freeplayStars").with_extension(""), true)?;
        let stars_bounds = stars.bounds(stars.whole().tl);
        let flame = assets.atlas(gfx, "default:freeplay/freeplayFlame").ok().map(|a| {
            let an = a.prefix("fire loop full instance 1");
            (a, an)
        });
        let mut albums = HashMap::new();
        for id in album_ids {
            if !albums.contains_key(id) {
                let a = Self::load_album(gfx, assets, id);
                if a.is_none() {
                    log::warn!("freeplay: could not load album {id}");
                }
                albums.insert(id.clone(), a);
            }
        }
        Ok(Self {
            albums,
            art,
            art_bounds,
            art_anims,
            stars,
            stars_bounds,
            flame,
            album_id: None,
            shown: None,
            visible: false,
            art_visible: false,
            title_visible: false,
            stars_visible: false,
            art_anim: ArtAnim::Idle,
            art_since: 0.0,
            title_switch_at: None,
            reveal_at: None,
            difficulty: 1,
            stars_since: 0.0,
            flames_at: [None; 5],
            time: 0.0,
        })
    }

    fn load_album(gfx: &mut Gfx, assets: &Assets, id: &str) -> Option<Album> {
        let path = assets.root.join("data").join("ui").join("freeplay").join("albums").join(format!("{id}.json"));
        let text = funkin_core::vfs::read_to_string(&path).ok()?;
        let v: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
        let art_key = v.get("albumArtAsset")?.as_str()?;
        let art = gfx.load_texture(&assets.image_path(&format!("default:{art_key}"))).ok()?;
        let title = v.get("albumTitleAsset").and_then(|x| x.as_str()).and_then(|k| assets.atlas(gfx, &format!("default:{k}")).ok()).map(|a| {
            let idle = a.prefix("idle0");
            let switch = a.prefix("switch0");
            (a, idle, switch)
        });
        let off = |i: usize| v.get("albumTitleOffsets").and_then(|o| o.get(i)).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
        Some(Album { art, title, offsets: [off(0), off(1)] })
    }

    /// `set_albumId` / `updateAlbum`.
    pub fn set_album(&mut self, id: Option<&str>) {
        if self.album_id.as_deref() == id && id.is_some() {
            return;
        }
        self.album_id = id.map(str::to_owned);
        let Some(id) = id else {
            self.visible = false;
            return;
        };
        self.visible = true;
        if !matches!(self.albums.get(id), Some(Some(_))) {
            return;
        }
        let art = self.albums[id].as_ref().map(|a| a.art);
        self.art.replace_symbol_graphic(ALBUM_ART_SYMBOL, art);
        self.shown = Some(id.to_owned());
        // buildAlbumTitle: a fresh title sprite playing idle.
        self.title_switch_at = None;
        self.title_visible = self.visible && self.art_visible && self.stars_visible;
    }

    pub fn album_id(&self) -> Option<&str> {
        self.album_id.as_deref()
    }

    pub fn play_intro(&mut self) {
        self.visible = true;
        self.title_visible = false;
        self.art_visible = true;
        self.play_art(ArtAnim::Intro);
        self.stars_visible = false;
        self.flame_check();
        self.reveal_at = Some(self.time + 0.75);
    }

    pub fn skip_intro(&mut self) {
        self.visible = true;
        self.play_art(ArtAnim::Switch);
        self.title_switch_at = Some(self.time);
    }

    fn play_art(&mut self, a: ArtAnim) {
        self.art_anim = a;
        self.art_since = 0.0;
    }

    /// `DifficultyStars.difficulty`.
    pub fn set_difficulty_stars(&mut self, rating: i32) {
        self.difficulty = rating.clamp(0, 15);
        self.stars_since = 0.0;
        self.flame_check();
    }

    fn cur_difficulty(&self) -> u32 {
        if self.difficulty <= 0 {
            15
        } else {
            (self.difficulty - 1) as u32
        }
    }

    fn flame_check(&mut self) {
        let count = if self.difficulty > 10 { (self.difficulty - 10) as usize } else { 0 };
        let mut queued = 0;
        for i in 0..5 {
            if i < count {
                if self.flames_at[i].is_none() {
                    self.flames_at[i] = Some(self.time + FLAME_DELAY * queued as f32);
                    queued += 1;
                }
            } else {
                self.flames_at[i] = None;
            }
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        self.art_since += dt;
        self.stars_since += dt;
        if let Some(at) = self.reveal_at {
            if self.time >= at {
                self.reveal_at = None;
                self.title_visible = self.shown.is_some();
                self.stars_visible = true;
                self.flame_check();
                self.title_switch_at = Some(self.time);
            }
        }
        // onAlbumFinish: anything but idle goes to a looped idle.
        if self.art_anim != ArtAnim::Idle {
            let len = self.art_anims[self.art_anim as usize].map(|r| r.len).unwrap_or(0) as f32;
            if self.art_since * self.art.fps >= len {
                self.play_art(ArtAnim::Idle);
            }
        }
    }

    /// Draw with everything shifted right by `exit_x` (the exit movers).
    pub fn draw(&self, gfx: &mut Gfx, exit_x: f32) {
        if !self.visible {
            return;
        }
        if self.art_visible && self.shown.is_some() {
            if let Some(r) = self.art_anims[self.art_anim as usize] {
                let i = (self.art_since * self.art.fps) as u32;
                let i = if self.art_anim == ArtAnim::Idle { i % r.len.max(1) } else { i.min(r.len.saturating_sub(1)) };
                let m = Affine::translate(ART_X + exit_x - self.art_bounds[0], ART_Y - self.art_bounds[1]);
                self.art.draw(gfx, r.tl, r.start + i, &m, WHITE);
            }
        }
        if self.stars_visible {
            if let Some((atlas, an)) = &self.flame {
                for (i, at) in self.flames_at.iter().enumerate() {
                    let Some(at) = at.filter(|at| self.time >= *at) else { continue };
                    // Plays once, then loops from frame 2.
                    let e = self.time - at;
                    let n = an.frames.len();
                    let k = (e * 24.0) as usize;
                    let k = if k < n || n <= 2 { k.min(n.saturating_sub(1)) } else { 2 + (k - n) % (n - 2) };
                    if let Some(&f) = an.frames.get(k) {
                        atlas.draw(gfx, f, FLAME_X + 29.0 * i as f32 + exit_x, FLAME_Y + 6.0 * i as f32, 1.0, WHITE);
                    }
                }
            }
            let cur = self.cur_difficulty();
            let f = if cur >= 15 { 1500 } else { cur * 100 + (self.stars_since * self.stars.fps) as u32 % 100 };
            let m = Affine::translate(STARS_X + exit_x - self.stars_bounds[0], STARS_Y - self.stars_bounds[1]);
            self.stars.draw(gfx, self.stars.whole().tl, f, &m, WHITE);
        }
        if self.title_visible {
            if let Some(Some(Album { title: Some((atlas, idle, switch)), offsets, .. })) = self.shown.as_ref().map(|s| &self.albums[s]) {
                let f = match self.title_switch_at {
                    Some(at) if (self.time - at) < switch.duration(24.0) => switch.frame_at(self.time - at, 24.0, false),
                    Some(at) => idle.frame_at(self.time - at - switch.duration(24.0), 24.0, true),
                    None => idle.frame_at(self.time, 24.0, true),
                };
                if let Some(f) = f {
                    atlas.draw(gfx, f, TITLE_X + offsets[0] + exit_x, TITLE_Y + offsets[1], 1.0, WHITE);
                }
            }
        }
    }
}
