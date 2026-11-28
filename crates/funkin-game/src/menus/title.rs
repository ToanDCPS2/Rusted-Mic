//! Port of `TitleState`: the intro text sequence over `freakyMenu`, then the
//! bumping logo, dancing Girlfriend and "Press Enter to Begin".

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use winit::keyboard::KeyCode;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::Audio;
use crate::gfx::shader::{ShaderId, ShaderParams};
use crate::gfx::{Gfx, TexId, VIEW_H, VIEW_W, WHITE};
use crate::video::Video;

use super::{anim, draw_flash, random, screen_camera, Fonts, MenuAction, Music};

/// `freakyMenu` metadata BPM.
pub(crate) const MENU_BPM: f64 = 102.0;
/// `girlfriendsRingtone` metadata BPM.
const RINGTONE_BPM: f64 = 160.0;
/// `Constants.TITLE_ATTRACT_DELAY`.
const ATTRACT_DELAY: f32 = 37.5;
/// `moveToAttract`: music fade out and camera fade to black.
const ATTRACT_FADE: f32 = 2.0;
/// `AttractState.HOLD_TIME`: hold any key this long to skip the video.
const HOLD_TIME: f32 = 1.5;
/// `AttractState.VIDEO_PATHS`, played in turn.
const ATTRACT_VIDEOS: [&str; 3] = ["riftCollabTrailer", "mobileRelease", "boyfriendEverywhere"];
static NEXT_VIDEO: AtomicUsize = AtomicUsize::new(0);
/// `TitleState.initialized`: the first visit waits a second before the intro.
static INITIALIZED: AtomicBool = AtomicBool::new(false);
/// The cheat code: up, down, up, down, left, right, left, right.
const CHEAT: [KeyCode; 8] = [
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
];

/// `AttractState`: a trailer, skipped by holding any key.
struct Attract {
    video: Video,
    hold: f32,
    pie: Option<TexId>,
    pie_amount: f32,
}

enum Newgrounds {
    Still(TexId, f32),
    /// `newgrounds_logo_animated`: two 600 px frames at 4 fps.
    Animated(TexId),
}

pub struct Title {
    logo: Actor,
    gf: Actor,
    press: Actor,
    ng: Newgrounds,
    wacky: Vec<String>,
    lines: Vec<String>,
    ng_visible: bool,
    skipped: bool,
    last_beat: i64,
    dance_left: bool,
    time: f32,
    flash_at: Option<f32>,
    /// When Enter was pressed on the title screen.
    confirmed_at: Option<f32>,
    confirm: Option<std::sync::Arc<crate::audio::Pcm>>,
    /// Waiting out the first visit's 1 s before `startIntro`.
    intro_delay: f32,
    /// `swagShader` (`ColorSwap`) hue, in turns.
    hue: f32,
    held: [bool; 2],
    cheat_pos: usize,
    cheat_active: bool,
    cheat_pending: bool,
    /// The music changed: take the next beat as the start.
    beat_resync: bool,
    /// When the attract timer started (`None`: not running).
    attract_timer: Option<f32>,
    attract_started: bool,
    /// When `moveToAttract` began fading.
    attract_fade: Option<f32>,
    attract: Option<Attract>,
    /// Keys held down (`FlxG.keys.pressed.ANY`), for skipping the video.
    keys_down: HashSet<KeyCode>,
}

impl Title {
    pub fn new(gfx: &mut Gfx, assets: &Assets) -> Result<Self, String> {
        let mut logo = Actor::animated(gfx, assets, "logoBl", "sparrow", "default:logoBumpin", None, &[anim("bump", "logo bumpin", 24.0, true, &[])], true)?;
        logo.x = -150.0;
        logo.y = -100.0;
        logo.play("bump", true, false);

        let left: Vec<i32> = std::iter::once(30).chain(0..=14).collect();
        let right: Vec<i32> = (15..=29).collect();
        let mut gf = Actor::animated(
            gfx,
            assets,
            "gfDance",
            "sparrow",
            "default:gfDanceTitle",
            None,
            &[anim("danceLeft", "gfDance", 24.0, false, &left), anim("danceRight", "gfDance", 24.0, false, &right)],
            true,
        )?;
        gf.x = VIEW_W * 0.4;
        gf.y = VIEW_H * 0.07;

        let mut press = Actor::animated(
            gfx,
            assets,
            "titleText",
            "animateatlas",
            "default:title-screen-text",
            None,
            &[anim("idle", "Idle", 24.0, true, &[]), anim("press", "Confirm", 24.0, true, &[])],
            true,
        )?;
        press.x = 100.0;
        press.y = VIEW_H * 0.8;
        press.play("idle", true, false);

        let roll = random();
        let ng = if roll < 0.01 {
            Newgrounds::Still(gfx.load_texture(&assets.image_path("default:newgrounds_logo_classic"))?, 1.0)
        } else if roll < 0.31 {
            Newgrounds::Animated(gfx.load_texture(&assets.image_path("default:newgrounds_logo_animated"))?)
        } else {
            Newgrounds::Still(gfx.load_texture(&assets.image_path("default:newgrounds_logo"))?, 0.8)
        };

        // `getIntroTextShit`: a random line of introText.txt split on "--".
        let text = funkin_core::vfs::read_to_string(&assets.root.join("data").join("introText.txt")).unwrap_or_default();
        let options: Vec<Vec<String>> = text
            .lines()
            .map(|l| l.trim_end_matches('\r'))
            .filter(|l| !l.is_empty())
            .map(|l| l.split("--").map(str::to_owned).collect())
            .collect();
        let wacky = if options.is_empty() {
            vec![String::new(), String::new()]
        } else {
            options[((random() * options.len() as f32) as usize).min(options.len() - 1)].clone()
        };

        Ok(Self {
            logo,
            gf,
            press,
            ng,
            wacky,
            lines: Vec::new(),
            ng_visible: false,
            skipped: false,
            last_beat: 0,
            dance_left: false,
            time: 0.0,
            flash_at: None,
            confirmed_at: None,
            confirm: assets.sound("default:confirmMenu"),
            intro_delay: if INITIALIZED.swap(true, Ordering::Relaxed) { 0.0 } else { 1.0 },
            hue: 0.0,
            held: [false; 2],
            cheat_pos: 0,
            cheat_active: false,
            cheat_pending: false,
            beat_resync: false,
            attract_timer: None,
            attract_started: false,
            attract_fade: None,
            attract: None,
            keys_down: HashSet::new(),
        })
    }

    /// Still waiting to start the intro (no music yet).
    pub fn delayed(&self) -> bool {
        self.intro_delay > 0.0
    }

    /// A raw key going down or up (any key held skips the attract video).
    pub fn raw_key(&mut self, code: KeyCode, pressed: bool, is_volume: bool) {
        if pressed && !is_volume {
            self.keys_down.insert(code);
        } else {
            self.keys_down.remove(&code);
        }
    }

    /// A menu key let go.
    pub fn key_up(&mut self, code: KeyCode) {
        match code {
            KeyCode::ArrowLeft => self.held[0] = false,
            KeyCode::ArrowRight => self.held[1] = false,
            _ => {}
        }
    }

    fn code_press(&mut self, code: KeyCode) {
        if !matches!(code, KeyCode::ArrowUp | KeyCode::ArrowDown | KeyCode::ArrowLeft | KeyCode::ArrowRight) {
            return;
        }
        if code == CHEAT[self.cheat_pos] {
            self.cheat_pos += 1;
            if self.cheat_pos >= CHEAT.len() {
                // startCheat (the music swap happens in `update`).
                self.cheat_active = true;
                self.cheat_pending = true;
                self.flash_at = Some(self.time);
                self.attract_timer = None;
            }
        } else {
            self.cheat_pos = 0;
        }
    }

    /// Return to the title screen later (intro already seen): `skipIntro` at once.
    pub fn skip_intro_now(&mut self) {
        self.skip_intro();
    }

    fn skip_intro(&mut self) {
        if !self.skipped {
            self.ng_visible = false;
            self.lines.clear();
            self.flash_at = Some(self.time);
            self.skipped = true;
        }
    }

    pub fn start_music(&self, audio: &Audio, assets: &Assets, music: &mut Music) {
        // Fade from 0 to 1 over 4 seconds, unless the music is already going.
        if (!music.playing() || music.id != "freakyMenu") && music.play_menu(audio, assets, "freakyMenu", false) {
            music.fade(0.0, 1.0, 4.0);
        }
    }

    pub fn key(&mut self, code: KeyCode, audio: &Audio) -> MenuAction {
        if self.attract.is_some() || self.attract_fade.is_some() || self.delayed() {
            return MenuAction::None;
        }
        match code {
            KeyCode::ArrowLeft => self.held[0] = true,
            KeyCode::ArrowRight => self.held[1] = true,
            _ => {}
        }
        if !self.cheat_active && self.skipped {
            self.code_press(code);
            if self.cheat_pending {
                if let Some(s) = &self.confirm {
                    audio.play_sfx(s.clone(), 0.7);
                }
            }
        }
        match code {
            KeyCode::Enter | KeyCode::Space => {
                if !self.skipped {
                    self.skip_intro();
                } else if self.confirmed_at.is_some() {
                    return MenuAction::ToMainMenu;
                } else {
                    self.press.play("press", true, false);
                    self.flash_at = Some(self.time);
                    if let Some(s) = &self.confirm {
                        audio.play_sfx(s.clone(), 0.7);
                    }
                    self.confirmed_at = Some(self.time);
                    self.attract_timer = None;
                }
                MenuAction::None
            }
            KeyCode::Escape => MenuAction::Quit,
            _ => MenuAction::None,
        }
    }

    pub fn update(&mut self, dt: f32, music_ms: f64, gfx: &mut Gfx, audio: &Audio, assets: &Assets, music: &mut super::Music) -> MenuAction {
        self.time += dt;
        if self.intro_delay > 0.0 {
            self.intro_delay -= dt;
            if self.intro_delay > 0.0 {
                return MenuAction::None;
            }
            self.intro_delay = 0.0;
            self.start_music(audio, assets, music);
        }
        if !self.attract_started {
            // startIntro: the attract timer starts with the intro.
            self.attract_started = true;
            self.attract_timer = Some(self.time);
        }
        if let Some(a) = &mut self.attract {
            return Self::update_attract(a, dt, gfx, audio, &self.keys_down);
        }
        if let Some(at) = self.attract_fade {
            if self.time - at >= ATTRACT_FADE {
                self.attract_fade = None;
                music.stop(audio);
                let i = NEXT_VIDEO.fetch_add(1, Ordering::Relaxed) % ATTRACT_VIDEOS.len();
                let path = assets.root.join("videos").join("videos").join(format!("{}.mp4", ATTRACT_VIDEOS[i]));
                match funkin_core::vfs::read(&path).map_err(|e| e.to_string()).and_then(Video::open) {
                    Ok(video) => self.attract = Some(Attract { video, hold: 0.0, pie: None, pie_amount: -1.0 }),
                    Err(e) => {
                        log::warn!("attract video {}: {e}", path.display());
                        return MenuAction::ToTitleNow;
                    }
                }
            }
            return MenuAction::None;
        }
        if let Some(at) = self.attract_timer {
            if self.time - at >= ATTRACT_DELAY {
                // moveToAttract.
                self.attract_timer = None;
                self.attract_fade = Some(self.time);
                music.fade(music.volume(), 0.0, ATTRACT_FADE);
            }
        }
        if std::mem::take(&mut self.cheat_pending) && music.play_menu(audio, assets, "girlfriendsRingtone", true) {
            music.fade(0.0, 1.0, 4.0);
            self.beat_resync = true;
        }
        // swagShader.update(±elapsed * 0.1) while left / right are held.
        if self.held[0] {
            self.hue -= dt * 0.1;
        }
        if self.held[1] {
            self.hue += dt * 0.1;
        }
        self.logo.update(dt);
        self.gf.update(dt);
        self.press.update(dt);

        let bpm = if self.cheat_active { RINGTONE_BPM } else { MENU_BPM };
        let beat = (music_ms.max(0.0) / (60000.0 / bpm)).floor() as i64;
        if self.beat_resync {
            // Wait for the new track's clock (it starts near 0).
            if music_ms < 2000.0 {
                self.beat_resync = false;
            }
            self.last_beat = beat;
        }
        if beat > self.last_beat {
            if !self.skipped {
                // Performs every missed step, like the original.
                for i in self.last_beat..beat {
                    self.intro_beat(i + 1);
                }
            }
            if self.skipped {
                if self.cheat_active && beat % 2 == 0 {
                    self.hue += 0.125;
                }
                self.logo.play("bump", true, false);
                self.dance_left = !self.dance_left;
                self.gf.play(if self.dance_left { "danceRight" } else { "danceLeft" }, true, false);
            }
            self.last_beat = beat;
        }

        if let Some(at) = self.confirmed_at {
            if self.time - at >= 2.0 {
                return MenuAction::ToMainMenu;
            }
        }
        MenuAction::None
    }

    fn update_attract(a: &mut Attract, dt: f32, gfx: &mut Gfx, audio: &Audio, keys: &HashSet<KeyCode>) -> MenuAction {
        let ended = a.video.update(dt, gfx, audio);
        if !keys.is_empty() {
            a.hold += dt;
        } else {
            a.hold += (-0.1 - a.hold) * (dt * 3.0).clamp(0.0, 1.0);
        }
        a.hold = a.hold.clamp(0.0, HOLD_TIME);
        let amount = (a.hold / HOLD_TIME * 1.025).clamp(0.0, 1.0);
        if (amount - a.pie_amount).abs() > 1e-3 {
            a.pie_amount = amount;
            let rgba = pie_pixels(amount);
            match a.pie {
                Some(t) => gfx.replace_rgba(t, PIE_SIZE, PIE_SIZE, &rgba),
                None => a.pie = Some(gfx.upload_rgba(PIE_SIZE, PIE_SIZE, &rgba)),
            }
        }
        if ended || amount >= 1.0 {
            a.video.stop(audio);
            return MenuAction::ToTitleNow;
        }
        MenuAction::None
    }

    fn intro_beat(&mut self, beat: i64) {
        let w = |i: usize| self.wacky.get(i).cloned().unwrap_or_default();
        match beat {
            1 => self.lines = vec!["The".into(), "Funkin Crew Inc".into()],
            3 => self.lines.push("presents".into()),
            4 => self.lines.clear(),
            5 => self.lines = vec!["In association".into(), "with".into()],
            7 => {
                self.lines.push("newgrounds".into());
                self.ng_visible = true;
            }
            8 => {
                self.lines.clear();
                self.ng_visible = false;
            }
            9 => self.lines = vec![w(0)],
            11 => self.lines.push(w(1).trim().to_owned()),
            12 => self.lines.clear(),
            13 => self.lines.push("Friday".into()),
            14 => self.lines.push(if w(0) == "trending" { "Nigth".into() } else { "Night".into() }),
            15 => self.lines.push("Funkin".into()),
            16 => self.skip_intro(),
            _ => {}
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        if let Some(a) = &self.attract {
            a.video.draw(gfx);
            if let Some(t) = a.pie {
                // FlxRadialGauge at (width - 1.5w, height - 1.5h), scaled 1..1.3 about its centre.
                let s = 1.0 + 0.3 * a.pie_amount;
                let d = PIE_SIZE as f32;
                let (cx, cy) = (VIEW_W - d * 1.5 + d / 2.0, VIEW_H - d * 1.5 + d / 2.0);
                gfx.image(t, [0.0, 0.0, d, d], cx - d * s / 2.0, cy - d * s / 2.0, d * s, d * s, [1.0, 1.0, 1.0, a.pie_amount]);
            }
            return;
        }
        let cam = screen_camera(0.0, 0.0);
        if self.skipped {
            let shader = if self.hue != 0.0 { gfx.shader_program(crate::gfx::builtin_shaders::HSV_SHADER, false).ok() } else { None };
            let params = hue_params(self.hue);
            let with = |gfx: &mut Gfx, id: Option<ShaderId>, a: &Actor| {
                if let Some(id) = id {
                    gfx.set_shader(Some((id, &params, Default::default())));
                }
                a.draw(gfx, &cam);
                if id.is_some() {
                    gfx.set_shader(None);
                }
            };
            with(gfx, shader, &self.logo);
            with(gfx, shader, &self.gf);
            with(gfx, shader, &self.press);
        } else {
            for (i, line) in self.lines.iter().enumerate() {
                let w = fonts.bold.width(line, 1.0);
                fonts.bold.draw(gfx, line, (VIEW_W - w) / 2.0, i as f32 * 60.0 + 200.0, 1.0, WHITE);
            }
            if self.ng_visible {
                let y = VIEW_H * 0.52;
                match self.ng {
                    Newgrounds::Still(t, s) => {
                        let (w, h) = gfx.texture_size(t);
                        gfx.image(t, [0.0, 0.0, w, h], (VIEW_W - w * s) / 2.0, y, w * s, h * s, WHITE);
                    }
                    Newgrounds::Animated(t) => {
                        let (_, h) = gfx.texture_size(t);
                        let f = ((self.time * 4.0) as i64 % 2) as f32;
                        let s = 0.55;
                        gfx.image(t, [600.0 * f, 0.0, 600.0, h], (VIEW_W - 600.0 * s) / 2.0, y + 25.0, 600.0 * s, h * s, WHITE);
                    }
                }
            }
        }
        if let Some(at) = self.flash_at {
            draw_flash(gfx, self.time - at, 1.0);
        }
        if let Some(at) = self.attract_fade {
            gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, ((self.time - at) / ATTRACT_FADE).clamp(0.0, 1.0)]);
        }
    }
}

fn hue_params(hue: f32) -> ShaderParams {
    let mut p = ShaderParams::default();
    p.set("hue", vec![hue.rem_euclid(1.0) as f64]);
    p.set("sat", vec![1.0]);
    p.set("val", vec![1.0]);
    p
}

/// The skip dial's size: `makeShapeGraphic(CIRCLE, 40, 20)`, a 40 px ring around a 20 px hole.
const PIE_SIZE: u32 = 80;

/// The dial filled `amount` of the way round, clockwise from the top
/// (white over `0x8AC5C4C4`).
fn pie_pixels(amount: f32) -> Vec<u8> {
    let n = PIE_SIZE as usize;
    let mut out = vec![0u8; n * n * 4];
    let r = PIE_SIZE as f32 / 2.0;
    for y in 0..n {
        for x in 0..n {
            let (dx, dy) = (x as f32 + 0.5 - r, y as f32 + 0.5 - r);
            let d = (dx * dx + dy * dy).sqrt();
            let cover = (r - d).clamp(0.0, 1.0) * (d - 20.0).clamp(0.0, 1.0);
            if cover <= 0.0 {
                continue;
            }
            // Angle from the top, clockwise, in turns.
            let turn = (dx.atan2(-dy) / std::f32::consts::TAU).rem_euclid(1.0);
            let (c, a) = if turn < amount { ([255u8, 255, 255], 1.0) } else { ([0xC5, 0xC4, 0xC4], 0x8A as f32 / 255.0) };
            let i = (y * n + x) * 4;
            out[i..i + 3].copy_from_slice(&c);
            out[i + 3] = (a * cover * 255.0).round() as u8;
        }
    }
    out
}
