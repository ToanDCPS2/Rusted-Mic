//! Port of the core of `FreeplayState` (with Boyfriend as the DJ): the backing
//! card, the DJ's intro/idle/confirm, the song capsules with their jump-in and
//! smooth scrolling, difficulty selection, song previews, and the score/clear
//! displays (scores are 0% until save data is ported).

use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use funkin_core::song::SongRegistry;
use funkin_core::state::PlayRequest;
use winit::keyboard::KeyCode;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::animate::{AnimRange, AnimateAtlas};
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::{fill, Affine, Color, Gfx, TexId, VIEW_H, VIEW_W, WHITE};

use super::album_roll::AlbumRoll;
use super::bf_card::BfCard;
use super::{random, smooth_lerp, Fonts, MenuAction, Music, Tween};

/// `SongRegistry.listAllDifficulties` for Boyfriend.
const DIFFICULTIES: [&str; 5] = ["easy", "normal", "hard", "erect", "nightmare"];
/// `LevelRegistry.listBaseGameEntryIds`: freeplay lists songs week by week.
const BASE_LEVELS: [&str; 10] = ["tutorial", "week1", "week2", "week3", "week4", "week5", "week6", "week7", "weekend1", "sserafim"];
const REAL_SCALED: f32 = 0.8;
const X_FRAMES: [f32; 7] = [1.7, 1.8, 0.85, 0.85, 0.97, 0.97, 1.0];
const CAPSULE_W: f32 = 612.0;
const CAPSULE_H: f32 = 132.0;
const FADE_IN_DELAY: f32 = 0.25;
const PREVIEW_VOLUME: f32 = 0.7;
/// `generateSongList(.., force = true)`: capsules are opaque and show their
/// details as soon as they jump in (the 50%-alpha, delayed variant is unused).
const SHOW_AFTER: f32 = 0.0;
const GLOW: Color = [0.0, 0.8, 1.0, 1.0];

/// One song in the list.
pub struct FreeplaySong {
    pub id: String,
    pub name: String,
    /// difficulty → variation
    pub charts: Vec<(String, String)>,
    pub bpm: f64,
    ratings: HashMap<String, i32>,
    /// variation → (instrumental path, preview start, preview end) as fractions
    previews: HashMap<String, (std::path::PathBuf, f64, f64)>,
    /// variation → opponent character id (the plaque's pixel icon)
    opponents: HashMap<String, String>,
    /// variation → album id
    albums: HashMap<String, String>,
    /// The first level the song is in.
    level: Option<String>,
}

impl FreeplaySong {
    fn variation(&self, diff: &str) -> Option<&str> {
        self.charts.iter().find(|(d, _)| d == diff).map(|(_, v)| v.as_str())
    }

    /// `FreeplaySongData.songCharacter`: the opponent on this difficulty.
    fn opponent(&self, diff: &str) -> Option<&str> {
        let v = self.variation(diff).unwrap_or("default");
        self.opponents.get(v).or_else(|| self.opponents.get("default")).map(String::as_str)
    }

    /// `Song.getAlbumId`: the album on this difficulty, `""` if none.
    fn album(&self, diff: &str) -> &str {
        self.variation(diff).and_then(|v| self.albums.get(v)).map(String::as_str).unwrap_or("")
    }
}

/// Every playable song, in level order (`FreeplayState.generateSongList`).
pub fn list_songs(registry: &SongRegistry) -> Vec<FreeplaySong> {
    let mut levels = registry.load_levels();
    levels.sort_by_key(|l| (BASE_LEVELS.iter().position(|b| *b == l.id).unwrap_or(BASE_LEVELS.len()), l.id.clone()));
    let mut order: Vec<String> = Vec::new();
    for l in &levels {
        for s in &l.songs {
            if !order.contains(s) {
                order.push(s.clone());
            }
        }
    }
    // Songs that aren't in any level go last, alphabetically.
    for id in registry.list_song_ids() {
        if !order.contains(&id) {
            order.push(id);
        }
    }
    let mut out = Vec::new();
    for id in order {
        let Ok(song) = registry.load_song(&id) else { continue };
        let Some(default) = song.variations.get("default").or_else(|| song.variations.values().next()) else { continue };
        let mut charts: Vec<(String, String)> = Vec::new();
        let mut ratings = HashMap::new();
        let mut previews = HashMap::new();
        let mut opponents = HashMap::new();
        let mut albums = HashMap::new();
        for (var_id, v) in &song.variations {
            let pd = &v.metadata.play_data;
            for d in &pd.difficulties {
                if v.chart.notes(d).is_some() && !charts.iter().any(|(cd, _)| cd == d) {
                    charts.push((d.clone(), var_id.clone()));
                    ratings.insert(d.clone(), pd.ratings.get(d).copied().unwrap_or(0));
                }
            }
            let (mut a, b) = (pd.preview_start, pd.preview_end);
            let b = if b <= 0.0 { 0.2 } else { b };
            if a >= b || a > 1.0 {
                a = 0.0;
            }
            opponents.insert(var_id.clone(), pd.characters.opponent.clone());
            if let Some(a) = &pd.album {
                albums.insert(var_id.clone(), a.clone());
            }
            previews.insert(var_id.clone(), (registry.inst_path(&id, &pd.characters.instrumental), a, b));
        }
        charts.sort_by_key(|(d, _)| DIFFICULTIES.iter().position(|o| o == d).unwrap_or(99));
        if charts.is_empty() {
            continue;
        }
        out.push(FreeplaySong {
            name: default.metadata.song_name.clone(),
            bpm: default.metadata.time_changes.first().map(|t| t.bpm).unwrap_or(100.0),
            level: levels.iter().find(|l| l.songs.contains(&id)).map(|l| l.id.clone()),
            id,
            charts,
            ratings,
            previews,
            opponents,
            albums,
        });
    }
    out
}

/// A difficulty graphic: the PNG, plus its Sparrow animation if it has one.
type DiffSprite = (TexId, Option<(Atlas, Anim)>);

struct Capsule {
    /// Index into `songs`; `None` is the "Random" capsule.
    song: Option<usize>,
    x: f32,
    y: f32,
    tx: f32,
    ty: f32,
    /// When the jump-in starts (scene time).
    jump_at: f32,
}

#[derive(PartialEq)]
enum Dj {
    Intro,
    Idle,
    Confirm,
}

/// `PixelatedIcon`: a character's freeplay pixel icon (idle / confirm / confirm-hold at 10 fps).
struct PixelIcon {
    atlas: Atlas,
    idle: Anim,
    confirm: Anim,
    hold: Anim,
    /// `origin.x` (y is the frame's centre); the icon is drawn at 2x about it.
    origin_x: f32,
}

impl PixelIcon {
    /// `setCharacter`: the longest `-`-separated prefix of `char` with a `<prefix>pixel` image.
    fn load(gfx: &mut Gfx, assets: &Assets, char: &str) -> Option<Self> {
        let parts: Vec<&str> = char.split('-').collect();
        let name = (1..=parts.len())
            .map(|n| parts[..n].join("-"))
            .rfind(|n| funkin_core::vfs::is_file(&assets.image_path(&format!("default:freeplay/icons/{n}pixel"))))?;
        let base = assets.image_path(&format!("default:freeplay/icons/{name}pixel")).with_extension("");
        let atlas = if funkin_core::vfs::is_file(&base.with_extension("xml")) {
            Atlas::load_filtered(gfx, &base, false).ok()?
        } else {
            // A plain image: one frame covering the whole texture.
            let tex = gfx.load_texture_filtered(&base.with_extension("png"), false).ok()?;
            let (w, h) = gfx.texture_size(tex);
            Atlas::single(tex, w, h)
        };
        let all = Anim { frames: vec![0] };
        let pick = |p: &str| Some(atlas.prefix(p)).filter(|a| !a.frames.is_empty());
        let idle = pick("idle0").unwrap_or(all.clone());
        let confirm = pick("confirm0").unwrap_or(idle.clone());
        let hold = pick("confirm-hold0").unwrap_or(confirm.clone());
        let origin_x = match char {
            "parents-christmas" => 140.0,
            "sserafim-kazuha" => 195.0,
            _ => 100.0,
        };
        Some(Self { atlas, idle, confirm, hold, origin_x })
    }

    /// Draw at `(x, y)` (the sprite position); `confirm` = seconds since confirming.
    fn draw(&self, gfx: &mut Gfx, x: f32, y: f32, t: f32, confirm: Option<f32>) {
        let f = match confirm {
            Some(c) if c * 10.0 < self.confirm.frames.len() as f32 => self.confirm.frame_at(c, 10.0, false),
            Some(c) => self.hold.frame_at(c - self.confirm.duration(10.0), 10.0, true),
            None => self.idle.frame_at(t, 10.0, true),
        };
        let Some(f) = f else { return };
        let oy = self.atlas.frames[f].frame_h / 2.0;
        let m = Affine::translate(x + self.origin_x, y + oy)
            .then(&Affine::scale(2.0, 2.0))
            .then(&Affine::translate(-self.origin_x, -oy));
        self.atlas.draw_xf(gfx, f, &m, false, WHITE);
    }
}

/// Sparrow frames for a set of digits ("ZERO".."NINE").
struct Digits {
    atlas: Atlas,
    anims: Vec<Anim>,
}

impl Digits {
    fn load(gfx: &mut Gfx, assets: &Assets, spec: &str, suffix: &str) -> Result<Self, String> {
        let atlas = assets.atlas(gfx, spec)?;
        let names = ["ZERO", "ONE", "TWO", "THREE", "FOUR", "FIVE", "SIX", "SEVEN", "EIGHT", "NINE"];
        let anims = names.iter().map(|n| atlas.prefix(&format!("{n}{suffix}"))).collect();
        Ok(Self { atlas, anims })
    }

    /// Draw digit `d` scaled about its frame's top-left (after `updateHitbox`).
    fn draw(&self, gfx: &mut Gfx, d: usize, t: f32, x: f32, y: f32, scale: f32) {
        if let Some(f) = self.anims[d % 10].frame_at(t, 24.0, false) {
            self.atlas.draw(gfx, f, x, y, scale, WHITE);
        }
    }
}

pub struct Freeplay {
    /// `Preferences.flashingLights`: difficulty graphics animate only when on.
    pub flashing_lights: bool,
    songs: Arc<Vec<FreeplaySong>>,
    capsules: Vec<Capsule>,
    selected: usize,
    diff: usize,
    time: f32,
    intro_done_at: Option<f32>,
    exit_at: Option<f32>,
    confirm_at: Option<f32>,
    pending_play: Option<PlayRequest>,

    pink_back: TexId,
    backing_image: TexId,
    dj: AnimateAtlas,
    dj_anims: HashMap<&'static str, AnimRange>,
    dj_state: Dj,
    dj_since: f32,
    capsule: Atlas,
    capsule_sel: Anim,
    capsule_unsel: Anim,
    bpm_text: TexId,
    diff_text: TexId,
    small_nums: Digits,
    big_nums: Digits,
    diff_sprites: Vec<Option<DiffSprite>>,
    diff_slide: Tween,
    selector: Atlas,
    selector_anim: Anim,
    selector_press: [f32; 2],
    highscore: Atlas,
    score_digits: Digits,
    clear_box: TexId,
    icons: HashMap<String, Option<PixelIcon>>,
    album: AlbumRoll,
    card: BfCard,

    sfx_scroll: Option<Arc<Pcm>>,
    sfx_confirm: Option<Arc<Pcm>>,
    sfx_cancel: Option<Arc<Pcm>>,

    /// Preview scheduled for (capsule index, at time).
    preview_due: Option<(usize, f32)>,
    preview_rx: Option<(String, Receiver<Result<Pcm, String>>)>,
    preview_cache: HashMap<String, Arc<Track>>,
}

fn load_tex(gfx: &mut Gfx, assets: &Assets, spec: &str) -> Result<TexId, String> {
    gfx.load_texture(&assets.image_path(spec))
}

impl Freeplay {
    pub fn new(gfx: &mut Gfx, assets: &Assets, songs: Arc<Vec<FreeplaySong>>, remembered: Option<(&str, &str)>) -> Result<Self, String> {
        let dj = AnimateAtlas::load(gfx, &assets.level_image("default:freeplay/freeplay-boyfriend", None, ""), true)?;
        let mut dj_anims = HashMap::new();
        for (k, label) in [("intro", "Intro"), ("idle", "Idle"), ("confirm", "Confirm")] {
            if let Some(r) = dj.anim(label) {
                dj_anims.insert(k, r);
            }
        }
        let capsule = assets.atlas(gfx, "default:freeplay/freeplayCapsule/capsule/freeplayCapsule")?;
        let capsule_sel = capsule.prefix("mp3 capsule w backing0");
        let capsule_unsel = capsule.prefix("mp3 capsule w backing NOT SELECTED");
        let mut diff_sprites = Vec::new();
        for d in DIFFICULTIES {
            let spec = format!("default:freeplay/freeplay{d}");
            let sprite = match load_tex(gfx, assets, &spec) {
                Ok(t) => {
                    let sparrow = assets.atlas(gfx, &spec).ok().map(|a| {
                        let an = a.prefix("idle0");
                        (a, an)
                    });
                    Some((t, sparrow))
                }
                Err(_) => None,
            };
            diff_sprites.push(sprite);
        }
        let selector = assets.atlas(gfx, "default:freeplay/freeplaySelector/freeplaySelector")?;
        let selector_anim = selector.prefix("arrow pointer loop");

        let album_ids: Vec<String> = songs.iter().flat_map(|s| s.albums.values().cloned()).collect();
        let album = AlbumRoll::new(gfx, assets, &album_ids)?;
        let card = BfCard::new(gfx, assets);
        let mut fp = Self {
            flashing_lights: true,
            songs,
            capsules: Vec::new(),
            selected: 0,
            diff: 1,
            time: 0.0,
            intro_done_at: None,
            exit_at: None,
            confirm_at: None,
            pending_play: None,
            pink_back: load_tex(gfx, assets, "default:freeplay/pinkBack")?,
            backing_image: load_tex(gfx, assets, "default:freeplay/freeplayBGweek1-bf")?,
            dj,
            dj_anims,
            dj_state: Dj::Intro,
            dj_since: 0.0,
            capsule,
            capsule_sel,
            capsule_unsel,
            bpm_text: load_tex(gfx, assets, "default:freeplay/freeplayCapsule/bpmtext")?,
            diff_text: load_tex(gfx, assets, "default:freeplay/freeplayCapsule/difficultytext")?,
            small_nums: Digits::load(gfx, assets, "default:freeplay/freeplayCapsule/smallnumbers", "")?,
            big_nums: Digits::load(gfx, assets, "default:freeplay/freeplayCapsule/bignumbers", "")?,
            diff_sprites,
            diff_slide: Tween::new(-300.0, -300.0, 0.0, 0.0, "linear"),
            selector,
            selector_anim,
            selector_press: [-1.0; 2],
            highscore: assets.atlas(gfx, "default:freeplay/highscore")?,
            score_digits: Digits::load(gfx, assets, "default:digital_numbers", " DIGITAL")?,
            clear_box: load_tex(gfx, assets, "default:freeplay/clearBox")?,
            icons: HashMap::new(),
            album,
            card,
            sfx_scroll: assets.sound("default:scrollMenu"),
            sfx_confirm: assets.sound("default:confirmMenu"),
            sfx_cancel: assets.sound("default:cancelMenu"),
            preview_due: None,
            preview_rx: None,
            preview_cache: HashMap::new(),
        };
        let chars: Vec<String> = fp.songs.iter().flat_map(|s| s.opponents.values().cloned()).collect();
        for c in chars {
            if let std::collections::hash_map::Entry::Vacant(e) = fp.icons.entry(c) {
                let icon = PixelIcon::load(gfx, assets, e.key());
                if icon.is_none() {
                    log::debug!("freeplay: no pixel icon for {}", e.key());
                }
                e.insert(icon);
            }
        }
        if let Some((song, diff)) = remembered {
            if let Some(d) = DIFFICULTIES.iter().position(|x| *x == diff) {
                fp.diff = d;
            }
            fp.rebuild(Some(song));
        } else {
            fp.rebuild(None);
        }
        Ok(fp)
    }

    fn diff_id(&self) -> &'static str {
        DIFFICULTIES[self.diff]
    }

    /// The selected song id and difficulty, for coming back to freeplay.
    pub fn remembered(&self) -> Option<(String, String)> {
        let song = self.capsules.get(self.selected)?.song?;
        Some((self.songs[song].id.clone(), self.diff_id().to_owned()))
    }

    /// `generateSongList`: "Random" then every song with a chart on this difficulty.
    fn rebuild(&mut self, keep: Option<&str>) {
        let diff = self.diff_id();
        let mut list = vec![None];
        list.extend((0..self.songs.len()).filter(|&i| self.songs[i].variation(diff).is_some()).map(Some));
        let old: HashMap<Option<usize>, (f32, f32)> = self.capsules.iter().map(|c| (c.song, (c.x, c.y))).collect();
        self.capsules = list
            .into_iter()
            .enumerate()
            .map(|(i, song)| {
                let (x, y) = old.get(&song).copied().unwrap_or((VIEW_W, Self::intended_y(i as f32 + 1.0) + 10.0));
                Capsule { song, x, y, tx: x, ty: y, jump_at: self.time }
            })
            .collect();
        self.selected = keep
            .and_then(|id| self.capsules.iter().position(|c| c.song.is_some_and(|s| self.songs[s].id == id)))
            .unwrap_or(0)
            .min(self.capsules.len() - 1);
        self.retarget();
    }

    fn intended_x(i: f32) -> f32 {
        270.0 + 60.0 * i.sin()
    }

    fn intended_y(i: f32) -> f32 {
        i * (CAPSULE_H * REAL_SCALED + 10.0) + 120.0
    }

    /// `changeSelection`'s capsule targets.
    fn retarget(&mut self) {
        let cur = self.selected as f32;
        for (m, c) in self.capsules.iter_mut().enumerate() {
            let idx = m as f32 + 1.0 - cur;
            let mut y_off = 0.0;
            if idx < 0.0 {
                y_off += 50.0;
            } else if idx > 4.0 {
                y_off -= 10.0;
            }
            c.ty = Self::intended_y(idx) - y_off;
            c.tx = Self::intended_x(idx);
            if (m + 1) < self.selected {
                c.ty -= 100.0;
            }
        }
    }

    fn sfx(&self, audio: &Audio, s: &Option<Arc<Pcm>>, v: f32) {
        if let Some(s) = s {
            audio.play_sfx(s.clone(), v);
        }
    }

    fn interactive(&self) -> bool {
        self.intro_done_at.is_some() && self.exit_at.is_none() && self.confirm_at.is_none()
    }

    fn change_selection(&mut self, audio: &Audio, music: &mut Music, by: i32) {
        let n = self.capsules.len() as i32;
        self.selected = (self.selected as i32 + by).rem_euclid(n) as usize;
        if by != 0 {
            self.sfx(audio, &self.sfx_scroll, 0.4);
        }
        self.retarget();
        music.stop(audio);
        self.preview_rx = None;
        self.preview_due = Some((self.selected, self.time + FADE_IN_DELAY));
        self.update_album();
    }

    /// The album roll and backing card follow the selected capsule.
    fn update_album(&mut self) {
        let diff = self.diff_id();
        let song = self.capsules.get(self.selected).and_then(|c| c.song).map(|s| &self.songs[s]);
        let id = song.map(|s| s.album(diff).to_owned());
        // changeSelection clears the album for "Random" before changeDiff runs.
        if song.is_none() {
            self.album.set_album(None);
        }
        if self.album.album_id() != id.as_deref() && self.interactive() {
            self.album.set_album(id.as_deref());
            self.album.skip_intro();
        }
        self.album.set_difficulty_stars(song.and_then(|s| s.ratings.get(diff).copied()).unwrap_or(0));
        let level = song.and_then(|s| s.level.clone());
        self.card.set_level(level.as_deref());
    }

    fn change_diff(&mut self, audio: &Audio, music: &mut Music, by: i32) {
        let keep = self.remembered().map(|(s, _)| s);
        self.diff = (self.diff as i32 + by).rem_euclid(DIFFICULTIES.len() as i32) as usize;
        self.rebuild(keep.as_deref());
        let from = if by > 0 { 500.0 } else { -320.0 };
        self.diff_slide = Tween::new(from, 90.0, self.time, 0.2, "expoOut");
        self.selector_press[if by > 0 { 1 } else { 0 }] = self.time;
        self.change_selection(audio, music, 0);
        self.sfx(audio, &self.sfx_scroll, 0.4);
    }

    pub fn key(&mut self, code: KeyCode, audio: &Audio, music: &mut Music) -> MenuAction {
        if !self.interactive() {
            return MenuAction::None;
        }
        match code {
            KeyCode::ArrowUp | KeyCode::KeyW => self.change_selection(audio, music, -1),
            KeyCode::ArrowDown | KeyCode::KeyS => self.change_selection(audio, music, 1),
            KeyCode::ArrowLeft | KeyCode::KeyA => self.change_diff(audio, music, -1),
            KeyCode::ArrowRight | KeyCode::KeyD => self.change_diff(audio, music, 1),
            // FREEPLAY_JUMP_TO_TOP / _BOTTOM (see `Controls::menu_key`).
            KeyCode::F17 => {
                let by = -(self.selected as i32);
                if by != 0 {
                    self.change_selection(audio, music, by);
                }
            }
            KeyCode::F18 => {
                let by = self.capsules.len() as i32 - 1 - self.selected as i32;
                if by != 0 {
                    self.change_selection(audio, music, by);
                }
            }
            KeyCode::Enter | KeyCode::Space => {
                let song = match self.capsules[self.selected].song {
                    Some(s) => Some(s),
                    None => {
                        // Random: any song on this difficulty.
                        let pool: Vec<usize> = self.capsules.iter().filter_map(|c| c.song).collect();
                        (!pool.is_empty()).then(|| pool[((random() * pool.len() as f32) as usize).min(pool.len() - 1)])
                    }
                };
                if let Some(s) = song {
                    let song = &self.songs[s];
                    let diff = self.diff_id();
                    let variation = song.variation(diff).unwrap_or("default").to_owned();
                    self.pending_play = Some(PlayRequest {
                        song_id: song.id.clone(),
                        difficulty: diff.to_owned(),
                        variation,
                        playlist: vec![],
                        is_story: false,
                        level_id: None,
                    });
                    self.confirm_at = Some(self.time);
                    self.dj_state = Dj::Confirm;
                    self.dj_since = 0.0;
                    self.preview_due = None;
                    music.fade(PREVIEW_VOLUME, 0.0, 1.0);
                    self.sfx(audio, &self.sfx_confirm, 1.0);
                }
            }
            KeyCode::Escape | KeyCode::Backspace => {
                self.exit_at = Some(self.time);
                self.preview_due = None;
                self.preview_rx = None;
                music.stop(audio);
                self.sfx(audio, &self.sfx_cancel, 1.0);
            }
            _ => {}
        }
        MenuAction::None
    }

    pub fn update(&mut self, dt: f32, audio: &Audio, assets: &Assets, music: &mut Music) -> MenuAction {
        self.time += dt;
        self.dj_since += dt;

        // DJ: intro → idle (and the rest of the screen appears).
        if self.dj_state == Dj::Intro {
            let len = self.dj_anims.get("intro").map(|r| r.len).unwrap_or(0) as f32;
            if self.dj_since * 24.0 >= len {
                self.dj_state = Dj::Idle;
                self.dj_since = 0.0;
                self.intro_done_at = Some(self.time);
                self.diff_slide = Tween::new(-300.0, 90.0, self.time, 0.6, "quartOut");
                self.album.play_intro();
                let diff = self.diff_id();
                let id = self.capsules.get(self.selected).and_then(|c| c.song).map(|s| self.songs[s].album(diff).to_owned());
                self.album.set_album(id.as_deref());
                self.change_selection(audio, music, 0);
            }
        }

        self.album.update(dt);
        // The conductor follows the preview (mapped to its song's BPM).
        let bpm = music
            .id
            .split_once(':')
            .and_then(|(id, _)| self.songs.iter().find(|s| s.id == id))
            .map(|s| s.bpm)
            .unwrap_or(102.0);
        let ms = music.time_ms(audio);
        self.card.update(self.time, ms, bpm, music.playing());

        for c in &mut self.capsules {
            if self.time >= c.jump_at {
                c.x = smooth_lerp(c.x, c.tx, dt, 0.256);
                c.y = smooth_lerp(c.y, c.ty, dt, 0.192);
            }
        }

        // Song previews.
        if let Some((idx, at)) = self.preview_due {
            if self.time >= at {
                self.preview_due = None;
                if idx == self.selected {
                    self.start_preview(audio, assets, music);
                }
            }
        }
        if let Some((key, rx)) = &self.preview_rx {
            if let Ok(result) = rx.try_recv() {
                let key = key.clone();
                self.preview_rx = None;
                match result {
                    Ok(pcm) => {
                        let t = Track::new(pcm);
                        self.preview_cache.insert(key.clone(), t.clone());
                        if self.preview_key().as_deref() == Some(key.as_str()) {
                            self.play_preview(audio, music, &key, t);
                        }
                    }
                    Err(e) => log::warn!("preview: {e}"),
                }
            }
        }

        if let Some(at) = self.confirm_at {
            if self.time - at >= 1.0 {
                if let Some(req) = self.pending_play.take() {
                    music.stop(audio);
                    return MenuAction::Play(req);
                }
            }
        }
        if let Some(at) = self.exit_at {
            if self.time - at >= 0.4 {
                return MenuAction::CloseFreeplay;
            }
        }
        MenuAction::None
    }

    fn preview_key(&self) -> Option<String> {
        let s = self.capsules.get(self.selected)?.song?;
        let song = &self.songs[s];
        let var = song.variation(self.diff_id())?;
        Some(format!("{}:{}", song.id, var))
    }

    fn start_preview(&mut self, audio: &Audio, assets: &Assets, music: &mut Music) {
        let Some(key) = self.preview_key() else {
            // The "Random" capsule plays its own track.
            if music.play_menu(audio, assets, "freeplayRandom", false) {
                music.fade(0.0, PREVIEW_VOLUME, 2.0);
            }
            return;
        };
        if let Some(t) = self.preview_cache.get(&key).cloned() {
            self.play_preview(audio, music, &key, t);
            return;
        }
        let Some(s) = self.capsules[self.selected].song else { return };
        let song = &self.songs[s];
        let var = song.variation(self.diff_id()).unwrap_or("default");
        let Some((path, ..)) = song.previews.get(var).or_else(|| song.previews.get("default")).cloned() else { return };
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(Pcm::load_ogg(&path));
        });
        self.preview_rx = Some((key, rx));
    }

    fn play_preview(&mut self, audio: &Audio, music: &mut Music, key: &str, track: Arc<Track>) {
        let Some((id, var)) = key.split_once(':') else { return };
        let Some(song) = self.songs.iter().find(|s| s.id == id) else { return };
        let Some(&(_, a, b)) = song.previews.get(var).or_else(|| song.previews.get("default")) else { return };
        let len = track.pcm.duration_ms();
        music.start_preview(audio, key, track, (a * len, b * len), PREVIEW_VOLUME);
    }

    fn dj_frame(&self) -> Option<(AnimRange, u32)> {
        let (name, looped) = match self.dj_state {
            Dj::Intro => ("intro", false),
            Dj::Idle => ("idle", true),
            Dj::Confirm => ("confirm", false),
        };
        let r = *self.dj_anims.get(name)?;
        let i = (self.dj_since * 24.0) as u32;
        let i = if looped { i % r.len.max(1) } else { i.min(r.len.saturating_sub(1)) };
        Some((r, r.start + i))
    }

    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts) {
        let t = self.time;
        let intro = self.intro_done_at;
        let since_intro = intro.map(|at| t - at);
        // Exit movers: everything slides off in 0.4 s.
        let exit = self.exit_at.map(|at| ((t - at) / 0.4).clamp(0.0, 1.0)).unwrap_or(0.0);
        let exit_left = -exit * 700.0;
        let exit_right = exit * 900.0;
        let exit_up = -exit * 170.0;

        // Backing card: the pink card slides in, turning yellow once the DJ's intro ends.
        let (pw, ph) = gfx.texture_size(self.pink_back);
        let pink_x = Tween::new(-pw, 0.0, 0.0, 0.6, "quartOut").at(t) + exit_left;
        let confirm = self.confirm_at.map(|at| t - at);
        let pink_col = match (confirm, intro) {
            // disappear(): yellow back to pink over 0.25 s.
            _ if self.exit_at.is_some() => {
                let e = t - self.exit_at.unwrap_or(t);
                lerp_color(hex(0xFFD863), hex(0xFFD0D5), ease_k("quadOut", e / 0.25))
            }
            (Some(c), _) => lerp_color([1.0, 0.816, 0.835, 1.0], [0.09, 0.094, 0.192, 1.0], ease_k("quadOut", c / 0.33)),
            (None, Some(_)) => hex(0xFFD863),
            _ => hex(0xFFD4E9),
        };
        gfx.image(self.pink_back, [0.0, 0.0, pw, ph], pink_x, 0.0, pw, ph, pink_col);
        // disappear() hides the orange strip, scrolling texts and glow at once.
        if intro.is_some() && confirm.is_none() && self.exit_at.is_none() {
            // orangeBackShit, alpha-masked by the card's own top rows.
            gfx.image(self.pink_back, [0.0, 0.0, pw, 75.0], 84.0 + pink_x, 440.0, pw, 75.0, hex(0xFEDA00));
            gfx.rect(pink_x, 440.0, 100.0, 75.0, hex(0xFFD400));
            self.card.draw(gfx, fonts, t);
        }

        // DJ, drawn through the Animate stage matrix.
        if let Some((r, f)) = self.dj_frame() {
            let m = Affine::translate(640.0 + exit_left * 1.2, 360.0);
            self.dj.draw(gfx, r.tl, f, &m, WHITE);
        }

        // Backing image (angle-masked), fading from black after the intro.
        let (bw, bh) = gfx.texture_size(self.backing_image);
        let s = (VIEW_H + 1.0) / bh;
        let bx = pw * 0.74 + exit_right;
        let black_x = Tween::new(VIEW_W, bx, 0.0, 0.7, "quintOut").at(t).max(bx);
        let tint = match (since_intro, confirm) {
            (_, Some(c)) => 0.66 - 0.33 * ease_k("expoOut", c / 2.0),
            (Some(e), None) => ease_k("expoOut", e / 0.6),
            _ => 0.0,
        };
        if since_intro.is_none_or(|e| e < 0.6) {
            let white = gfx.white();
            draw_angle_masked(gfx, white, 90.0 / bw, black_x, bw * s, bh * s, [0.0, 0.0, 0.0, 1.0]);
        }
        if intro.is_some() {
            draw_angle_masked(gfx, self.backing_image, 90.0 / bw, bx, bw * s, bh * s, [tint, tint, tint, 1.0]);
        }

        // Difficulty sprite.
        let diff_x = self.diff_slide.at(t) + exit_left;
        if intro.is_some() {
            if let Some(Some((tex, sparrow))) = self.diff_sprites.get(self.diff) {
                match sparrow {
                    Some((a, an)) if !an.frames.is_empty() => {
                        let at = if self.flashing_lights { t } else { 0.0 };
                        if let Some(f) = an.frame_at(at, 24.0, true) {
                            a.draw(gfx, f, diff_x, 80.0, 1.0, WHITE);
                        }
                    }
                    _ => gfx.sprite(*tex, diff_x, 80.0, 1.0, WHITE),
                }
            }
        }

        // Capsules.
        for (m, c) in self.capsules.iter().enumerate() {
            if t < c.jump_at {
                continue;
            }
            let jump = t - c.jump_at;
            self.draw_capsule(gfx, fonts, c, m == self.selected, jump, exit_right);
        }

        // Difficulty selectors.
        if intro.is_some() {
            for (i, x) in [20.0f32, 325.0].into_iter().enumerate() {
                let pressed = self.selector_press[i] >= 0.0 && t - self.selector_press[i] < 0.1;
                if let Some(f) = self.selector_anim.frame_at(t, 24.0, true) {
                    let m = Affine::translate(x + exit_left, 70.0 + if pressed { 5.0 } else { 0.0 });
                    self.selector.draw_xf(gfx, f, &m, i == 1, WHITE);
                }
            }
        }

        self.album.draw(gfx, exit * 360.0);

        // Overhang bar + texts.
        let bar_y = Tween::new(-164.0, -100.0, 0.0, 0.3, "quartOut").at(t) + exit_up;
        gfx.rect(0.0, bar_y, VIEW_W, 164.0, [0.0, 0.0, 0.0, 1.0]);
        if let Some(e) = since_intro {
            if e >= 1.0 / 24.0 {
                let y = 8.0 + exit_up;
                let ost = "Official OST";
                match &fonts.vcr {
                    Some(vcr) => {
                        // topLeftCornerText (its stroke is cleared after the intro).
                        vcr.draw(gfx, "FREEPLAY", 8.0, y, 48.0, WHITE);
                        // ostName: right-aligned in an 8..1272 field, 2 px white stroke.
                        let w = vcr.width(gfx, ost, 48.0);
                        let x = VIEW_W - 8.0 - 2.0 - w - 2.0;
                        for (dx, dy) in [(-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
                            vcr.draw(gfx, ost, x + dx, y + dy, 48.0, WHITE);
                        }
                        vcr.draw(gfx, ost, x, y, 48.0, WHITE);
                    }
                    None => {
                        let ts = 0.62;
                        fonts.default.draw(gfx, "FREEPLAY", 8.0, y, ts, fill(WHITE));
                        let w = fonts.default.width(&ost.to_uppercase(), ts);
                        fonts.default.draw(gfx, &ost.to_uppercase(), VIEW_W - 8.0 - w, y, ts, fill(WHITE));
                    }
                }

                // Score panel.
                if let Some(f) = self.highscore.prefix("highscore small instance 1").frames.first() {
                    self.highscore.draw(gfx, *f, VIEW_W - 420.0 + exit_right, 70.0, 1.0, WHITE);
                }
                for i in 0..7 {
                    let x = VIEW_W - 353.0 + 45.0 * i as f32 + exit_right;
                    self.score_digits.draw(gfx, 0, e, x, 120.0, 0.4);
                }
                gfx.sprite(self.clear_box, VIEW_W - 115.0 + exit_right, 65.0, 1.0, WHITE);
                // txtCompletion "0", right-aligned with offset.x = -24.
                fonts.clear.draw(gfx, "0", VIEW_W - 95.0 + 24.0 + exit_right, 87.0, 1.0, WHITE);
            }
        }
    }

    fn draw_capsule(&self, gfx: &mut Gfx, fonts: &Fonts, c: &Capsule, selected: bool, jump: f32, exit_x: f32) {
        let (x, y) = (c.x + exit_x, c.y);
        // Jump-in wobble: 7 frames of x/y squash at 24 fps.
        let k = ((jump.max(0.0) * 24.0) as usize).min(X_FRAMES.len() - 1);
        let xf = X_FRAMES[k];
        let (sx, sy) = (xf * REAL_SCALED, REAL_SCALED / xf);
        let anim = if selected { &self.capsule_sel } else { &self.capsule_unsel };
        if let Some(f) = anim.frame_at(self.time, 24.0, true) {
            let off = if selected { 0.0 } else { 5.0 };
            let m = Affine::translate(x + off + CAPSULE_W / 2.0, y + CAPSULE_H / 2.0)
                .then(&Affine::scale(sx, sy))
                .then(&Affine::translate(-CAPSULE_W / 2.0, -CAPSULE_H / 2.0));
            self.capsule.draw_xf(gfx, f, &m, false, WHITE);
        }

        let name = match c.song {
            Some(s) => self.songs[s].name.as_str(),
            None => "Random",
        };
        let (tx, ty) = (x + CAPSULE_W * 0.26, y + 45.0);
        let alpha = if selected { 1.0 } else { 0.6 };
        match &fonts.five_by_seven {
            Some(f) => {
                // CapsuleText: 5by7 at 32 px, white with a glow; `textAppear` squashes it in.
                let since = jump - SHOW_AFTER;
                let (sx, sy) = if since < 1.0 / 24.0 {
                    (1.7, 0.2)
                } else if since < 3.0 / 24.0 {
                    (0.4, 1.4)
                } else {
                    (1.0, 1.0)
                };
                let size = 32.0;
                let w = f.width(gfx, name, size) + 4.0;
                let h = size * 1.2;
                let (ox, oy) = (tx + w * (1.0 - sx) / 2.0, ty + h * (1.0 - sy) / 2.0);
                // A faint halo outside the strokes; the letters themselves stay white.
                let glow = if selected { 0.12 } else { 0.05 };
                for r in [4.0f32, 3.0] {
                    for k in 0..8 {
                        let a = k as f32 * std::f32::consts::FRAC_PI_4;
                        f.draw_scaled(gfx, name, ox + r * a.cos(), oy + r * a.sin(), size, sx, sy, [GLOW[0], GLOW[1], GLOW[2], glow]);
                    }
                }
                f.draw_scaled(gfx, name, ox, oy, size, sx, sy, [1.0, 1.0, 1.0, alpha]);
            }
            None => {
                let ts = 0.5;
                if selected {
                    for (dx, dy) in [(-2.0, 0.0), (2.0, 0.0), (0.0, -2.0), (0.0, 2.0)] {
                        fonts.default.draw(gfx, name, tx + dx, ty + dy, ts, fill([GLOW[0], GLOW[1], GLOW[2], 0.45]));
                    }
                }
                fonts.default.draw(gfx, name, tx, ty, ts, fill([1.0, 1.0, 1.0, alpha]));
            }
        }

        // BPM and difficulty rating.
        let Some(s) = c.song else { return };
        let song = &self.songs[s];
        if let Some(Some(icon)) = song.opponent(self.diff_id()).and_then(|o| self.icons.get(o)) {
            let confirm = self.confirm_at.filter(|_| selected).map(|at| self.time - at);
            icon.draw(gfx, x + 160.0, y + 35.0, self.time, confirm);
        }
        let gray = if selected { WHITE } else { [0.55, 0.55, 0.55, 1.0] };
        let (bw, bh) = gfx.texture_size(self.bpm_text);
        gfx.image(self.bpm_text, [0.0, 0.0, bw, bh], x + 144.0 + bw * 0.05, y + 87.0 + bh * 0.05, bw * 0.9, bh * 0.9, gray);
        let (dw, dh) = gfx.texture_size(self.diff_text);
        gfx.image(self.diff_text, [0.0, 0.0, dw, dh], x + 414.0 + dw * 0.05, y + 87.0 + dh * 0.05, dw * 0.9, dh * 0.9, gray);

        let bpm = song.bpm.round().max(0.0) as i64;
        let shift = if bpm / 100 == 1 { 186.0 } else { 191.0 };
        let digits = [if bpm < 100 { 0 } else { (bpm / 100) % 10 }, if bpm < 10 { 0 } else { (bpm / 10) % 10 }, bpm % 10];
        let mut temp = 0.0;
        for (i, d) in digits.iter().enumerate() {
            if i == 1 && bpm >= 10 && *d == 1 {
                temp = -4.0;
            }
            if i == 2 && *d == 1 {
                temp -= 4.0;
            }
            self.small_nums.draw(gfx, *d as usize, 99.0, x + shift + i as f32 * 11.0 + temp, y + 88.5, 0.9);
        }
        let rating = song.ratings.get(self.diff_id()).copied().unwrap_or(0).max(0) as usize;
        let rd = [if rating < 10 { 0 } else { rating / 10 }, rating % 10];
        for (i, d) in rd.iter().enumerate() {
            self.big_nums.draw(gfx, *d, 99.0, x + 466.0 + i as f32 * 30.0, y + 32.0, 0.9);
        }
    }
}

fn hex(c: u32) -> Color {
    [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, 1.0]
}

fn lerp_color(a: Color, b: Color, k: f32) -> Color {
    [a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, a[2] + (b[2] - a[2]) * k, 1.0]
}

fn ease_k(name: &str, t: f32) -> f32 {
    funkin_core::camera::ease(name, t.clamp(0.0, 1.0) as f64) as f32
}

/// `AngleMask` (endPosition.x = 90 texels): hide everything left of the line
/// from the texture's top-left to (90, bottom).
fn draw_angle_masked(gfx: &mut Gfx, tex: TexId, cut: f32, x: f32, w: f32, h: f32, color: Color) {
    let dst = [[x, 0.0], [x + w, 0.0], [x + w, h], [x + w * cut, h]];
    gfx.quad_uv(tex, [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [cut, 1.0]], dst, color);
}
