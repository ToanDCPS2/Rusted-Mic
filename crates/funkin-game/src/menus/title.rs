//! Port of `TitleState`: the intro text sequence over `freakyMenu`, then the
//! bumping logo, dancing Girlfriend and "Press Enter to Begin".

use winit::keyboard::KeyCode;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::Audio;
use crate::gfx::{Gfx, TexId, VIEW_H, VIEW_W, WHITE};

use super::{anim, draw_flash, random, screen_camera, Fonts, MenuAction, Music};

/// `freakyMenu` metadata BPM.
pub(crate) const MENU_BPM: f64 = 102.0;

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
        })
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
                }
                MenuAction::None
            }
            KeyCode::Escape => MenuAction::Quit,
            _ => MenuAction::None,
        }
    }

    pub fn update(&mut self, dt: f32, music_ms: f64) -> MenuAction {
        self.time += dt;
        self.logo.update(dt);
        self.gf.update(dt);
        self.press.update(dt);

        let beat = (music_ms.max(0.0) / (60000.0 / MENU_BPM)).floor() as i64;
        if beat > self.last_beat {
            if !self.skipped {
                // Performs every missed step, like the original.
                for i in self.last_beat..beat {
                    self.intro_beat(i + 1);
                }
            }
            if self.skipped {
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
        let cam = screen_camera(0.0, 0.0);
        if self.skipped {
            self.logo.draw(gfx, &cam);
            self.gf.draw(gfx, &cam);
            self.press.draw(gfx, &cam);
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
    }
}
