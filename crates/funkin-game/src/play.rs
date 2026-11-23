//! The gameplay screen (port of the visual side of `PlayState` / `Strumline`):
//! strumlines, scrolling notes and holds, countdown, rating popups, health bar.

use std::sync::Arc;
use std::time::Instant;

use funkin_core::constants::*;
use funkin_core::play::*;
use funkin_core::scoring::Judgement;
use funkin_core::song::SongRegistry;
use funkin_core::state::PlayRequest;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::text::Font;
use crate::gfx::{Affine, Gfx, TexId, VIEW_H, VIEW_W, WHITE};
use crate::gfx::ttf::TtfFont;
use crate::health_icon::HealthIcon;
use crate::world::{CharType, World};

// Layout constants from Strumline.hx / Constants.hx.
const STRUMLINE_SIZE: f32 = 104.0;
const NOTE_SPACING: f32 = STRUMLINE_SIZE + 8.0;
const STRUMLINE_X_OFFSET: f32 = 48.0;
const STRUMLINE_Y_OFFSET: f32 = 24.0;
/// `Strumline.NUDGE`: notes sit 2 px left of the lane centre.
const NOTE_NUDGE: f32 = 2.0;
const ANIM_FPS: f32 = 24.0;
/// `Strumline.INITIAL_OFFSET`.
const RECEPTOR_INITIAL_OFFSET: f32 = -0.275 * STRUMLINE_SIZE;

pub use crate::notestyle::NoteSkin;

/// Display preferences that shape the gameplay HUD.
#[derive(Debug, Clone, Copy, Default)]
pub struct PlayPrefs {
    pub downscroll: bool,
    pub middlescroll: bool,
    pub hide_opponent: bool,
    /// Key presses with no note to hit aren't penalized.
    pub ghost_tapping: bool,
    /// Psych Engine's time bar.
    pub psych_time_bar: bool,
    /// Psych Engine's health bar.
    pub psych_health_bar: bool,
    /// Strumline background opacity, 0..1.
    pub strumline_bg: f32,
    pub zoom_camera: bool,
    pub subtitles: bool,
}

/// One SRT subtitle (`SubtitleEntry`).
struct Subtitle {
    start: f64,
    end: f64,
    text: String,
}

/// `SRTParser.parseFromString`, with markup tags dropped.
fn parse_srt(s: &str) -> Vec<Subtitle> {
    let s = s.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let time = |t: &str| -> Option<f64> {
        let t = t.trim().replace(',', ".");
        let mut parts = t.split(':');
        let h: f64 = parts.next()?.trim().parse().ok()?;
        let m: f64 = parts.next()?.trim().parse().ok()?;
        let sec: f64 = parts.next()?.trim().parse().ok()?;
        Some(((h * 60.0 + m) * 60.0 + sec) * 1000.0)
    };
    let strip = |l: &str| {
        let mut out = String::new();
        let mut depth = 0;
        for c in l.chars() {
            match c {
                '<' | '{' => depth += 1,
                '>' | '}' if depth > 0 => depth -= 1,
                _ if depth == 0 => out.push(c),
                _ => {}
            }
        }
        out
    };
    let mut out = Vec::new();
    for block in s.split("\n\n") {
        let lines: Vec<&str> = block.trim().lines().collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else { continue };
        let Some((a, b)) = lines[ti].split_once("-->") else { continue };
        let (Some(start), Some(end)) = (time(a), time(b)) else { continue };
        let text = lines[ti + 1..].iter().map(|l| strip(l)).collect::<Vec<_>>().join("\n");
        out.push(Subtitle { start, end, text });
    }
    out
}

/// Receptor animation state (`StrumlineNote`).
#[derive(Clone, Copy, PartialEq)]
enum StrumAnim {
    Static,
    Press,
    Confirm { since: f64 },
    /// `confirm-hold`: the confirm frames replayed once, then held, while sustaining.
    ConfirmHold { since: f64 },
}

/// `StrumlineNote.CONFIRM_HOLD_TIME`, in ms.
const CONFIRM_HOLD_MS: f64 = 150.0;

#[derive(Clone, Copy, PartialEq)]
enum CoverPhase {
    Start,
    Hold,
    End,
}

/// A `NoteHoldCover` glowing over a receptor while a sustain is held.
#[derive(Clone, Copy)]
struct Cover {
    phase: CoverPhase,
    since: f64,
}

/// One `PopUpStuff` sprite (a rating or a combo digit): Flixel motion, then
/// an alpha tween to 0 over 0.2 s after `delay`.
struct Popup {
    tex: TexId,
    scale: f32,
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    ay: f32,
    age: f32,
    delay: f32,
    /// Pixel styles: a two-step fade (`EaseUtil.stepped(2)`) and whole-pixel positions.
    pixel: bool,
}

/// The rain on Weekend 1's stages (`phillyStreets.hxc` / `phillyStreetsErect.hxc`).
struct Rain {
    /// Intensity at the song's start and end (it's remapped over the song).
    start: f32,
    end: f32,
    color: [f32; 3],
    /// `RuntimeRainShader.time` (starts at 1).
    time: f32,
}

impl Rain {
    fn for_stage(stage: &str, song: &str) -> Option<Self> {
        let erect = match stage {
            "phillyStreets" => false,
            "phillyStreetsErect" => true,
            _ => return None,
        };
        let (start, end) = match song {
            "darnell" => (0.0, 0.1),
            "lit-up" => (0.1, 0.2),
            "2hot" => (0.2, 0.4),
            _ => (0.0, 0.0),
        };
        // Erect: a tenth of the intensity, grey rain.
        let (k, rgb) = if erect { (0.1, 0xa8adb5u32) } else { (1.0, 0x6680cc) };
        let c = |sh: u32| ((rgb >> sh) & 0xff) as f32 / 255.0;
        Some(Self { start: start * k, end: end * k, color: [c(16), c(8), c(0)], time: 1.0 })
    }

    fn params(&self, cam: &funkin_core::camera::GameCamera, pos_ms: f64, len_ms: f64) -> crate::gfx::rain::RainParams {
        // FlxMath.remapToRange(songPosition, 0, music.length, start, end)
        let t = if len_ms > 0.0 { (pos_ms / len_ms) as f32 } else { 0.0 };
        let intensity = self.start + (self.end - self.start) * t;
        // FlxCamera.viewLeft/Top/Right/Bottom.
        let z = cam.zoom();
        let (w, h) = (cam.view_w / z, cam.view_h / z);
        let left = cam.scroll[0] + (cam.view_w - w) / 2.0;
        let top = cam.scroll[1] + (cam.view_h - h) / 2.0;
        crate::gfx::rain::RainParams {
            bounds: [left as f32, top as f32, (left + w) as f32, (top + h) as f32],
            color: self.color,
            scale: VIEW_H / 200.0,
            intensity,
            time: self.time,
        }
    }
}

/// `EaseUtil.stepped(steps)`.
fn stepped(t: f32, steps: f32) -> f32 {
    (t * steps).floor() / steps
}

impl Popup {
    fn alpha(&self) -> f32 {
        let k = ((self.age - self.delay) / 0.2).clamp(0.0, 1.0);
        1.0 - if self.pixel { stepped(k, 2.0) } else { k }
    }

    /// `FlxObject.updateMotion` (half the acceleration before and after the move).
    fn update(&mut self, dt: f32) {
        self.age += dt;
        let dv = 0.5 * self.ay * dt;
        self.vy += dv;
        self.x += self.vx * dt;
        self.y += self.vy * dt;
        self.vy += dv;
    }
}

/// How a play session ended.
pub enum PlayOutcome {
    Running,
    Finished,
    Died,
}

pub struct PlayScene {
    pub request: PlayRequest,
    pub session: PlaySession,
    events: Vec<PlayEvent>,
    player_voices: Vec<Arc<Track>>,
    song_length_ms: f64,
    offset_ms: f64,
    scroll_speed: f64,
    beat_ms: f64,
    song_pos: f64,
    strums: [[StrumAnim; 4]; 2],
    /// `confirmHoldTimer` start (song ms), per receptor.
    strum_timer: [[Option<f64>; 4]; 2],
    covers: [[Option<Cover>; 4]; 2],
    keys_held: [bool; 4],
    world: Option<World>,
    /// Instrumental, sampled for the A-Bot visualizer.
    inst: Arc<Track>,
    /// Textures loaded after this belong to the song (stage, characters).
    pub tex_mark: usize,
    /// [`Audio::play`]'s id for this song, and the time it starts from.
    audio_id: u64,
    start_ms: f64,
    last_step: i64,
    /// `iconP1` / `iconP2`.
    icons: [Option<HealthIcon>; 2],
    /// The health bar's displayed value (`PlayState.healthLerp`).
    health_lerp: f64,
    icon_step: i64,
    last_frame: Option<Instant>,
    /// Live rating/combo popups, oldest first.
    popups: Vec<Popup>,
    /// The combo before this frame's events (for the combo-break \"000\").
    popup_combo: u32,
    last_countdown_beat: i64,
    countdown_shown: Option<(usize, f64)>,
    /// Weekend 1's rain shader (Philly Streets stages only).
    rain: Option<Rain>,
    /// The game over screen, once the player has died.
    death: Option<crate::game_over::GameOver>,
    /// Retrying after a game over: the game camera fades in from black
    /// (elapsed seconds, and whether in RetroCameraFade's 10 steps).
    intro_fade: Option<(f32, bool)>,
    rng: u64,
    pub title: String,
    pub song_name: String,
    pub artist: String,
    pub charter: Option<String>,
    /// The song's note style (`playData.noteStyle`).
    pub note_style: String,
    /// The chart's `stickerPack`, if any.
    pub sticker_pack: Option<String>,
    /// The player character id.
    pub player: String,
    /// The opponent's character id (for the game-over quote).
    pub opponent: String,
    /// Difficulties of this variation (pause menu).
    pub difficulties: Vec<String>,
    inst_offset_ms: f64,
    prefs: PlayPrefs,
    subtitles: Vec<Subtitle>,
    /// RESET was pressed: die on the next update.
    reset: bool,
}

/// The note direction for a lane (0 = left .. 3 = right).
pub fn lane_direction(lane: usize) -> NoteDirection {
    [NoteDirection::Left, NoteDirection::Down, NoteDirection::Up, NoteDirection::Right][lane.min(3)]
}

impl PlayScene {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gfx: &mut Gfx,
        assets: &Assets,
        registry: &SongRegistry,
        audio: &Audio,
        request: PlayRequest,
        global_offset_ms: f64,
        botplay: bool,
        prefs: PlayPrefs,
        practice: bool,
    ) -> Result<Self, String> {
        let song = registry
            .load_song(&request.song_id)
            .map_err(|e| e.to_string())?;
        let var = song
            .variation_for_difficulty(&request.difficulty, Some(&request.variation))
            .ok_or_else(|| format!("{} has no {} chart", request.song_id, request.difficulty))?;
        let meta = &var.metadata;
        let notes = var
            .chart
            .notes(&request.difficulty)
            .ok_or("no notes for difficulty")?;
        let scroll_speed = var.chart.scroll_speed(&request.difficulty);

        let config = PlayConfig {
            scroll_speed,
            botplay,
            practice,
            ghost_tapping: prefs.ghost_tapping,
            ..Default::default()
        };
        let session =
            PlaySession::new(config, &meta.time_changes, notes, &var.chart.events, |_| {
                NoteKindFlags::default()
            });

        let inst_id = &meta.play_data.characters.instrumental;
        let inst = Track::new(Pcm::load_ogg(
            &registry.inst_path(&request.song_id, inst_id),
        )?);
        let song_length_ms = inst.pcm.duration_ms();
        let inst_track = inst.clone();
        let load = |paths: Vec<std::path::PathBuf>| -> Result<Vec<Arc<Track>>, String> {
            paths
                .iter()
                .map(|p| Pcm::load_ogg(p).map(Track::new))
                .collect()
        };
        let player_voices = load(registry.player_voices(&request.song_id, meta))?;
        let opponent_voices = load(registry.opponent_voices(&request.song_id, meta))?;

        // Stage and characters (art only; failures leave a plain background).
        let tex_mark = gfx.texture_mark();
        let chars = &meta.play_data.characters;
        let icons = [
            HealthIcon::load(gfx, assets, &chars.player, true),
            HealthIcon::load(gfx, assets, &chars.opponent, false),
        ];
        let mut world = match World::load(
            gfx,
            assets,
            &meta.play_data.stage,
            &chars.player,
            &chars.opponent,
            &chars.girlfriend,
        ) {
            Ok(w) => Some(w),
            Err(e) => {
                log::warn!("stage {}: {e}", meta.play_data.stage);
                None
            }
        };

        if let Some(w) = &mut world {
            w.zoom_camera = prefs.zoom_camera;
        }
        // Subtitles: `songs/<id>/subtitles/song-lyrics[-<variation>].srt`.
        let subtitles = if prefs.subtitles {
            let suffix = if request.variation.is_empty() || request.variation == "default" { String::new() } else { format!("-{}", request.variation) };
            let p = assets.root().join("data").join("songs").join(&request.song_id).join("subtitles").join(format!("song-lyrics{suffix}.srt"));
            funkin_core::vfs::read_to_string(&p).map(|s| parse_srt(&s)).unwrap_or_default()
        } else {
            Vec::new()
        };
        let difficulties = meta.play_data.difficulties.clone();
        let inst_offset_ms = meta.instrumental_offset(Some(inst_id).filter(|s| !s.is_empty()).map(|s| s.as_str()));

        let beat_ms = session.conductor.beat_length_ms();
        let mut tracks = vec![inst];
        tracks.extend(player_voices.iter().cloned());
        tracks.extend(opponent_voices);
        // Five beats of lead-in for the countdown, as in `PlayState.startCountdown`.
        let start_ms = -beat_ms * 5.0;
        let audio_id = audio.play(tracks, start_ms);

        let title = format!(
            "{} ({}) - {}",
            meta.song_name, request.difficulty, meta.artist
        );
        let rain = Rain::for_stage(&meta.play_data.stage, &request.song_id);
        Ok(Self {
            request,
            session,
            events: Vec::new(),
            player_voices,
            song_length_ms,
            offset_ms: inst_offset_ms + global_offset_ms,
            scroll_speed,
            beat_ms,
            song_pos: -beat_ms * 5.0,
            strums: [[StrumAnim::Static; 4]; 2],
            strum_timer: [[None; 4]; 2],
            covers: [[None; 4]; 2],
            keys_held: [false; 4],
            world,
            inst: inst_track,
            tex_mark,
            audio_id,
            start_ms,
            last_step: i64::MIN,
            icons,
            health_lerp: HEALTH_STARTING,
            icon_step: i64::MIN,
            last_frame: None,
            popups: Vec::new(),
            popup_combo: 0,
            last_countdown_beat: i64::MIN,
            countdown_shown: None,
            rain,
            death: None,
            intro_fade: None,
            rng: 0x9E37_79B9_7F4A_7C15,
            title,
            song_name: meta.song_name.clone(),
            artist: meta.artist.clone(),
            charter: meta.charter.clone(),
            note_style: if meta.play_data.note_style.is_empty() { "funkin".into() } else { meta.play_data.note_style.clone() },
            sticker_pack: meta.play_data.sticker_pack.clone(),
            player: chars.player.clone(),
            opponent: chars.opponent.clone(),
            difficulties,
            inst_offset_ms,
            prefs,
            subtitles,
            reset: false,
        })
    }

    /// The player died: open the game over screen. False if there's no stage
    /// to show it on (the plain game over screen is used then).
    pub fn start_death(&mut self, gfx: &mut Gfx, assets: &Assets, naughty: bool) -> bool {
        let Some(w) = &mut self.world else { return false };
        self.popups.clear();
        self.death = Some(crate::game_over::GameOver::new(gfx, assets, w, &self.opponent, naughty));
        true
    }

    pub fn death_update(&mut self, dt: f32, audio: &Audio, assets: &Assets) -> crate::game_over::DeathAction {
        match (&mut self.death, &mut self.world) {
            (Some(d), Some(w)) => d.update(dt, w, audio, assets),
            _ => crate::game_over::DeathAction::None,
        }
    }

    pub fn death_confirm(&mut self, audio: &Audio, assets: &Assets) {
        if let (Some(d), Some(w)) = (&mut self.death, &mut self.world) {
            d.confirm(w, audio, assets);
        }
    }

    /// Whether Back may leave the game over screen (`goBack`'s checks).
    pub fn death_can_go_back(&self) -> bool {
        self.death.as_ref().is_none_or(|d| d.can_go_back())
    }

    pub fn death_stop(&mut self, audio: &Audio) {
        if let Some(d) = &mut self.death {
            d.stop(audio);
        }
    }

    pub fn has_death(&self) -> bool {
        self.death.is_some()
    }

    pub fn draw_death(&self, gfx: &mut Gfx) {
        if let (Some(d), Some(w)) = (&self.death, &self.world) {
            d.draw(gfx, w);
        }
    }

    /// Fade the game camera in from black (after a retry).
    pub fn fade_in(&mut self, pixel: bool) {
        self.intro_fade = Some((0.0, pixel));
    }

    /// The global offset changed (pause menu): applies immediately.
    pub fn set_global_offset(&mut self, ms: f64) {
        self.offset_ms = self.inst_offset_ms + ms;
    }

    /// Practice mode from the pause menu: no more deaths.
    pub fn set_practice(&mut self, on: bool) {
        self.session.config.practice = on;
    }

    pub fn practice(&self) -> bool {
        self.session.config.practice
    }

    /// RESET: die straight away (`PlayState`: health = HEALTH_MIN).
    pub fn reset_death(&mut self) {
        self.reset = true;
    }

    /// Strumline layout: the centre x of each lane per side (None = hidden)
    /// and each side's alpha.
    fn layout(&self) -> ([[Option<f32>; 4]; 2], [f32; 2]) {
        let p = &self.prefs;
        let lane = |base: f32, i: usize| base + i as f32 * NOTE_SPACING + STRUMLINE_SIZE / 2.0;
        let mut xs = [[None; 4]; 2];
        let mut alpha = [1.0, 1.0];
        if p.middlescroll {
            let base = (VIEW_W - 4.0 * NOTE_SPACING) / 2.0;
            for i in 0..4 {
                xs[0][i] = Some(lane(base, i));
                // The opponent's lanes split to either side.
                xs[1][i] = Some(if i < 2 {
                    lane(STRUMLINE_X_OFFSET, i)
                } else {
                    lane(VIEW_W - STRUMLINE_X_OFFSET - 4.0 * NOTE_SPACING, i)
                });
            }
            alpha[1] = 0.35;
        } else {
            for i in 0..4 {
                xs[0][i] = Some(lane(VIEW_W / 2.0 + STRUMLINE_X_OFFSET, i));
                xs[1][i] = Some(lane(STRUMLINE_X_OFFSET, i));
            }
        }
        if p.hide_opponent {
            xs[1] = [None; 4];
        }
        (xs, alpha)
    }

    /// The strumlines' y (`PlayState`): `STRUMLINE_Y_OFFSET` from the top, or
    /// in downscroll `height - strumline.height - STRUMLINE_Y_OFFSET - offsets[1]`,
    /// where the strumline's height is its receptors' hitbox height.
    ///
    /// With the Psych Engine time bar on, Psych's placement is used instead:
    /// the receptors' top at 50, or `height - 150` in downscroll.
    fn strum_y(&self, skin: &NoteSkin) -> f32 {
        if self.prefs.psych_time_bar {
            let top = if self.prefs.downscroll { VIEW_H - 150.0 } else { 50.0 };
            return top - skin.strum_off[1];
        }
        if self.prefs.downscroll {
            VIEW_H - skin.strum_size.1 - STRUMLINE_Y_OFFSET - skin.strum_off[1]
        } else {
            STRUMLINE_Y_OFFSET
        }
    }

    fn song_time(&self, audio: &Audio, at: Instant) -> f64 {
        // Until the audio thread picks the song up, hold at the start.
        if !audio.started(self.audio_id) {
            return self.start_ms + self.offset_ms;
        }
        audio.song_time_at(at) + self.offset_ms
    }

    pub fn key_down(&mut self, audio: &Audio, dir: NoteDirection, at: Instant) {
        let t = self.song_time(audio, at);
        self.session.press(dir, t);
        self.keys_held[dir.index()] = true;
        self.strums[0][dir.index()] = StrumAnim::Press;
        self.strum_timer[0][dir.index()] = None;
    }

    pub fn key_up(&mut self, dir: NoteDirection) {
        self.session.release(dir);
        self.keys_held[dir.index()] = false;
        self.strums[0][dir.index()] = StrumAnim::Static;
        self.strum_timer[0][dir.index()] = None;
    }

    /// Is a sustain currently being held in `lane` on `side` (0 = player)?
    fn hold_active(&self, side: usize, lane: usize, pos: f64) -> bool {
        let line = if side == 0 { &self.session.player } else { &self.session.opponent };
        line.notes[line.cursor()..]
            .iter()
            .take_while(|n| n.strum_time() <= pos)
            .any(|n| {
                n.direction.index() == lane
                    && n.hold.as_ref().is_some_and(|h| h.alive && h.hit_note && !h.missed_note)
            })
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    fn rand_int(&mut self, lo: i32, hi: i32) -> f32 {
        (lo + (self.rand() % (hi - lo + 1) as u64) as i32) as f32
    }

    fn rand_float(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.rand() >> 40) as f32 / (1u64 << 24) as f32 * (hi - lo)
    }

    /// `applyScore` with `isComboBreak`: a broken combo of 10+ shows "000".
    fn combo_break(&mut self, skin: &NoteSkin) {
        if self.popup_combo >= 10 {
            self.display_combo(skin, 0);
        }
        self.popup_combo = 0;
    }

    /// `PopUpStuff.displayRating` (no style offsets in the base game's styles).
    fn display_rating(&mut self, skin: &NoteSkin, j: Judgement) {
        let idx = match j {
            Judgement::Sick => 0,
            Judgement::Good => 1,
            Judgement::Bad => 2,
            Judgement::Shit | Judgement::Miss => 3,
        };
        let tex = skin.ratings[idx];
        let (w, h) = skin.rating_sizes[idx];
        let scale = skin.rating_scale;
        let (w, h) = ((w * scale).abs(), (h * scale).abs());
        let vy = -self.rand_int(140, 175);
        let vx = -self.rand_int(0, 10);
        self.popups.push(Popup {
            tex,
            scale,
            x: VIEW_W * 0.474 - w / 2.0,
            y: VIEW_H * 0.45 - 60.0 - h / 2.0,
            vx,
            vy,
            ay: 550.0,
            age: 0.0,
            delay: (self.beat_ms * 0.001) as f32,
            pixel: skin.popup_pixel,
        });
    }

    /// `PopUpStuff.displayCombo`: at least three digits, ones digit rightmost.
    fn display_combo(&mut self, skin: &NoteSkin, combo: u32) {
        let mut digits = Vec::new();
        let mut c = combo;
        while c != 0 {
            digits.push((c % 10) as usize);
            c /= 10;
        }
        while digits.len() < 3 {
            digits.push(0);
        }
        for (i, d) in digits.into_iter().enumerate() {
            let ay = self.rand_int(250, 300);
            let vy = -self.rand_int(130, 150);
            let vx = self.rand_float(-5.0, 5.0);
            self.popups.push(Popup {
                tex: skin.digits[d],
                scale: skin.digit_scale,
                x: VIEW_W * 0.507 - 36.0 * (i + 1) as f32 - 65.0,
                y: VIEW_H * 0.44,
                vx,
                vy,
                ay,
                age: 0.0,
                delay: (self.beat_ms * 0.002) as f32,
                pixel: skin.popup_pixel,
            });
        }
    }

    fn miss_sound(&mut self, skin: &NoteSkin, audio: &Audio, lo: f32, hi: f32) {
        if skin.miss_sfx.is_empty() {
            return;
        }
        let r = self.rand();
        let pcm = skin.miss_sfx[(r % skin.miss_sfx.len() as u64) as usize].clone();
        let vol = lo + (hi - lo) * ((r >> 32) % 1000) as f32 / 1000.0;
        audio.play_sfx(pcm, vol);
    }

    fn set_player_vocals(&self, v: f32) {
        for t in &self.player_voices {
            t.set_volume(v);
        }
    }

    pub fn update(&mut self, audio: &Audio, skin: &NoteSkin) -> PlayOutcome {
        let now = Instant::now();
        self.song_pos = self.song_time(audio, now);
        let pos = self.song_pos;

        // Countdown (beats -4..-1 before the song starts).
        if pos < 0.0 {
            let beat = (pos / self.beat_ms).floor() as i64;
            if beat != self.last_countdown_beat && (-4..=-1).contains(&beat) {
                self.last_countdown_beat = beat;
                let step = (beat + 4) as usize;
                if let Some(s) = &skin.countdown_sfx[step] {
                    audio.play_sfx(s.clone(), 0.6);
                }
                self.countdown_shown = (step > 0).then_some((step - 1, pos));
            }
        }

        let mut events = std::mem::take(&mut self.events);
        self.session.update(pos, &mut events);
        let mut outcome = PlayOutcome::Running;
        if std::mem::take(&mut self.reset) {
            outcome = PlayOutcome::Died;
        }
        let botplay = self.session.config.botplay;
        let step_ms = self.session.conductor.step_length_ms();
        for e in events.drain(..) {
            match e {
                PlayEvent::NoteHit { side, index, direction, judgement, combo, .. } => {
                    let s = if side == Side::Player { 0 } else { 1 };
                    let lane = direction.index();
                    self.strums[s][lane] = StrumAnim::Confirm { since: pos };
                    // playConfirm: the opponent (and botplay) release after a timer.
                    self.strum_timer[s][lane] = (s == 1 || botplay).then_some(pos);
                    let line = if s == 0 { &self.session.player } else { &self.session.opponent };
                    if line.notes[index].hold.as_ref().is_some_and(|h| h.full_sustain_length > 0.0) {
                        self.covers[s][lane] = Some(Cover { phase: CoverPhase::Start, since: pos });
                    }
                    if let Some(w) = &mut self.world {
                        let kind = if s == 0 { CharType::Bf } else { CharType::Dad };
                        if let Some(c) = w.char_mut(kind) {
                            c.sing(direction, false);
                        }
                        if s == 0 && matches!(judgement, Some(Judgement::Sick | Judgement::Good)) {
                            if let Some(gf) = w.char_mut(CharType::Gf) {
                                gf.combo(combo);
                            }
                        }
                    }
                    if s == 0 {
                        self.set_player_vocals(1.0);
                        if let Some(j) = judgement {
                            if combo == 0 {
                                // A bad/shit that broke the combo (`applyScore`).
                                self.combo_break(skin);
                            }
                            self.display_rating(skin, j);
                            if combo >= 10 {
                                self.display_combo(skin, combo);
                            }
                        }
                        self.popup_combo = combo;
                    }
                }
                PlayEvent::NoteMiss { direction, .. } => {
                    self.combo_break(skin);
                    self.set_player_vocals(0.0);
                    self.miss_sound(skin, audio, 0.5, 0.6);
                    self.bf_miss(direction);
                }
                PlayEvent::GhostMiss { direction, penalized: true, .. } => {
                    self.combo_break(skin);
                    self.set_player_vocals(0.0);
                    self.miss_sound(skin, audio, 0.1, 0.2);
                    self.bf_miss(direction);
                }
                PlayEvent::HoldDropped { direction, penalized, .. } => {
                    self.covers[0][direction.index()] = None;
                    if penalized {
                        self.combo_break(skin);
                        self.set_player_vocals(0.0);
                        self.miss_sound(skin, audio, 0.5, 0.6);
                        self.bf_miss(direction);
                    }
                }
                PlayEvent::HoldFinished { side, direction, .. } => {
                    let lane = direction.index();
                    let s = if side == Side::Player { 0 } else { 1 };
                    self.strums[s][lane] = if s == 0 && !botplay && self.keys_held[lane] {
                        StrumAnim::Press
                    } else {
                        StrumAnim::Static
                    };
                    self.strum_timer[s][lane] = None;
                    // The player cover plays its end animation; the CPU one just vanishes.
                    if s == 0 {
                        if let Some(c) = &mut self.covers[0][lane] {
                            *c = Cover { phase: CoverPhase::End, since: pos };
                        }
                    } else {
                        self.covers[1][lane] = None;
                    }
                }
                PlayEvent::ChartEvent(i) => {
                    if let Some(w) = &mut self.world {
                        w.handle_event(&self.session.events[i], step_ms);
                    }
                }
                PlayEvent::Died => outcome = PlayOutcome::Died,
                _ => {}
            }
        }
        self.events = events;

        // Receptors (`Strumline.holdConfirm` / `StrumlineNote.update`).
        let confirm_len = skin.confirms[0].duration(ANIM_FPS) as f64 * 1000.0;
        for side in 0..2 {
            for lane in 0..4 {
                if self.hold_active(side, lane, pos) {
                    match self.strums[side][lane] {
                        StrumAnim::ConfirmHold { .. } => {}
                        StrumAnim::Confirm { since } => {
                            if pos - since >= confirm_len {
                                self.strums[side][lane] = StrumAnim::ConfirmHold { since: pos };
                                self.strum_timer[side][lane] = None;
                            }
                        }
                        _ => self.strums[side][lane] = StrumAnim::Confirm { since: pos },
                    }
                } else if let StrumAnim::Confirm { since } = self.strums[side][lane] {
                    // onAnimationFinished starts the player release timer.
                    if side == 0 && self.strum_timer[0][lane].is_none() && pos - since >= confirm_len {
                        self.strum_timer[0][lane] = Some(since + confirm_len);
                    }
                }
                if let Some(t) = self.strum_timer[side][lane] {
                    if pos - t >= CONFIRM_HOLD_MS {
                        self.strum_timer[side][lane] = None;
                        self.strums[side][lane] = StrumAnim::Static;
                    }
                }
                // Hold covers advance start -> hold, and vanish after the end animation.
                if let (Some(c), Some((_, anims))) = (self.covers[side][lane], skin.covers.get(lane)) {
                    let age = ((pos - c.since) / 1000.0) as f32;
                    match c.phase {
                        CoverPhase::Start if age >= anims[0].duration(ANIM_FPS) => {
                            self.covers[side][lane] = Some(Cover { phase: CoverPhase::Hold, since: pos });
                        }
                        CoverPhase::End if age >= anims[2].duration(ANIM_FPS) => {
                            self.covers[side][lane] = None;
                        }
                        _ => {}
                    }
                }
            }
        }

        // World: characters dance on every step, camera bops, animations advance.
        let elapsed = self
            .last_frame
            .map(|t| now.duration_since(t).as_secs_f64().min(0.1))
            .unwrap_or(0.0);
        self.last_frame = Some(now);
        if let Some(r) = &mut self.rain {
            r.time += elapsed as f32;
        }
        if let Some((t, _)) = &mut self.intro_fade {
            *t += elapsed as f32;
            if *t >= 1.0 {
                self.intro_fade = None;
            }
        }
        for p in &mut self.popups {
            p.update(elapsed as f32);
        }
        self.popups.retain(|p| p.age < p.delay + 0.2);
        self.popup_combo = self.session.tallies.combo;
        let step = self.session.conductor.current_step;
        let holding = [
            (0..4).any(|l| self.hold_active(0, l, pos)),
            (0..4).any(|l| self.hold_active(1, l, pos)),
        ];
        if let Some(w) = &mut self.world {
            if self.last_step == i64::MIN {
                self.last_step = step - 1;
            }
            let from = (self.last_step + 1).max(step - 16);
            for s in from..=step {
                w.on_step(s);
            }
            w.update(elapsed, step_ms, holding, self.keys_held.iter().any(|&k| k));
            w.update_visualizer(&self.inst.pcm, pos);
        }
        self.last_step = self.last_step.max(step);
        // Health bar: `updateHealthBar` (a per-frame lerp, as in the original) and the icons.
        self.health_lerp = if botplay { HEALTH_MAX } else { self.health_lerp + (self.session.health - self.health_lerp) * 0.15 };
        if self.icon_step == i64::MIN {
            self.icon_step = step - 1;
        }
        let from = (self.icon_step + 1).max(step - 16);
        for icon in self.icons.iter_mut().flatten() {
            for s in from..=step {
                icon.on_step(s, step_ms);
            }
            icon.update(elapsed as f32, self.session.health);
        }
        self.icon_step = self.icon_step.max(step);
        if matches!(outcome, PlayOutcome::Running) && pos > self.song_length_ms {
            outcome = PlayOutcome::Finished;
        }
        outcome
    }

    fn bf_miss(&mut self, dir: NoteDirection) {
        if let Some(bf) = self.world.as_mut().and_then(|w| w.char_mut(CharType::Bf)) {
            bf.sing(dir, true);
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, skin: &NoteSkin, font: &Font, vcr: Option<&TtfFont>) {
        match &self.world {
            Some(w) => {
                gfx.begin([0.0, 0.0, 0.0, 1.0]);
                w.draw(gfx);
                if let Some((t, pixel)) = self.intro_fade {
                    // FlxG.camera.fade(BLACK, 1, true) after a retry.
                    let k = if pixel { (t * 10.0).floor() / 10.0 } else { t };
                    gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, 1.0 - k.clamp(0.0, 1.0)]);
                }
                if let Some(r) = &self.rain {
                    gfx.rain(r.params(&w.camera, self.song_pos, self.song_length_ms));
                }
                let z = w.camera.hud_zoom as f32;
                gfx.set_transform(Affine::zoom_about(z, VIEW_W / 2.0, VIEW_H / 2.0));
            }
            None => gfx.begin([0.1, 0.1, 0.12, 1.0]),
        }
        self.draw_hud_layer(gfx, skin, font, vcr);
        gfx.set_transform(Affine::IDENTITY);
    }

    fn draw_hud_layer(&self, gfx: &mut Gfx, skin: &NoteSkin, font: &Font, vcr: Option<&TtfFont>) {
        let pos = self.song_pos;
        let (xs, side_alpha) = self.layout();
        let sign = if self.prefs.downscroll { -1.0 } else { 1.0 };
        let px_per_ms = (PIXELS_PER_MS * self.scroll_speed) as f32;
        let sy = self.strum_y(skin);
        // `Strumline.getXPos` + the strumline's x: each lane's left edge.
        let lane_x = |cx: f32| cx - STRUMLINE_SIZE / 2.0;

        // zIndex order: health bar 800, icons 850, combo popups 900, strumlines 1000+.
        self.draw_hud(gfx, skin, font, vcr);
        self.draw_popup(gfx, skin);

        // Strumline backgrounds (`Strumline.background`, 16 px padding).
        if self.prefs.strumline_bg > 0.0 {
            for side in [1usize, 0] {
                let (Some(first), Some(last)) = (xs[side][0], xs[side][3]) else { continue };
                if last - first > 4.0 * NOTE_SPACING {
                    continue; // split middlescroll lanes
                }
                let x = first - STRUMLINE_SIZE / 2.0 - 16.0;
                let a = self.prefs.strumline_bg * side_alpha[side];
                gfx.rect(x, 0.0, 4.0 * NOTE_SPACING + 32.0, VIEW_H, [0.0, 0.0, 0.0, a]);
            }
        }

        // StrumlineNote: placed at INITIAL_OFFSET + the style's offsets; every
        // animation is centred on the hitbox (`centerOffsets`).
        let (rw, rh) = skin.strum_size;
        let receptor_cy = sy + skin.strum_off[1] + rh / 2.0;
        let receptor_dx = RECEPTOR_INITIAL_OFFSET + skin.strum_off[0] + rw / 2.0;

        for side in [1usize, 0] {
            let line = if side == 0 {
                &self.session.player
            } else {
                &self.session.opponent
            };
            let sa = side_alpha[side];

            // Receptors.
            for lane in 0..4 {
                let Some(cx) = xs[side][lane] else { continue };
                let (anim, t) = match self.strums[side][lane] {
                    StrumAnim::Static => (&skin.statics[lane], 0.0),
                    StrumAnim::Press => (&skin.presses[lane], 0.0),
                    StrumAnim::Confirm { since } | StrumAnim::ConfirmHold { since } => {
                        (&skin.confirms[lane], ((pos - since) / 1000.0) as f32)
                    }
                };
                if let Some(f) = anim.frame_at(t, ANIM_FPS, false) {
                    skin.strum.draw_centered(gfx, f, lane_x(cx) + receptor_dx, receptor_cy, skin.strum_scale, [1.0, 1.0, 1.0, sa]);
                }
            }

            // Notes and hold trails within the visible window.
            let horizon = pos + (720.0 / px_per_ms as f64);
            let start = line.cursor().min(
                line.notes
                    .partition_point(|n| n.strum_time() < pos - 1000.0),
            );
            for n in line.notes[start..]
                .iter()
                .take_while(|n| n.strum_time() <= horizon)
            {
                let lane = n.direction.index();
                let Some(cx) = xs[side][lane] else { continue };
                let dist = |t: f64| ((t - pos) as f32) * px_per_ms;

                if let Some(h) = &n.hold {
                    // A dropped hold is hidden completely (`holdNote.alpha = 0`).
                    let dropped = h.hit_note && h.missed_note;
                    if h.alive && !dropped {
                        let end_t = n.strum_time() + h.full_sustain_length;
                        let near = if h.hit_note { dist(n.strum_time()).max(0.0) } else { dist(n.strum_time()) };
                        let far = dist(end_t);
                        if far > near {
                            // Strumline: y = strum.y - INITIAL_OFFSET + noteY + STRUMLINE_SIZE / 2
                            // (the trail's near end), drawn at minus the style's offsets.
                            let near_y = sy - RECEPTOR_INITIAL_OFFSET + sign * near + STRUMLINE_SIZE / 2.0 - skin.hold_off[1];
                            let x = cx - skin.hold_off[0];
                            draw_hold(gfx, skin, lane, x, near_y, far - near, sign, sa);
                        }
                    }
                }
                if !n.has_been_hit {
                    // NoteSprite: centred on the lane minus NUDGE, top at strum.y - INITIAL_OFFSET + noteY.
                    let y = sy - RECEPTOR_INITIAL_OFFSET + sign * dist(n.strum_time()) + skin.note_h[lane] / 2.0;
                    if y > -200.0 && y < VIEW_H + 200.0 {
                        skin.notes.draw_centered(gfx, skin.note_frames[lane], cx - NOTE_NUDGE, y, skin.note_scale, [1.0, 1.0, 1.0, sa]);
                    }
                }
            }
        }

        // Hold covers above the strumlines (`noteHoldCovers.zIndex = 40`).
        for side in [1usize, 0] {
            #[allow(clippy::needless_range_loop)]
            for lane in 0..4 {
                let (Some(c), Some((atlas, anims)), Some(cx)) = (self.covers[side][lane], skin.covers.get(lane), xs[side][lane]) else {
                    continue;
                };
                let (anim, looped) = match c.phase {
                    CoverPhase::Start => (&anims[0], false),
                    CoverPhase::Hold => (&anims[1], true),
                    CoverPhase::End => (&anims[2], false),
                };
                let t = ((pos - c.since) / 1000.0) as f32;
                let Some(f) = anim.frame_at(t, ANIM_FPS, looped) else { continue };
                // Strumline.playNoteHoldCover (the cover's top-left).
                let s = skin.cover_scale;
                let w = atlas.frames[0].frame_w * s;
                let x = lane_x(cx) + STRUMLINE_SIZE / 2.0 - w / 2.0 + skin.cover_off[0] * s - 12.0;
                let y = sy + RECEPTOR_INITIAL_OFFSET + STRUMLINE_SIZE / 2.0 + skin.cover_off[1] * s - 96.0;
                atlas.draw(gfx, f, x, y, s, [1.0, 1.0, 1.0, side_alpha[side]]);
            }
        }

        self.draw_countdown(gfx, skin);
    }

    /// `Subtitles`: lines active at the instrumental's time, VCR 30 on a
    /// half-transparent box, 139 px from the bottom (or the top in downscroll).
    pub fn draw_subtitles(&self, gfx: &mut Gfx, vcr: Option<&crate::gfx::ttf::TtfFont>) {
        let Some(vcr) = vcr else { return };
        let t = self.song_pos - self.offset_ms;
        let mut lines: Vec<&str> = Vec::new();
        for s in self.subtitles.iter().filter(|s| t >= s.start && t <= s.end) {
            for (i, l) in s.text.lines().enumerate() {
                lines.insert(i, l);
            }
        }
        if lines.is_empty() {
            return;
        }
        let line_h = 34.0;
        let h = lines.len() as f32 * line_h + 4.0;
        let w = lines.iter().map(|l| vcr.width(gfx, l, 30.0)).fold(0.0, f32::max) + 4.0;
        let y = if self.prefs.downscroll { 139.0 } else { VIEW_H - h - 139.0 };
        let x = (VIEW_W - w) / 2.0;
        gfx.rect(x, y, w, h, [0.0, 0.0, 0.0, 0.5]);
        for (i, l) in lines.iter().enumerate() {
            let lw = vcr.width(gfx, l, 30.0);
            vcr.draw(gfx, l, (VIEW_W - lw) / 2.0 - 2.0, y + i as f32 * line_h, 30.0, WHITE);
        }
    }

    fn draw_popup(&self, gfx: &mut Gfx, _skin: &NoteSkin) {
        for p in &self.popups {
            let a = p.alpha();
            if a > 0.0 {
                let (x, y) = if p.pixel { (p.x.round(), p.y.round()) } else { (p.x, p.y) };
                gfx.sprite(p.tex, x, y, p.scale, [1.0, 1.0, 1.0, a]);
            }
        }
    }

    fn draw_countdown(&self, gfx: &mut Gfx, skin: &NoteSkin) {
        let Some((i, shown_at)) = self.countdown_shown else {
            return;
        };
        let age = (self.song_pos - shown_at) / self.beat_ms;
        if !(0.0..1.0).contains(&age) {
            return;
        }
        if let Some(t) = skin.countdown[i] {
            // screenCenter() on the scaled sprite; pixel styles fade in 8 steps.
            let (w, h) = gfx.texture_size(t);
            let s = skin.countdown_scale;
            let fade = if skin.countdown_pixel { stepped(age as f32, 8.0) } else { cube_in_out(age as f32) };
            gfx.sprite(t, (VIEW_W - w * s) / 2.0, (VIEW_H - h * s) / 2.0, s, [1.0, 1.0, 1.0, 1.0 - fade]);
        }
    }

    /// Psych Engine's time bar (`timeBar` + `timeTxt`, "Time Left" mode): a
    /// 400x19 bar filling white with the song's progress, with the time left in
    /// VCR 32 over it. It fades in over 0.5 s (`circOut`) when the song starts.
    fn draw_psych_time_bar(&self, gfx: &mut Gfx, vcr: Option<&TtfFont>) {
        let pos = self.song_pos;
        if pos < 0.0 || self.song_length_ms <= 0.0 {
            return;
        }
        let t = (pos / 500.0).min(1.0) as f32;
        let alpha = (1.0 - (t - 1.0) * (t - 1.0)).sqrt();
        // timeTxt: x = STRUM_X + width / 2 - 248, y = 19 (height - 44 in downscroll).
        let txt_y = if self.prefs.downscroll { VIEW_H - 44.0 } else { 19.0 };
        let txt_h = vcr.map(|v| v.text_height(32.0, 1)).unwrap_or(40.0);
        let (bw, bh) = (400.0, 19.0);
        let bx = ((VIEW_W - bw) / 2.0).floor();
        let by = (txt_y + txt_h / 4.0).floor();
        let k = (pos / self.song_length_ms).clamp(0.0, 1.0) as f32;
        let (ix, iy, iw, ih) = (bx + 3.0, by + 3.0, bw - 6.0, bh - 6.0);
        gfx.rect(bx, by, bw, bh, [0.0, 0.0, 0.0, alpha]);
        gfx.rect(ix, iy, (iw * k).round(), ih, [1.0, 1.0, 1.0, alpha]);
        let Some(v) = vcr else { return };
        // FlxStringUtil.formatTime(seconds left, false): "m:ss".
        let left = ((self.song_length_ms - pos) / 1000.0).floor().max(0.0) as u64;
        let text = format!("{}:{:02}", left / 60, left % 60);
        let w = v.width(gfx, &text, 32.0);
        let x = 42.0 + VIEW_W / 2.0 - 248.0 + (400.0 - w) / 2.0 - 2.0;
        crate::menus::options::draw_outlined_alpha(gfx, v, &text, x, txt_y, 32.0, 2.0, alpha);
    }

    /// `healthBarBG`, `healthBar` (an `FlxBar`, RIGHT_TO_LEFT, 100 divisions),
    /// `scoreText` and the two health icons.
    fn draw_hud(&self, gfx: &mut Gfx, skin: &NoteSkin, font: &Font, vcr: Option<&TtfFont>) {
        if self.prefs.psych_time_bar {
            self.draw_psych_time_bar(gfx, vcr);
        }
        let (bw, bh) = gfx.texture_size(skin.health_bar);
        let frac = (self.health_lerp / HEALTH_MAX).clamp(0.0, 1.0) as f32;
        let (bx, by, ix, iy, iw) = if self.prefs.psych_health_bar {
            // Psych Engine's `Bar`: 601x19 at 89% of the height (11% in downscroll),
            // a 3 px black border, the opponent's colour on the left and the
            // player's on the right, from the icons' dominant colours.
            let (bw, bh): (f32, f32) = (601.0, 19.0);
            let by: f32 = (if self.prefs.downscroll { 720.0f32 * 0.11 } else { 720.0f32 * 0.89 }).floor();
            let bx = ((1280.0 - bw) / 2.0).floor();
            let (ix, iy, iw, ih) = (bx + 3.0, by + 3.0, bw - 6.0, bh - 6.0);
            gfx.rect(bx, by, bw, bh, [0.0, 0.0, 0.0, 1.0]);
            let color = |i: usize| self.icons[i].as_ref().map(|c| c.color).unwrap_or([161.0 / 255.0; 3]);
            let (p, o) = (color(0), color(1));
            let left = (iw * (1.0 - frac)).round();
            gfx.rect(ix, iy, left, ih, [o[0], o[1], o[2], 1.0]);
            gfx.rect(ix + left, iy, iw - left, ih, [p[0], p[1], p[2], 1.0]);
            (bx, by, ix, iy, iw)
        } else {
            let by = if self.prefs.downscroll { 720.0 * 0.1 } else { 720.0 * 0.9 };
            let bx = ((1280.0 - bw) / 2.0).floor();
            gfx.sprite(skin.health_bar, bx, by, 1.0, WHITE);
            let (ix, iy, iw, ih) = (bx + 4.0, by + 4.0, (bw - 8.0).floor(), (bh - 8.0).floor());
            gfx.rect(ix, iy, iw, ih, [1.0, 0.0, 0.0, 1.0]);
            let green = ((frac * 100.0).floor() * iw / 100.0).round();
            gfx.rect(ix + iw - green, iy, green, ih, [0.4, 1.0, 0.2, 1.0]);
            (bx, by, ix, iy, iw)
        };

        let score = if self.session.config.botplay {
            "Bot Play Enabled".to_owned()
        } else {
            format!("Score: {}", format_thousands(self.session.score()))
        };
        let (sx, sy) = (bx + bw - 190.0, by + 30.0);
        match vcr {
            Some(v) => crate::menus::options::draw_outlined_alpha(gfx, v, &score, sx, sy, 16.0, 1.0, 1.0),
            None => {
                font.draw(gfx, &score.to_uppercase(), sx, sy, 0.35, WHITE);
            }
        }
        for icon in self.icons.iter().flatten() {
            icon.draw(gfx, ix, iy, iw, self.health_lerp);
        }
    }
}

/// Port of `SustainTrail.updateClipping` + `draw`: a hold trail `len` px long
/// from `near_y` (the end at the note), extending down (`sign` 1, upscroll) or
/// up (-1, downscroll), centred on `cx`.
///
/// The graphic holds 8 columns, (body, end cap) per direction. The body tiles
/// along the trail ending flush with the cap; the cap starts `endOffset` of the
/// graphic's height before the trail's end and runs on to `bottomClip`. Pixel
/// styles use `endOffset = bottomClip = 1`, the others 0.5 and 0.9.
#[allow(clippy::too_many_arguments)]
fn draw_hold(gfx: &mut Gfx, skin: &NoteSkin, lane: usize, cx: f32, near_y: f32, len: f32, sign: f32, alpha: f32) {
    let (tw, th) = gfx.texture_size(skin.hold);
    let zoom = skin.hold_scale;
    let (end_offset, bottom_clip) = if skin.hold_pixel { (1.0, 1.0) } else { (0.5, 0.9) };
    let col_w = tw / 8.0;
    let w = col_w * zoom;
    let tile = th * zoom;
    let color = [1.0, 1.0, 1.0, alpha];
    let (x0, x1) = (cx - w / 2.0, cx + w / 2.0);
    let (u_body, u_cap) = (col_w * (lane * 2) as f32, col_w * (lane * 2 + 1) as f32);
    // From distance d0 to d1 along the trail, showing texture rows v0..v1 (0..1).
    let mut seg = |u: f32, d0: f32, d1: f32, v0: f32, v1: f32| {
        let (y0, y1) = (near_y + sign * d0, near_y + sign * d1);
        gfx.quad(skin.hold, [u, v0 * th, col_w, (v1 - v0) * th], [[x0, y0], [x1, y0], [x1, y1], [x0, y1]], color);
    };
    let bottom_h = tile * end_offset;
    let part = len - bottom_h;
    if part > 0.0 {
        // v = -(part - d) / tile with wrapping: whole tiles counted back from the cap.
        let mut end = part;
        while end > 0.0 {
            let start = (end - tile).max(0.0);
            seg(u_body, start, end, 1.0 - (end - start) / tile, 1.0);
            end = start;
        }
    }
    let cap_v0 = if part > 0.0 { 0.0 } else { (bottom_h - len) / tile };
    seg(u_cap, part.max(0.0), len + tile * (bottom_clip - end_offset), cap_v0, bottom_clip);
}

/// `FlxStringUtil.formatMoney(n, false, true)`: 1234567 -> "1,234,567".
fn format_thousands(n: i64) -> String {
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod hud_tests {
    use super::format_thousands;

    #[test]
    fn score_commas() {
        assert_eq!(format_thousands(0), "0");
        assert_eq!(format_thousands(999), "999");
        assert_eq!(format_thousands(1000), "1,000");
        assert_eq!(format_thousands(1234567), "1,234,567");
        assert_eq!(format_thousands(-12345), "-12,345");
    }
}

/// `FlxEase.cubeInOut`.
fn cube_in_out(t: f32) -> f32 {
    let t = t * 2.0;
    if t < 1.0 {
        t * t * t / 2.0
    } else {
        let t = t - 2.0;
        (t * t * t + 2.0) / 2.0
    }
}
