//! Port of `GameOverSubState`: the player's death animation on black, the
//! blue-balled sound, the game over music (`gameOverStart` → `gameOver` loop →
//! `gameOverEnd`), Tankman's death quotes, and the retry confirm. Includes the
//! death hooks from the playable characters' scripts (`pico-playable.hxc`,
//! `pico-pixel.hxc`, `pico-blazin.hxc`, `pico-holding-nene.hxc`, ...).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::{Gfx, VIEW_H, VIEW_W};
use crate::menus::smooth_lerp;
use crate::world::{CharType, World};

/// `Constants.DEFAULT_CAMERA_FOLLOW_RATE / 2`.
const FOLLOW_RATE: f64 = 0.04 / 2.0;
const CAMERA_ZOOM_DURATION: f32 = 0.5;

/// What the game over screen wants next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeathAction {
    None,
    /// The confirm fade is done: restart the song, fading in (in 10 steps if `pixel`).
    Retry { pixel: bool },
}

struct DeathMusic {
    track: Arc<Track>,
    id: u64,
    len_ms: f64,
    looped: bool,
    /// `gameOverStart`: when it ends, the loop starts.
    starting: bool,
    /// `fadeIn(duration, from, to)`: (from, to, start, duration).
    fade: Option<(f32, f32, Instant, f32)>,
}

impl DeathMusic {
    fn playing(&self, audio: &Audio) -> bool {
        self.looped || !audio.started(self.id) || audio.song_time_at(Instant::now()) < self.len_ms
    }
}

/// A timer from a character script, relative to the first death animation.
#[derive(Clone, Copy)]
enum Hook {
    /// `pico-blazin`: the music and loop start 1.25 s in.
    BlazinMusic,
    /// `pico-holding-nene`: the quote starts 0.58 s in.
    NeneQuote,
}

pub struct GameOver {
    music_suffix: String,
    blue_ball_suffix: String,
    level: Option<String>,
    player: String,
    started: bool,
    blueballed: bool,
    is_starting: bool,
    is_ending: bool,
    music: Option<DeathMusic>,
    quote: Option<Arc<Pcm>>,
    /// The playing quote: (sfx id, when it started, its length in seconds).
    quote_playing: Option<(u64, Instant, f32)>,
    has_played_quote: bool,
    target_zoom: f64,
    /// Nene's knife toss (Pico's deaths).
    nene: Option<Actor>,
    hooks: Vec<(f32, Hook)>,
    time: f32,
    first_death_frame: usize,
    /// Confirmed at this time; the fade starts `fade_delay` later.
    confirmed: Option<(f32, f32)>,
}

/// `Paths.music` / `Paths.sound` with the current level: the level library,
/// then `shared`, then the default library.
fn find(assets: &Assets, level: Option<&str>, kind: &str, key: &str) -> Option<PathBuf> {
    let root = assets.root();
    level
        .into_iter()
        .chain(["shared", ""])
        .map(|l| if l.is_empty() { root.join(kind) } else { root.join(l).join(kind) }.join(format!("{key}.ogg")))
        .find(|p| funkin_core::vfs::exists(p))
}

/// `FlxG.random.int(min, max, excludes)`.
fn random_int(min: u32, max: u32, exclude: &[u32]) -> u32 {
    let pool: Vec<u32> = (min..=max).filter(|n| !exclude.contains(n)).collect();
    pool[((crate::menus::random() * pool.len() as f32) as usize).min(pool.len() - 1)]
}

/// The player character's `getDeathQuote()` (week 7's taunts), as a sound key.
pub fn death_quote_key(player: &str, opponent: &str, naughty: bool) -> Option<String> {
    let bf = || {
        let censor: &[u32] = if naughty { &[] } else { &[1, 3, 8, 13, 17, 21] };
        format!("jeffGameover/jeffGameover-{}", random_int(1, 25, censor))
    };
    let pico = || {
        let censor: &[u32] = if naughty { &[] } else { &[4, 7, 8, 9] };
        format!("jeffGameover-pico/jeffGameover-{}", random_int(1, 10, censor))
    };
    match player {
        "bf-holding-gf" => Some(bf()),
        "pico-holding-nene" => Some(pico()),
        "bf" if opponent == "tankman" => Some(bf()),
        "pico-playable" if opponent == "tankman" => Some(pico()),
        _ => None,
    }
}

/// The suffixes the player's script sets (`GameOverSubState.musicSuffix`, `blueBallSuffix`).
fn suffixes(player: &str) -> (&'static str, &'static str) {
    match player {
        "bf-pixel" => ("-pixel", "-pixel"),
        "pico-pixel" => ("-pixel-pico", "-pixel-pico"),
        "pico-blazin" => ("-pico", "-pico-gutpunch"),
        "pico-holding-nene" => ("-pico", "-pico-and-nene"),
        "pico-playable" => ("-pico", "-pico"),
        _ => ("", ""),
    }
}

impl GameOver {
    /// `create` + `setCameraTarget`. `quote` is the death quote, if the player has one.
    pub fn new(gfx: &mut Gfx, assets: &Assets, world: &mut World, opponent: &str, naughty: bool) -> Self {
        let level = world.stage.directory.clone();
        let player = world.chars[CharType::Bf as usize].as_ref().map(|c| c.id.clone()).unwrap_or_default();
        let (music_suffix, blue_ball_suffix) = suffixes(&player);
        let quote = death_quote_key(&player, opponent, naughty)
            .and_then(|k| find(assets, level.as_deref(), "sounds", &k))
            .and_then(|p| Pcm::load_ogg(&p).map_err(|e| log::warn!("death quote: {e}")).ok())
            .map(Arc::new);

        let cam = &mut world.camera;
        cam.current_zoom = cam.zoom();
        cam.bop_multiplier = 1.0;
        cam.hud_zoom = 1.0;
        let mut target_zoom = cam.stage_zoom;
        if let Some(bf) = world.chars[CharType::Bf as usize].as_mut() {
            let [x, y] = bf.death_focus;
            world.camera.focus(x, y, None, "");
            target_zoom *= bf.death.camera_zoom;
        }

        // Nene's knife toss, behind Pico.
        let gf = world.chars[CharType::Gf as usize].as_ref().map(|g| (g.actor.x, g.actor.y, g.actor.global_offsets));
        let nene = match (player.as_str(), gf) {
            ("pico-playable", Some((gx, gy, _))) => knife_toss(gfx, assets, level.as_deref(), "characters/NeneKnifeToss", "knife toss0", gx + 120.0, gy, 1.0),
            ("pico-pixel", Some((gx, gy, off))) => {
                knife_toss(gfx, assets, level.as_deref(), "characters/nenePixel/nenePixelKnifeToss", "knifetosscolor0", gx + 280.0 + off[0] * 6.0, gy + 170.0 + off[1] * 6.0, 6.0)
            }
            _ => None,
        };
        let hooks = match player.as_str() {
            "pico-blazin" => vec![(1.25, Hook::BlazinMusic)],
            "pico-holding-nene" => vec![(0.58, Hook::NeneQuote)],
            _ => Vec::new(),
        };

        Self {
            music_suffix: music_suffix.to_owned(),
            blue_ball_suffix: blue_ball_suffix.to_owned(),
            level,
            player,
            started: false,
            blueballed: false,
            is_starting: true,
            is_ending: false,
            music: None,
            quote,
            quote_playing: None,
            has_played_quote: false,
            target_zoom,
            nene,
            hooks,
            time: 0.0,
            first_death_frame: 0,
            confirmed: None,
        }
    }

    /// `resolveMusicPath`: strip `-suffix` parts until a track exists.
    fn music_path(&self, assets: &Assets) -> Option<PathBuf> {
        let base = format!(
            "gameplay/gameover/gameOver{}",
            if self.is_ending { "End" } else if self.is_starting { "Start" } else { "" }
        );
        let mut suffix = self.music_suffix.clone();
        loop {
            if let Some(p) = find(assets, self.level.as_deref(), "music", &format!("{base}{suffix}")) {
                return Some(p);
            }
            if suffix.is_empty() {
                return None;
            }
            suffix = match suffix.rfind('-') {
                Some(i) => suffix[..i].to_owned(),
                None => String::new(),
            };
        }
    }

    /// `startDeathMusic`.
    fn start_music(&mut self, audio: &Audio, assets: &Assets, volume: f32, force: bool) {
        let mut path = self.music_path(assets);
        let mut starting = false;
        if self.is_starting {
            if path.is_none() {
                self.is_starting = false;
                path = self.music_path(assets);
            } else {
                starting = true;
            }
        }
        let Some(path) = path else {
            log::warn!("game over music not found");
            return;
        };
        if self.music.as_ref().is_some_and(|m| m.playing(audio)) && !force {
            return;
        }
        let pcm = match Pcm::load_ogg(&path) {
            Ok(p) => p,
            Err(e) => {
                log::warn!("game over music: {e}");
                return;
            }
        };
        let track = Track::new(pcm);
        track.set_volume(volume);
        let len_ms = track.pcm.duration_ms();
        let looped = !(self.is_ending || self.is_starting);
        let id = if looped { audio.play_looped(vec![track.clone()], 0.0, 0.0, len_ms) } else { audio.play(vec![track.clone()], 0.0) };
        self.music = Some(DeathMusic { track, id, len_ms, looped, starting, fade: None });
    }

    /// `playBlueBalledSFX`.
    fn blue_balled(&mut self, audio: &Audio, assets: &Assets) {
        self.blueballed = true;
        let key = format!("gameplay/gameover/fnf_loss_sfx{}", self.blue_ball_suffix);
        match find(assets, self.level.as_deref(), "sounds", &key).and_then(|p| Pcm::load_ogg(&p).ok()) {
            Some(p) => audio.play_sfx(Arc::new(p), 1.0),
            None => log::warn!("missing blue ball sound effect: {key}"),
        }
    }

    fn stop_quote(&mut self, audio: &Audio) {
        if let Some((id, ..)) = self.quote_playing.take() {
            audio.stop_sfx(id);
        }
    }

    /// `playDeathQuote`.
    fn play_quote(&mut self, world: &mut World, audio: &Audio, assets: &Assets) {
        if self.is_ending {
            return;
        }
        let Some(q) = self.quote.clone() else { return };
        self.stop_quote(audio);
        self.start_music(audio, assets, 0.2, false);
        if let Some(bf) = world.char_mut(CharType::Bf) {
            bf.actor.play("deathLoop", false, true);
        }
        let len = q.duration_ms() as f32 / 1000.0;
        self.quote_playing = Some((audio.play_sfx_tracked(q, 1.0), Instant::now(), len));
    }

    /// The death music and loop start (when the first death animation is over).
    fn start_loop(&mut self, world: &mut World, audio: &Audio, assets: &Assets) {
        self.start_music(audio, assets, 1.0, false);
        if let Some(bf) = world.char_mut(CharType::Bf) {
            bf.actor.play("deathLoop", false, true);
        }
    }

    /// Accept: `confirmDeath`.
    pub fn confirm(&mut self, world: &mut World, audio: &Audio, assets: &Assets) {
        if !self.blueballed || self.is_ending {
            return;
        }
        self.blueballed = false;
        self.is_ending = true;
        self.has_played_quote = true;
        self.stop_quote(audio);
        self.start_music(audio, assets, 1.0, true);
        if let Some(bf) = world.char_mut(CharType::Bf) {
            bf.actor.play("deathConfirm", true, true);
        }
        let len = self.music.as_ref().map(|m| m.len_ms).unwrap_or(0.0);
        self.confirmed = Some((self.time, (len / 7000.0) as f32));
    }

    /// Back: `goBack` (only once the blue-balled sound has played).
    pub fn can_go_back(&self) -> bool {
        self.blueballed && !self.is_ending
    }

    /// Leaving for the menu: stop the music and any quote.
    pub fn stop(&mut self, audio: &Audio) {
        self.is_ending = true;
        self.blueballed = false;
        self.has_played_quote = true;
        self.stop_quote(audio);
        if self.music.take().is_some() {
            audio.stop();
        }
    }

    fn pixel(&self) -> bool {
        self.music_suffix.contains("-pixel")
    }

    pub fn update(&mut self, dt: f32, world: &mut World, audio: &Audio, assets: &Assets) -> DeathAction {
        self.time += dt;
        let has_bf = world.chars[CharType::Bf as usize].is_some();
        if !self.started {
            self.started = true;
            let has_anim = world.chars[CharType::Bf as usize].as_ref().is_some_and(|b| b.actor.has_anim("firstDeath"));
            if let (true, Some(bf)) = (has_anim, world.char_mut(CharType::Bf)) {
                bf.actor.play("firstDeath", true, true);
            }
            if let Some(n) = &mut self.nene {
                n.play("throw", true, false);
            }
            self.blue_balled(audio, assets);
            if !has_anim {
                self.start_music(audio, assets, 1.0, false);
            }
        }

        // The camera: follow the death focus, zoom to the death zoom.
        let cam = &mut world.camera;
        cam.update_follow(dt as f64, FOLLOW_RATE);
        cam.current_zoom = smooth_lerp(cam.current_zoom as f32, self.target_zoom as f32, dt, CAMERA_ZOOM_DURATION) as f64;

        if let Some(bf) = world.char_mut(CharType::Bf) {
            bf.actor.update(dt);
        }
        if let Some(n) = &mut self.nene {
            n.update(dt);
            if n.finished() {
                n.visible = false;
            }
        }

        // Script timers.
        let due: Vec<Hook> = self.hooks.iter().filter(|(at, _)| self.time >= *at).map(|(_, h)| *h).collect();
        self.hooks.retain(|(at, _)| self.time < *at);
        for h in due {
            match h {
                Hook::BlazinMusic => {
                    if !self.is_ending {
                        self.start_music(audio, assets, 1.0, false);
                        if let Some(bf) = world.char_mut(CharType::Bf) {
                            bf.actor.play("deathLoop", true, true);
                        }
                    }
                }
                Hook::NeneQuote => {
                    if !self.has_played_quote {
                        self.has_played_quote = true;
                        self.play_quote(world, audio, assets);
                    }
                }
            }
        }

        // Frame hooks (`onAnimationFrame`).
        let (cur, frame, count, finished) = match world.chars[CharType::Bf as usize].as_ref() {
            Some(b) => (b.actor.current().to_owned(), b.actor.frame_number(), b.actor.frame_count(), b.actor.finished()),
            None => (String::new(), 0, 0, false),
        };
        if cur == "firstDeath" && !self.is_ending {
            let prev = self.first_death_frame;
            let passed = |n: usize| prev < n && frame >= n;
            let (at35, at_last) = (passed(35), count > 0 && passed(count - 1));
            self.first_death_frame = frame;
            match self.player.as_str() {
                "pico-playable" if at35 => {
                    if self.quote.is_some() {
                        if !self.has_played_quote {
                            self.has_played_quote = true;
                            self.play_quote(world, audio, assets);
                        }
                    } else {
                        self.start_music(audio, assets, 1.0, false);
                    }
                    if let Some(bf) = world.char_mut(CharType::Bf) {
                        bf.actor.play("deathLoop", false, true);
                    }
                }
                "pico-pixel" => {
                    if at35 {
                        self.start_music(audio, assets, 1.0, false);
                    }
                    if at_last {
                        if let Some(bf) = world.char_mut(CharType::Bf) {
                            bf.actor.play("deathLoop", false, true);
                        }
                    }
                }
                _ => {}
            }
        }
        if cur == "deathConfirm" && finished && self.player == "pico-holding-nene" {
            if let Some(bf) = world.char_mut(CharType::Bf) {
                bf.actor.play("deathConfirm-loop", true, true);
            }
        }

        // The music: the start jingle hands over to the loop; fades.
        if let Some(m) = &mut self.music {
            if let Some((from, to, at, dur)) = m.fade {
                let k = (at.elapsed().as_secs_f32() / dur).min(1.0);
                m.track.set_volume(from + (to - from) * k);
                if k >= 1.0 {
                    m.fade = None;
                }
            }
        }
        let music_playing = self.music.as_ref().is_some_and(|m| m.playing(audio));
        if !music_playing && self.music.as_ref().is_some_and(|m| m.starting) && !self.is_ending {
            self.is_starting = false;
            self.start_music(audio, assets, 1.0, true);
        } else if !music_playing && has_bf && !self.is_ending {
            let first_done = cur.starts_with("firstDeath") && finished;
            if self.quote.is_some() {
                if first_done && !self.has_played_quote {
                    self.has_played_quote = true;
                    self.play_quote(world, audio, assets);
                }
            } else if first_done {
                self.start_loop(world, audio, assets);
            }
        }
        // The quote ended: the music comes back up.
        if let Some((_, at, len)) = self.quote_playing {
            if at.elapsed().as_secs_f32() >= len {
                self.quote_playing = None;
                if let (false, Some(m)) = (self.is_ending, &mut self.music) {
                    m.fade = Some((0.2, 1.0, Instant::now(), 4.0));
                }
            }
        }

        // After the confirm: wait, fade to black over 2 s, then retry.
        if let Some((at, delay)) = self.confirmed {
            if self.time - at >= delay + 2.0 {
                return DeathAction::Retry { pixel: self.pixel() };
            }
        }
        DeathAction::None
    }

    pub fn draw(&self, gfx: &mut Gfx, world: &World) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        if let Some(bf) = &world.chars[CharType::Bf as usize] {
            bf.actor.draw(gfx, &world.camera);
        }
        if let Some(n) = &self.nene {
            n.draw(gfx, &world.camera);
        }
        if let Some((at, delay)) = self.confirmed {
            // FlxG.camera.fade(BLACK, 2), or RetroCameraFade's 10 steps for pixel styles.
            let k = ((self.time - at - delay) / 2.0).clamp(0.0, 1.0);
            let k = if self.pixel() { (k * 10.0).floor() / 10.0 } else { k };
            if k > 0.0 {
                gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, k]);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn knife_toss(gfx: &mut Gfx, assets: &Assets, level: Option<&str>, path: &str, prefix: &str, x: f32, y: f32, scale: f32) -> Option<Actor> {
    let data: funkin_core::stage::AnimationData =
        serde_json::from_value(serde_json::json!({ "name": "throw", "prefix": prefix, "frameRate": 24, "looped": false })).ok()?;
    match Actor::animated(gfx, assets, "deathSpriteNene", "sparrow", path, level, &[data], scale == 1.0) {
        Ok(mut a) => {
            a.x = x;
            a.y = y;
            a.scale = [scale, scale];
            Some(a)
        }
        Err(e) => {
            log::warn!("nene knife toss: {e}");
            None
        }
    }
}
