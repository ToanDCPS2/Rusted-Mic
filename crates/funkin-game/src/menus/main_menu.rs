//! Port of `MainMenuState`: the scrolling yellow background and the five
//! menu items, with the camera following the selection.

use std::sync::Arc;

use winit::keyboard::KeyCode;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Gfx, VIEW_H, VIEW_W};

use super::{anim, flicker_visible, follow_lerp, screen_camera, MenuAction, Tween};

const ITEMS: [&str; 5] = ["storymode", "freeplay", "merch", "options", "credits"];
const SPACING: f32 = 160.0;
/// `MenuTypedList`: the picked item flickers this long before its callback.
const FLICKER: f32 = 1.0;
/// `startExitState`: the other items fade out this long before the switch.
const EXIT_FADE: f32 = 0.4;
/// `Constants.URL_MERCH_FALLBACK` (the Newgrounds referral isn't available).
const URL_MERCH: &str = "https://needlejuicerecords.com/en-ca/pages/friday-night-funkin";

/// `WindowUtil.openURL`: open a link in the default browser.
pub fn open_url(url: &str) {
    #[cfg(windows)]
    let r = {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        std::process::Command::new("rundll32").args(["url.dll,FileProtocolHandler", url]).creation_flags(CREATE_NO_WINDOW).spawn()
    };
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        log::warn!("open {url}: {e}");
    }
}

pub struct MainMenu {
    bg: Actor,
    magenta: Actor,
    items: Vec<Actor>,
    pub selected: usize,
    scroll_y: f32,
    time: f32,
    /// When the current item was accepted.
    accepted_at: Option<f32>,
    /// Freeplay is open on top: the menu draws but takes no input.
    pub covered: bool,
    fades: Vec<Tween>,
    /// `Preferences.flashingLights`: the magenta background flickers only when on.
    pub flashing_lights: bool,
    scroll_sfx: Option<Arc<Pcm>>,
    confirm_sfx: Option<Arc<Pcm>>,
    cancel_sfx: Option<Arc<Pcm>>,
}

impl MainMenu {
    pub fn new(gfx: &mut Gfx, assets: &Assets, selected: usize) -> Result<Self, String> {
        // bg.setGraphicSize(FlxG.width * 1.2); updateHitbox(); screenCenter().
        let place_bg = |mut a: Actor| {
            let w = a.size()[0];
            let s = VIEW_W * 1.2 / w;
            a.scale = [s, s];
            let [w, h] = a.size();
            a.x = (VIEW_W - w) / 2.0;
            a.y = (VIEW_H - h) / 2.0;
            a.scroll = [0.0, 0.17];
            a
        };
        let bg = place_bg(Actor::image(gfx, assets, "menuBG", "default:menuBG", None, true)?);
        let mut magenta = place_bg(Actor::image(gfx, assets, "menuBGMagenta", "default:menuBGMagenta", None, true)?);
        magenta.visible = false;

        let top = (VIEW_H - SPACING * (ITEMS.len() as f32 - 1.0)) / 2.0;
        let mut items = Vec::new();
        for (i, name) in ITEMS.iter().enumerate() {
            let mut a = Actor::animated(
                gfx,
                assets,
                name,
                "sparrow",
                &format!("default:mainmenu/{name}"),
                None,
                &[anim("idle", &format!("{name} idle"), 24.0, true, &[]), anim("selected", &format!("{name} selected"), 24.0, true, &[])],
                true,
            )?;
            a.x = VIEW_W / 2.0;
            a.y = top + SPACING * i as f32;
            a.scroll = [0.0, 0.4];
            a.centered = true;
            a.play("idle", true, false);
            items.push(a);
        }
        let mut m = Self {
            bg,
            magenta,
            items,
            selected: selected.min(ITEMS.len() - 1),
            scroll_y: 0.0,
            time: 0.0,
            accepted_at: None,
            covered: false,
            fades: vec![Tween::hold(1.0); ITEMS.len()],
            flashing_lights: true,
            scroll_sfx: assets.sound("default:scrollMenu"),
            confirm_sfx: assets.sound("default:confirmMenu"),
            cancel_sfx: assets.sound("default:cancelMenu"),
        };
        m.select(m.selected);
        m.scroll_y = m.target_scroll();
        Ok(m)
    }

    fn select(&mut self, i: usize) {
        self.items[self.selected].play("idle", true, false);
        self.selected = i;
        self.items[i].play("selected", true, false);
    }

    /// Camera follows the selected item's midpoint.
    fn target_scroll(&self) -> f32 {
        self.items[self.selected].y - VIEW_H / 2.0
    }

    /// Waiting for input (nothing picked, not covered by another menu).
    pub fn idle(&self) -> bool {
        !self.covered && self.accepted_at.is_none()
    }

    /// Back from freeplay: interactive again, everything visible.
    pub fn uncover(&mut self) {
        self.covered = false;
        self.accepted_at = None;
        self.magenta.visible = false;
        self.fades = vec![Tween::hold(1.0); ITEMS.len()];
    }

    pub fn key(&mut self, code: KeyCode, audio: &Audio) -> MenuAction {
        if self.covered || self.accepted_at.is_some() {
            return MenuAction::None;
        }
        let n = ITEMS.len();
        let sfx = |s: &Option<Arc<Pcm>>, v: f32| {
            if let Some(s) = s {
                audio.play_sfx(s.clone(), v);
            }
        };
        match code {
            KeyCode::ArrowUp => {
                self.select((self.selected + n - 1) % n);
                sfx(&self.scroll_sfx, 0.4);
            }
            KeyCode::ArrowDown => {
                self.select((self.selected + 1) % n);
                sfx(&self.scroll_sfx, 0.4);
            }
            KeyCode::Enter | KeyCode::Space => {
                if matches!(ITEMS[self.selected], "storymode" | "freeplay" | "options" | "merch" | "credits") {
                    sfx(&self.confirm_sfx, 1.0);
                    self.accepted_at = Some(self.time);
                }
            }
            KeyCode::Escape | KeyCode::Backspace => {
                sfx(&self.cancel_sfx, 1.0);
                return MenuAction::ToTitle;
            }
            _ => {}
        }
        MenuAction::None
    }

    pub fn update(&mut self, dt: f32) -> MenuAction {
        self.time += dt;
        for a in &mut self.items {
            a.update(dt);
        }
        self.scroll_y = follow_lerp(self.scroll_y, self.target_scroll(), dt, 0.06);
        if let Some(at) = self.accepted_at {
            let e = self.time - at;
            // FlxFlicker.flicker(magenta, 1.1, 0.15, false, true)
            self.magenta.visible = self.flashing_lights && e < 1.1 && flicker_visible(e, 0.15);
            let exits = matches!(ITEMS[self.selected], "storymode" | "options" | "credits");
            if exits && e >= FLICKER && self.fades[self.selected].at(self.time) > 0.0 {
                // startExitState: the others fade out, the picked one hides,
                // and the state switches when the fade ends.
                let start = at + FLICKER;
                for (i, f) in self.fades.iter_mut().enumerate() {
                    *f = if i == self.selected { Tween::hold(0.0) } else { Tween::new(1.0, 0.0, start, EXIT_FADE, "quadOut") };
                }
            }
            let switch_at = if exits { FLICKER + EXIT_FADE } else { FLICKER };
            if e >= switch_at && !self.covered {
                if ITEMS[self.selected] == "merch" {
                    // selectMerch: open the store, then back to idle.
                    open_url(URL_MERCH);
                    self.accepted_at = None;
                    self.magenta.visible = false;
                    return MenuAction::None;
                }
                self.covered = true;
                self.magenta.visible = false;
                return match ITEMS[self.selected] {
                    "storymode" => MenuAction::OpenStory,
                    "options" => MenuAction::OpenOptions,
                    "credits" => MenuAction::OpenCredits,
                    _ => MenuAction::OpenFreeplay,
                };
            }
        }
        MenuAction::None
    }

    pub fn draw(&self, gfx: &mut Gfx, vcr: Option<&TtfFont>) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        let cam = screen_camera(0.0, self.scroll_y);
        self.bg.draw(gfx, &cam);
        self.magenta.draw(gfx, &cam);
        for (i, a) in self.items.iter().enumerate() {
            // Item flickers (1 s, 0.06 s) while being accepted.
            if let Some(at) = self.accepted_at {
                let e = self.time - at;
                if i == self.selected && e < FLICKER && !flicker_visible(e, 0.06) {
                    continue;
                }
            }
            a.draw_alpha(gfx, &cam, self.fades[i].at(self.time));
        }
        // leftWatermarkText: `Constants.VERSION` at (0, height - 18), VCR 16 outlined.
        if let Some(v) = vcr {
            crate::menus::options::draw_outlined_alpha(gfx, v, concat!("v", env!("CARGO_PKG_VERSION")), 0.0, VIEW_H - 18.0, 16.0, 1.0, 1.0);
        }
    }
}
