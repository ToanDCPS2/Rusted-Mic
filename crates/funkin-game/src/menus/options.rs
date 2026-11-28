//! Port of `OptionsState` and its pages: the options list, `PreferencesMenu`
//! (every desktop preference, plus Rusted Mic's Middlescroll, Hide
//! Opponent Notes and Ghost Tapping), `ControlsMenu` (key rebinding), `OffsetMenu` (lag
//! adjustment: global offset, calibration and a test strumline) and the
//! clear-save-data prompt.

use std::sync::Arc;

use winit::keyboard::KeyCode;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm, Track};
use crate::gfx::atlas::{Anim, Atlas};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Color, Gfx, TexId, VIEW_H, VIEW_W, WHITE};
use crate::play::NoteSkin;
use crate::settings::{Control, Controls, DebugDisplay, Settings, VSync};

use super::{flicker_visible, follow_lerp, Fonts, MenuAction, Music};

const YELLOW: Color = [0xfa as f32 / 255.0, 0xfd as f32 / 255.0, 0x6d as f32 / 255.0, 1.0];
const IDLE_ALPHA: f32 = 0.6;
/// `NumberPreferenceItem.HOLD_DELAY` / `CHANGE_RATE`.
const HOLD_DELAY: f32 = 0.3;
const CHANGE_RATE: f32 = 0.08;
/// `OffsetMenu.BPM`.
const OFFSET_BEAT_MS: f64 = 60000.0 / 100.0;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Main,
    Prefs,
    Controls,
    Offsets,
}

/// What runs once an accepted item has finished flickering.
#[derive(Clone, Copy)]
enum Pending {
    Main(usize),
    Pref(usize),
    Offset(usize),
    Prompt(bool),
}

const MAIN_ITEMS: [&str; 5] = ["PREFERENCES", "CONTROLS", "LAG ADJUSTMENT", "CLEAR SAVE DATA", "EXIT"];

enum PrefKind {
    Check(fn(&Settings) -> bool, fn(&mut Settings, bool)),
    Num { get: fn(&Settings) -> i32, set: fn(&mut Settings, i32), min: i32, max: i32, step: i32, pct: bool },
    Enum { names: &'static [&'static str], get: fn(&Settings) -> usize, set: fn(&mut Settings, usize) },
}

struct Pref {
    name: &'static str,
    desc: &'static str,
    kind: PrefKind,
}

fn prefs() -> Vec<Pref> {
    use PrefKind::*;
    vec![
        Pref { name: "Naughtyness", desc: "When enabled, raunchy content (such as swearing, etc.) is displayed.", kind: Check(|s| s.naughtyness, |s, v| s.naughtyness = v) },
        Pref { name: "Downscroll", desc: "When enabled, notes move downwards toward the strumline at the bottom of the screen.", kind: Check(|s| s.downscroll, |s, v| s.downscroll = v) },
        Pref { name: "Middlescroll", desc: "When enabled, your notes are centered and the opponent's notes move to the sides.", kind: Check(|s| s.middlescroll, |s, v| s.middlescroll = v) },
        Pref { name: "Hide Opponent Notes", desc: "When enabled, the opponent's strumline and notes are hidden.", kind: Check(|s| s.hide_opponent_notes, |s, v| s.hide_opponent_notes = v) },
        Pref { name: "Ghost Tapping", desc: "When enabled, pressing a key with no note to hit doesn't count as a miss.", kind: Check(|s| s.ghost_tapping, |s, v| s.ghost_tapping = v) },
        Pref { name: "Psych Engine Time Bar", desc: "When enabled, a bar at the top of the screen shows how much of the song is left, like in Psych Engine.", kind: Check(|s| s.psych_time_bar, |s, v| s.psych_time_bar = v) },
        Pref { name: "Psych Engine Health Bar", desc: "When enabled, the health bar uses the characters' colours, like in Psych Engine.", kind: Check(|s| s.psych_health_bar, |s, v| s.psych_health_bar = v) },
        Pref { name: "Note Splashes", desc: "When enabled, hitting a note perfectly makes it splash.", kind: Check(|s| s.note_splashes, |s, v| s.note_splashes = v) },
        Pref { name: "Antialiasing", desc: "When disabled, graphics are drawn without smoothing. Can look sharper and run faster on slow computers.", kind: Check(|s| s.antialiasing, |s, v| s.antialiasing = v) },
        Pref { name: "Strumline Background", desc: "Show a semi-transparent background behind the strumline.", kind: Num { get: |s| s.strumline_background, set: |s, v| s.strumline_background = v, min: 0, max: 100, step: 10, pct: true } },
        Pref { name: "Flashing Lights", desc: "When disabled, flashing effects are dampened. Useful for people with photosensitive epilepsy.", kind: Check(|s| s.flashing_lights, |s, v| s.flashing_lights = v) },
        Pref { name: "Camera Zooms", desc: "When enabled, the camera bounces during songs.", kind: Check(|s| s.zoom_camera, |s, v| s.zoom_camera = v) },
        Pref { name: "Subtitles", desc: "When enabled, subtitles appear during some songs and cutscenes.", kind: Check(|s| s.subtitles, |s, v| s.subtitles = v) },
        Pref {
            name: "Debug Display",
            desc: "When enabled, FPS and other debug stats are displayed.",
            kind: Enum {
                names: &["Off", "Simple", "Advanced"],
                get: |s| match s.debug_display {
                    DebugDisplay::Off => 0,
                    DebugDisplay::Simple => 1,
                    DebugDisplay::Advanced => 2,
                },
                set: |s, i| s.debug_display = [DebugDisplay::Off, DebugDisplay::Simple, DebugDisplay::Advanced][i],
            },
        },
        Pref { name: "Debug Display BG", desc: "Adjust the debug display's background opacity.", kind: Num { get: |s| s.debug_display_bg, set: |s, v| s.debug_display_bg = v, min: 0, max: 100, step: 10, pct: true } },
        Pref { name: "Pause on Unfocus", desc: "When enabled, the game automatically pauses when losing focus.", kind: Check(|s| s.auto_pause, |s, v| s.auto_pause = v) },
        Pref { name: "Launch in Fullscreen", desc: "When enabled, the game automatically starts up in fullscreen mode.", kind: Check(|s| s.auto_fullscreen, |s, v| s.auto_fullscreen = v) },
        Pref {
            name: "VSync",
            desc: "When enabled, the game attempts to match the framerate with your monitor's refresh rate.",
            kind: Enum {
                names: &["Off", "On", "Adaptive"],
                get: |s| match s.vsync {
                    VSync::Off => 0,
                    VSync::On => 1,
                    VSync::Adaptive => 2,
                },
                set: |s, i| s.vsync = [VSync::Off, VSync::On, VSync::Adaptive][i],
            },
        },
        Pref { name: "Unlocked Framerate", desc: "When enabled, the framerate is unlocked.\nThis setting is mutually exclusive with FPS.", kind: Check(|s| s.unlocked_framerate, |s, v| s.unlocked_framerate = v) },
        Pref { name: "FPS", desc: "The maximum framerate that the game targets.\nThis setting is mutually exclusive with Unlocked Framerate.", kind: Num { get: |s| s.framerate, set: |s, v| s.framerate = v, min: 30, max: 500, step: 5, pct: false } },
        Pref { name: "Hide Mouse", desc: "When enabled, the mouse is hidden while taking a screenshot.", kind: Check(|s| s.screenshot_hide_mouse, |s, v| s.screenshot_hide_mouse = v) },
        Pref { name: "Fancy Preview", desc: "When enabled, a preview is shown after taking a screenshot.", kind: Check(|s| s.screenshot_fancy_preview, |s, v| s.screenshot_fancy_preview = v) },
        Pref { name: "Preview on Save", desc: "When enabled, the preview is only shown after a screenshot is saved.", kind: Check(|s| s.screenshot_preview_on_save, |s, v| s.screenshot_preview_on_save = v) },
        Pref { name: "Discord RPC", desc: "Toggles Discord RPC.", kind: Check(|s| s.discord_rpc, |s, v| s.discord_rpc = v) },
    ]
}

/// The gamepad's Back (Select) button, which cancels a gamepad rebind.
const PAD_BACK: KeyCode = KeyCode::F27;

/// `ControlsMenu.controlGroups`: controls that can't share a key (a clash swaps).
const GROUPS: &[&[Control]] = &[
    &[Control::NoteUp, Control::NoteDown, Control::NoteLeft, Control::NoteRight],
    &[Control::UiUp, Control::UiDown, Control::UiLeft, Control::UiRight, Control::Accept, Control::Back],
    &[Control::CutsceneAdvance],
    &[Control::FreeplayFavorite, Control::FreeplayLeft, Control::FreeplayRight, Control::FreeplayCharSelect],
    &[Control::WindowFullscreen, Control::WindowScreenshot],
    &[Control::VolumeUp, Control::VolumeDown, Control::VolumeMute],
    &[Control::DebugMenu, Control::DebugChart, Control::DebugStage, Control::DebugDisplay],
];

/// The rows of the controls menu, in `Control.createAll()` order with headers.
/// (Character select is hidden, as in the original with one character unlocked.)
const CONTROL_ROWS: &[(Option<&str>, Control)] = &[
    (Some("NOTES"), Control::NoteLeft),
    (None, Control::NoteDown),
    (None, Control::NoteUp),
    (None, Control::NoteRight),
    (Some("UI"), Control::UiLeft),
    (None, Control::UiDown),
    (None, Control::UiUp),
    (None, Control::UiRight),
    (None, Control::Accept),
    (None, Control::Back),
    (None, Control::Pause),
    (None, Control::Reset),
    (Some("CUTSCENE"), Control::CutsceneAdvance),
    (Some("FREEPLAY"), Control::FreeplayFavorite),
    (None, Control::FreeplayLeft),
    (None, Control::FreeplayRight),
    (None, Control::FreeplayJumpToTop),
    (None, Control::FreeplayJumpToBottom),
    (Some("WINDOW"), Control::WindowScreenshot),
    (None, Control::WindowFullscreen),
    (Some("VOLUME"), Control::VolumeUp),
    (None, Control::VolumeDown),
    (None, Control::VolumeMute),
    (Some("DEBUG"), Control::DebugMenu),
    (None, Control::DebugChart),
    (None, Control::DebugStage),
    (None, Control::DebugDisplay),
];

/// `ControlsMenu.onInputSelect` on the keyboard: bind `key` (None = unbind) to
/// column `col` of `control`. Returns false if the change is refused (the last
/// key of a UI control can't be unbound).
#[cfg(test)]
pub fn rebind(c: &mut Controls, control: Control, col: usize, key: Option<KeyCode>) -> bool {
    rebind_on(c, false, control, col, key)
}

/// [`rebind`] on the keyboard (`pad` false) or gamepad bindings.
pub fn rebind_on(c: &mut Controls, pad: bool, control: Control, col: usize, key: Option<KeyCode>) -> bool {
    let slots = |c: &mut Controls, ctl: Control| -> [Option<KeyCode>; 2] {
        let k = c.device(pad).get(&ctl).map(|v| v.as_slice()).unwrap_or(&[]);
        [k.first().copied(), k.get(1).copied()]
    };
    let mine = slots(c, control);
    let in_ui = GROUPS[1].contains(&control);
    if key.is_none() && in_ui && mine[1 - col].is_none() {
        return false;
    }
    if let Some(k) = key {
        // Already bound to this control: nothing to do.
        if mine.contains(&Some(k)) {
            return true;
        }
        // A clash inside the group swaps the two keys.
        if let Some(group) = GROUPS.iter().find(|g| g.contains(&control)) {
            for &other in group.iter().filter(|&&o| o != control) {
                let keys = c.device(pad).entry(other).or_default();
                if let Some(j) = keys.iter().take(2).position(|&x| x == k) {
                    match mine[col] {
                        Some(old) => keys[j] = old,
                        None => {
                            keys.remove(j);
                        }
                    }
                }
            }
        }
    }
    // Unbound slots shift left: column 1 of an empty row becomes column 0.
    let keys = c.device(pad).entry(control).or_default();
    match key {
        Some(k) if col < keys.len() => keys[col] = k,
        Some(k) => keys.insert(keys.len().min(col), k),
        None if col < keys.len() => {
            keys.remove(col);
        }
        None => {}
    }
    true
}

/// `FlxCamera` follow with a deadzone: the scroll target that keeps an object
/// of height `h` at `y` inside `[top, top + height]` of the view.
fn deadzone_target(scroll: f32, y: f32, h: f32, top: f32, height: f32, min: f32) -> f32 {
    let mut t = scroll;
    if y < scroll + top {
        t = y - top;
    } else if y + h > scroll + top + height {
        t = y + h - top - height;
    }
    t.max(min)
}

fn ease_cube_in_out(t: f32) -> f32 {
    funkin_core::camera::ease("cubeInOut", t.clamp(0.0, 1.0) as f64) as f32
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Held left/right with `NumberPreferenceItem`'s repeat.
#[derive(Default)]
struct Hold {
    held: [bool; 2],
    delay: f32,
    rate: f32,
}

impl Hold {
    fn press(&mut self, right: bool) {
        self.held[right as usize] = true;
        self.delay = HOLD_DELAY;
        self.rate = 0.0;
    }
    fn release(&mut self, right: bool) {
        self.held[right as usize] = false;
    }
    /// Repeated steps this frame (-1, 0 or 1).
    fn tick(&mut self, dt: f32) -> i32 {
        if !(self.held[0] || self.held[1]) {
            return 0;
        }
        self.delay -= dt;
        if self.delay > 0.0 {
            return 0;
        }
        self.rate -= dt;
        if self.rate > 0.0 {
            return 0;
        }
        self.rate = CHANGE_RATE;
        if self.held[0] {
            -1
        } else {
            1
        }
    }
}

struct TestNote {
    time: f64,
    lane: usize,
    hit: bool,
}

/// `OffsetMenu` state.
struct Offsets {
    sel: usize,
    /// Intro slide (0..1 over 2 s), from the first visit.
    lerped: f32,
    /// 1 = calibrating/testing, -1 = leaving it, 0 = menu.
    should: i32,
    calibrating: bool,
    offset_lerp: f32,
    scale_mod: f32,
    arrows: Vec<(f64, f32)>,
    arrow_beat: f64,
    differences: Vec<f64>,
    temp_offset: i32,
    applied: f64,
    last_offset: f64,
    lerp_time: f32,
    saved_offset: i32,
    got_mad: bool,
    jump_text: String,
    count_text: String,
    notes: Vec<TestNote>,
    last_dir: usize,
    /// Receptor animation per lane: 0 static, 1 press, 2 confirm, and since when.
    strums: [(u8, f32); 4],
    last_local: f64,
    drums_fade: Option<(f32, f32, f32, f32)>,
}

impl Default for Offsets {
    fn default() -> Self {
        Self {
            sel: 0,
            lerped: 0.0,
            should: 0,
            calibrating: false,
            offset_lerp: 0.0,
            scale_mod: 1.0,
            arrows: Vec::new(),
            arrow_beat: 0.0,
            differences: Vec::new(),
            temp_offset: 0,
            applied: 0.0,
            last_offset: 0.0,
            lerp_time: 0.0,
            saved_offset: 0,
            got_mad: false,
            jump_text: String::new(),
            count_text: String::new(),
            notes: Vec::new(),
            last_dir: 0,
            strums: [(0, 0.0); 4],
            last_local: 0.0,
            drums_fade: None,
        }
    }
}

impl Offsets {
    fn average(&self) -> f64 {
        if self.differences.is_empty() {
            return 0.0;
        }
        self.differences.iter().sum::<f64>() / self.differences.len() as f64
    }
    fn consistency(&self) -> f64 {
        if self.differences.is_empty() {
            return 0.0;
        }
        let avg = self.average();
        (self.differences.iter().map(|d| (d - avg).powi(2)).sum::<f64>() / self.differences.len() as f64).sqrt()
    }
    fn add_difference(&mut self, ms: f64) {
        self.differences.push(ms);
        if self.differences.len().is_multiple_of(4) && self.calibrating {
            self.temp_offset = self.average() as i32;
            self.last_offset = self.applied;
            self.lerp_time = 0.0;
        }
    }
}

const OFFSET_ITEMS: [&str; 4] = ["Offset (Global)", "Reset Offset", "Offset Calibration", "Test"];

pub struct Options {
    bg: TexId,
    bg_size: (f32, f32),
    page: Page,
    time: f32,
    main_sel: usize,
    flicker: Option<(f32, usize, Pending)>,
    pref_list: Vec<Pref>,
    pref_sel: usize,
    pref_scroll: f32,
    /// When each checkbox last turned on (its "selecting" animation).
    checked_at: Vec<f32>,
    hold: Hold,
    ctl_sel: usize,
    ctl_scroll: f32,
    /// A gamepad is connected (set by the app each frame).
    pub gamepad_present: bool,
    /// The controls page has the Keyboard / Gamepad bar (a gamepad was
    /// connected when it opened).
    ctl_devices: bool,
    /// The bar has focus, and which device is highlighted (0 keys, 1 gamepad).
    ctl_on_devices: bool,
    ctl_device: usize,
    /// Editing the gamepad bindings.
    ctl_pad: bool,
    /// A device was picked: it flickers, then the grid takes over.
    ctl_device_pick: Option<f32>,
    /// Waiting for a key; holds the key that opened the prompt until it's released.
    rebinding: Option<Option<KeyCode>>,
    popup: bool,
    /// Clear-save-data prompt: selected button (0 = Delete, 1 = Cancel).
    prompt: Option<usize>,
    off: Offsets,
    /// Switch to the offsets page once the menu music has faded (scene time).
    to_offsets_at: Option<f32>,
    drums: Option<Arc<Track>>,
    checkbox: Option<(Atlas, Anim, Anim)>,
    latency_arrow: Option<TexId>,
    latency_receptor: Option<TexId>,
    scroll_sfx: Option<Arc<Pcm>>,
    confirm_sfx: Option<Arc<Pcm>>,
    cancel_sfx: Option<Arc<Pcm>>,
    /// Settings changed: the app saves and applies them.
    pub dirty: bool,
    exiting: bool,
}

/// `menuBG` through `HSVShader(-0.6, 0.9, 3.6)`, applied once on the CPU.
fn hsv_background(gfx: &mut Gfx, assets: &Assets) -> Result<(TexId, (f32, f32)), String> {
    let path = assets.image_path("default:menuBG");
    let bytes = funkin_core::vfs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut img = image::load_from_memory(&bytes).map_err(|e| e.to_string())?.to_rgba8();
    let (hue, sat, val) = (-0.6f32, 0.9f32, 3.6f32);
    for p in img.pixels_mut() {
        let [r, g, b, a] = p.0;
        let (r, g, b) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
        // rgb2hsv / hsv2rgb from hsv.frag.
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let d = max - min;
        let mut h = if d <= 1e-10 {
            0.0
        } else if max == r {
            ((g - b) / d).rem_euclid(6.0) / 6.0
        } else if max == g {
            ((b - r) / d + 2.0) / 6.0
        } else {
            ((r - g) / d + 4.0) / 6.0
        };
        let mut s = if max <= 1e-10 { 0.0 } else { d / max };
        let mut v = max;
        h *= hue;
        s *= sat;
        v *= val * (hue * 0.5 + 0.5);
        let ch = |k: f32| {
            let p = ((h + k).rem_euclid(1.0) * 6.0 - 3.0).abs();
            v * (1.0 + ((p - 1.0).clamp(0.0, 1.0) - 1.0) * s)
        };
        let to8 = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u8;
        p.0 = [to8(ch(1.0)), to8(ch(2.0 / 3.0)), to8(ch(1.0 / 3.0)), a];
    }
    let (w, h) = img.dimensions();
    Ok((gfx.upload_rgba(w, h, &img), (w as f32, h as f32)))
}

impl Options {
    pub fn new(gfx: &mut Gfx, assets: &Assets, settings: &Settings) -> Result<Self, String> {
        let (bg, bg_size) = hsv_background(gfx, assets)?;
        let checkbox = assets.atlas(gfx, "default:checkboxThingie").ok().map(|a| {
            let st = a.prefix("Check Box unselected");
            let ch = a.prefix("Check Box selecting animation");
            (a, st, ch)
        });
        let pref_list = prefs();
        let checked_at = vec![-100.0; pref_list.len()];
        let _ = settings;
        Ok(Self {
            bg,
            bg_size,
            page: Page::Main,
            time: 0.0,
            main_sel: 0,
            flicker: None,
            pref_list,
            pref_sel: 0,
            pref_scroll: 0.0,
            checked_at,
            hold: Hold::default(),
            ctl_sel: 0,
            ctl_scroll: 0.0,
            gamepad_present: false,
            ctl_devices: false,
            ctl_on_devices: false,
            ctl_device: 0,
            ctl_pad: false,
            ctl_device_pick: None,
            rebinding: None,
            popup: false,
            prompt: None,
            off: Offsets::default(),
            to_offsets_at: None,
            drums: None,
            checkbox,
            latency_arrow: assets.texture(gfx, "shared:latencyArrow").ok(),
            latency_receptor: assets.texture(gfx, "shared:latencyReceptor").ok(),
            scroll_sfx: assets.sound("default:scrollMenu"),
            confirm_sfx: assets.sound("default:confirmMenu"),
            cancel_sfx: assets.sound("default:cancelMenu"),
            dirty: false,
            exiting: false,
        })
    }

    fn sfx(&self, audio: &Audio, s: &Option<Arc<Pcm>>, v: f32) {
        if let Some(s) = s {
            audio.play_sfx(s.clone(), v);
        }
    }

    fn busy(&self) -> bool {
        self.flicker.is_some() || self.exiting || self.to_offsets_at.is_some()
    }

    /// `MenuTypedList.accept` for a non-instant item: confirm, flicker, then run.
    fn accept(&mut self, audio: &Audio, idx: usize, p: Pending) {
        self.sfx(audio, &self.confirm_sfx, 1.0);
        self.flicker = Some((self.time, idx, p));
    }

    fn back(&mut self, audio: &Audio) {
        self.sfx(audio, &self.cancel_sfx, 1.0);
    }

    /// The song position the offsets page runs on: the music clock within the
    /// loop, plus the global offset (as gameplay applies it) unless calibrating.
    fn offsets_pos(&self, settings: &Settings, music: &Music, audio: &Audio) -> f64 {
        let t = music.time_ms(audio);
        let len = music.loop_len_ms().max(1.0);
        let local = t.rem_euclid(len);
        local + if self.off.calibrating { 0.0 } else { settings.global_offset as f64 }
    }

    /// A key press. `menu` is the key translated through the UI bindings.
    #[allow(clippy::too_many_arguments)]
    pub fn key_down(
        &mut self,
        raw: KeyCode,
        menu: Option<KeyCode>,
        repeat: bool,
        settings: &mut Settings,
        audio: &Audio,
        music: &mut Music,
        assets: &Assets,
    ) -> MenuAction {
        // Rebinding waits for a key release; the popup closes on Escape release.
        if self.rebinding.is_some() || self.popup {
            return MenuAction::None;
        }
        if self.page == Page::Offsets && self.off.should == 1 {
            if repeat {
                return MenuAction::None;
            }
            if menu == Some(KeyCode::Escape) {
                self.exit_calibration(true, settings, audio);
                return MenuAction::None;
            }
            let pos = self.offsets_pos(settings, music, audio);
            if self.off.calibrating {
                self.calibration_press(pos, settings, audio);
            } else if let Some(lane) = settings.controls.note_lane(raw) {
                self.test_press(lane, pos);
            }
            return MenuAction::None;
        }
        let Some(code) = menu else { return MenuAction::None };
        if self.busy() {
            return MenuAction::None;
        }
        if let Some(sel) = self.prompt {
            match code {
                KeyCode::ArrowLeft | KeyCode::ArrowRight if !repeat => {
                    self.prompt = Some(1 - sel);
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                // No Escape: the page behind is disabled and `Prompt` has no back action.
                KeyCode::Enter if !repeat => self.accept(audio, sel, Pending::Prompt(sel == 0)),
                _ => {}
            }
            return MenuAction::None;
        }
        let nav = |sel: usize, n: usize, by: i32| (sel as i32 + by).rem_euclid(n as i32) as usize;
        match self.page {
            Page::Main => match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    self.main_sel = nav(self.main_sel, MAIN_ITEMS.len(), if code == KeyCode::ArrowUp { -1 } else { 1 });
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                KeyCode::Enter if !repeat => self.accept(audio, self.main_sel, Pending::Main(self.main_sel)),
                KeyCode::Escape if !repeat => {
                    self.back(audio);
                    self.exiting = true;
                    return MenuAction::CloseOptions;
                }
                _ => {}
            },
            Page::Prefs => match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    self.pref_sel = nav(self.pref_sel, self.pref_list.len(), if code == KeyCode::ArrowUp { -1 } else { 1 });
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                KeyCode::ArrowLeft | KeyCode::ArrowRight if !repeat => {
                    let right = code == KeyCode::ArrowRight;
                    self.hold.press(right);
                    self.step_pref(settings, if right { 1 } else { -1 }, true);
                }
                KeyCode::Enter if !repeat => {
                    if matches!(self.pref_list[self.pref_sel].kind, PrefKind::Check(..)) {
                        self.accept(audio, self.pref_sel, Pending::Pref(self.pref_sel));
                    }
                }
                KeyCode::Escape if !repeat => {
                    self.back(audio);
                    self.pref_scroll = 0.0;
                    self.page = Page::Main;
                }
                _ => {}
            },
            Page::Controls if self.ctl_device_pick.is_some() => {}
            Page::Controls if self.ctl_on_devices => match code {
                // `deviceList`: horizontal, no wrap.
                KeyCode::ArrowLeft if self.ctl_device == 1 => {
                    self.ctl_device = 0;
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                KeyCode::ArrowRight if self.ctl_device == 0 => {
                    self.ctl_device = 1;
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                KeyCode::Enter if !repeat => {
                    self.sfx(audio, &self.confirm_sfx, 1.0);
                    self.ctl_device_pick = Some(self.time);
                }
                KeyCode::Escape if !repeat => {
                    self.back(audio);
                    self.page = Page::Main;
                }
                _ => {}
            },
            Page::Controls => {
                let n = CONTROL_ROWS.len() * 2;
                match code {
                    KeyCode::ArrowUp => {
                        self.ctl_sel = nav(self.ctl_sel, n, -2);
                        self.sfx(audio, &self.scroll_sfx, 0.4);
                    }
                    KeyCode::ArrowDown => {
                        self.ctl_sel = nav(self.ctl_sel, n, 2);
                        self.sfx(audio, &self.scroll_sfx, 0.4);
                    }
                    KeyCode::ArrowLeft if self.ctl_sel % 2 == 1 => {
                        self.ctl_sel -= 1;
                        self.sfx(audio, &self.scroll_sfx, 0.4);
                    }
                    KeyCode::ArrowRight if self.ctl_sel.is_multiple_of(2) => {
                        self.ctl_sel += 1;
                        self.sfx(audio, &self.scroll_sfx, 0.4);
                    }
                    KeyCode::Enter if !repeat => self.rebinding = Some(Some(raw)),
                    // goToDeviceList
                    KeyCode::Escape if !repeat && self.ctl_devices => self.ctl_on_devices = true,
                    KeyCode::Escape if !repeat => {
                        self.back(audio);
                        self.page = Page::Main;
                    }
                    _ => {}
                }
            }
            Page::Offsets => match code {
                KeyCode::ArrowUp | KeyCode::ArrowDown => {
                    self.off.sel = nav(self.off.sel, OFFSET_ITEMS.len(), if code == KeyCode::ArrowUp { -1 } else { 1 });
                    self.sfx(audio, &self.scroll_sfx, 0.4);
                }
                KeyCode::ArrowLeft | KeyCode::ArrowRight if !repeat && self.off.sel == 0 => {
                    let right = code == KeyCode::ArrowRight;
                    self.hold.press(right);
                    self.step_offset(settings, if right { 1 } else { -1 });
                }
                KeyCode::Enter if !repeat && self.off.sel > 0 => self.accept(audio, self.off.sel, Pending::Offset(self.off.sel)),
                KeyCode::Escape if !repeat && self.off.should == 0 => {
                    self.back(audio);
                    self.leave_offsets(audio, music, assets);
                }
                _ => {}
            },
        }
        MenuAction::None
    }

    /// Listening for a key to bind (global hotkeys stay quiet).
    pub fn capturing(&self) -> bool {
        self.rebinding.is_some()
    }

    pub fn key_up(&mut self, raw: KeyCode, menu: Option<KeyCode>, settings: &mut Settings, audio: &Audio) {
        if matches!(menu, Some(KeyCode::ArrowLeft)) {
            self.hold.release(false);
        }
        if matches!(menu, Some(KeyCode::ArrowRight)) {
            self.hold.release(true);
        }
        if self.page == Page::Offsets && self.off.should == 1 && !self.off.calibrating {
            if let Some(lane) = settings.controls.note_lane(raw) {
                self.off.strums[lane] = (0, self.time);
            }
        }
        if self.popup {
            if raw == KeyCode::Escape || (self.ctl_pad && raw == PAD_BACK) {
                self.popup = false;
            }
            return;
        }
        let Some(opened_by) = self.rebinding else { return };
        // The key that opened the prompt doesn't count until it's released.
        if opened_by == Some(raw) {
            self.rebinding = Some(None);
            return;
        }
        let (row, col) = (self.ctl_sel / 2, self.ctl_sel % 2);
        let control = CONTROL_ROWS[row].1;
        let new = match raw {
            KeyCode::Escape => {
                self.rebinding = None;
                return;
            }
            KeyCode::Backspace => None,
            // On the gamepad, Back cancels and any other button binds.
            PAD_BACK if self.ctl_pad => {
                self.rebinding = None;
                return;
            }
            k if self.ctl_pad && crate::pad::is_pad(k) => Some(k),
            k if !self.ctl_pad && crate::settings::key_id(k).is_some() => Some(k),
            _ => return,
        };
        self.rebinding = None;
        if rebind_on(&mut settings.controls, self.ctl_pad, control, col, new) {
            self.dirty = true;
        } else {
            self.popup = true;
        }
        let _ = audio;
    }

    fn step_pref(&mut self, settings: &mut Settings, dir: i32, pressed: bool) {
        let p = &self.pref_list[self.pref_sel];
        match &p.kind {
            PrefKind::Num { get, set, min, max, step, .. } => {
                let v = (get(settings) + dir * step).clamp(*min, *max);
                if v != get(settings) {
                    set(settings, v);
                    self.dirty = true;
                }
            }
            PrefKind::Enum { names, get, set } if pressed => {
                let i = (get(settings) as i32 + dir).rem_euclid(names.len() as i32) as usize;
                set(settings, i);
                self.dirty = true;
            }
            _ => {}
        }
    }

    fn step_offset(&mut self, settings: &mut Settings, dir: i32) {
        let v = (settings.global_offset + dir).clamp(-1500, 1500);
        if v != settings.global_offset {
            settings.global_offset = v;
            self.dirty = true;
        }
    }

    fn run_pending(&mut self, p: Pending, settings: &mut Settings, audio: &Audio, music: &mut Music) -> MenuAction {
        match p {
            Pending::Main(i) => match MAIN_ITEMS[i] {
                "PREFERENCES" => self.page = Page::Prefs,
                "CONTROLS" => {
                    self.page = Page::Controls;
                    self.ctl_devices = self.gamepad_present;
                    self.ctl_on_devices = self.ctl_devices;
                    self.ctl_device = 0;
                    self.ctl_pad = false;
                    self.ctl_device_pick = None;
                    self.ctl_scroll = 0.0;
                }
                "LAG ADJUSTMENT" => {
                    let v = music.volume();
                    music.fade(v, 0.0, 0.5);
                    self.to_offsets_at = Some(self.time + 0.5);
                }
                "CLEAR SAVE DATA" => self.prompt = Some(0),
                _ => {
                    self.exiting = true;
                    return MenuAction::CloseOptions;
                }
            },
            Pending::Pref(i) => {
                if let PrefKind::Check(get, set) = self.pref_list[i].kind {
                    let v = !get(settings);
                    set(settings, v);
                    if v {
                        self.checked_at[i] = self.time;
                    }
                    self.dirty = true;
                }
            }
            Pending::Offset(1) => {
                settings.global_offset = 0;
                self.dirty = true;
            }
            Pending::Offset(2) => self.start_calibration(settings, audio, music),
            Pending::Offset(_) => self.start_test(settings, audio, music),
            Pending::Prompt(yes) => {
                self.prompt = None;
                if yes {
                    // Save.clearData(), then FlxG.switchState(InitState).
                    let _ = std::fs::remove_file(crate::settings::settings_path());
                    *settings = Settings::default();
                    self.dirty = true;
                    self.exiting = true;
                    return MenuAction::Restart;
                }
            }
        }
        MenuAction::None
    }

    fn fade_drums(&mut self, to: f32, dur: f32) {
        let from = self.drums_volume();
        self.off.drums_fade = Some((from, to, self.time, dur));
    }

    fn drums_volume(&self) -> f32 {
        match self.off.drums_fade {
            Some((a, b, at, dur)) => lerp(a, b, ((self.time - at) / dur.max(1e-3)).clamp(0.0, 1.0)),
            None => 0.0,
        }
    }

    fn start_calibration(&mut self, settings: &mut Settings, audio: &Audio, music: &Music) {
        let o = &mut self.off;
        o.notes.clear();
        o.jump_text = "Press any key to the beat!".into();
        o.count_text = "Current Offset: 0ms".into();
        o.calibrating = true;
        o.differences.clear();
        o.offset_lerp = 0.0;
        o.saved_offset = settings.global_offset;
        // Saved and zeroed so the player can recalibrate from scratch.
        settings.global_offset = 0;
        o.should = 1;
        o.temp_offset = 0;
        o.applied = 0.0;
        o.got_mad = false;
        let pos = self.offsets_pos(settings, music, audio);
        self.off.arrow_beat = (pos / OFFSET_BEAT_MS).floor() + 4.0;
        self.off.last_local = pos;
        self.fade_drums(1.0, 1.0);
    }

    fn start_test(&mut self, settings: &Settings, audio: &Audio, music: &Music) {
        let pos = self.offsets_pos(settings, music, audio);
        let o = &mut self.off;
        o.should = 1;
        o.notes.clear();
        let b = pos / OFFSET_BEAT_MS;
        let floored = b.floor();
        o.arrow_beat = floored - floored.rem_euclid(4.0) + 4.0;
        o.last_dir = 0;
        if (o.arrow_beat - b).floor() < 4.0 {
            o.arrow_beat += o.arrow_beat.rem_euclid(4.0) + 4.0;
        }
        o.jump_text = "Hit the notes as they come in!".into();
        o.differences.clear();
        o.last_local = pos;
        self.fade_drums(1.0, 1.0);
    }

    fn exit_calibration(&mut self, cancel: bool, settings: &mut Settings, audio: &Audio) {
        self.off.should = -1;
        self.off.temp_offset = 0;
        if cancel {
            if self.off.calibrating {
                settings.global_offset = self.off.saved_offset;
            }
            self.sfx(audio, &self.cancel_sfx, 1.0);
        } else {
            self.sfx(audio, &self.confirm_sfx, 1.0);
        }
        self.dirty = true;
        self.fade_drums(0.0, 1.0);
    }

    fn calibration_press(&mut self, pos: f64, settings: &mut Settings, audio: &Audio) {
        let b = pos / OFFSET_BEAT_MS;
        let ms = (b.round() - b) * OFFSET_BEAT_MS;
        let o = &mut self.off;
        if let Some(i) = (0..o.arrows.len()).min_by(|&x, &y| (o.arrows[x].0 - b).abs().total_cmp(&(o.arrows[y].0 - b).abs())) {
            let time_diff = (o.arrows[i].0 - b) * OFFSET_BEAT_MS;
            if (time_diff - o.temp_offset as f64).abs() <= 80.0 {
                o.arrows.remove(i);
            }
        }
        if o.consistency() > 40.0 && o.differences.len() > 8 {
            o.jump_text = "Try to be a little more consistent with your timing!".into();
            o.differences.clear();
            o.temp_offset = 0;
            o.applied = 0.0;
            o.got_mad = true;
            return;
        }
        o.add_difference(ms);
        let actual = ms - o.temp_offset as f64;
        let n = o.differences.len();
        if n >= 30 {
            o.jump_text = "Calibration complete!".into();
            settings.global_offset = o.temp_offset;
            self.exit_calibration(false, settings, audio);
            return;
        }
        if !o.got_mad {
            o.jump_text = if actual.abs() < 45.0 { "Great job" } else { "Nice job" }.into();
            o.jump_text += if n < 8 { ", keep going!" } else { "!" };
        }
        o.jump_text += &format!("\n{n}/30");
        o.got_mad = false;
        o.scale_mod = 0.75;
    }

    fn test_press(&mut self, lane: usize, pos: f64) {
        let o = &mut self.off;
        let hit = o
            .notes
            .iter()
            .position(|n| !n.hit && n.lane == lane && (n.time - pos).abs() <= funkin_core::constants::HIT_WINDOW_MS);
        match hit {
            None => o.strums[lane] = (1, self.time),
            Some(i) => {
                let diff = (o.notes[i].time - pos) as i32;
                o.notes[i].hit = true;
                o.add_difference(diff as f64);
                o.jump_text = if diff == 0 {
                    "Perfect!\n".into()
                } else if diff > 0 {
                    format!("Early!\n{diff}ms")
                } else {
                    format!("Late!\n{diff}ms")
                };
                o.jump_text += &format!("\nAvg: {}ms", o.average() as i32);
                o.strums[lane] = (2, self.time);
            }
        }
    }

    fn leave_offsets(&mut self, audio: &Audio, music: &mut Music, assets: &Assets) {
        // exitOffsets: drums and music fade out, then freakyMenu restarts.
        self.fade_drums(0.0, 0.5);
        self.page = Page::Main;
        if music.play_menu(audio, assets, "freakyMenu", true) {
            music.fade(0.0, 1.0, 0.5);
        }
        self.drums = None;
    }

    fn enter_offsets(&mut self, audio: &Audio, music: &mut Music, assets: &Assets) {
        let main = Pcm::load_ogg(&assets.music_path("offsetsLoop")).map(Track::new);
        let drums = Pcm::load_ogg(&assets.music_path("offsetsLoop").with_file_name("drumsLoop.ogg")).map(Track::new);
        match (main, drums) {
            (Ok(m), Ok(d)) => {
                d.set_volume(0.0);
                music.start_layered(audio, "offsetsLoop", m, vec![d.clone()]);
                music.fade(0.0, 1.0, 1.0);
                self.drums = Some(d);
            }
            (Ok(m), Err(e)) | (Err(e), Ok(m)) => {
                log::warn!("offsets music: {e}");
                music.start_layered(audio, "offsetsLoop", m, vec![]);
                music.fade(0.0, 1.0, 1.0);
            }
            (Err(e), Err(_)) => log::warn!("offsets music: {e}"),
        }
        self.off.drums_fade = None;
        self.page = Page::Offsets;
    }

    pub fn update(&mut self, dt: f32, settings: &mut Settings, audio: &Audio, music: &mut Music, assets: &Assets) -> MenuAction {
        self.time += dt;
        if let Some(t) = self.to_offsets_at {
            if self.time >= t {
                self.to_offsets_at = None;
                self.enter_offsets(audio, music, assets);
            }
        }
        let mut action = MenuAction::None;
        if let Some((start, _, p)) = self.flicker {
            if self.time - start >= 1.0 {
                self.flicker = None;
                action = self.run_pending(p, settings, audio, music);
            }
        }
        // Held left/right repeats on number items.
        let steps = self.hold.tick(dt);
        if steps != 0 && !self.busy() && self.prompt.is_none() {
            match self.page {
                Page::Prefs => self.step_pref(settings, steps, false),
                Page::Offsets if self.off.sel == 0 && self.off.should == 0 => self.step_offset(settings, steps),
                _ => {}
            }
        }
        if let Some(d) = &self.drums {
            d.set_volume(self.drums_volume());
        }
        match self.page {
            Page::Prefs => {
                let y = 120.0 * self.pref_sel as f32 + 30.0;
                let target = deadzone_target(self.pref_scroll, y, 70.0, 160.0, VIEW_H - 320.0, 0.0);
                self.pref_scroll = follow_lerp(self.pref_scroll, target, dt, 0.085);
            }
            Page::Controls => {
                if let Some(at) = self.ctl_device_pick {
                    // selectDevice, after the 1 s flicker.
                    if self.time - at >= 1.0 {
                        self.ctl_device_pick = None;
                        self.ctl_pad = self.ctl_device == 1;
                        self.ctl_on_devices = false;
                    }
                }
                // camFollow: the device bar, or the selected row.
                let y = if self.ctl_on_devices { 0.0 } else { self.control_y(self.ctl_sel / 2) };
                let target = deadzone_target(self.ctl_scroll, y, 70.0, 100.0, VIEW_H - 200.0, 0.0);
                self.ctl_scroll = follow_lerp(self.ctl_scroll, target, dt, 0.06);
            }
            Page::Offsets => {
                let pos = self.offsets_pos(settings, music, audio);
                self.update_offsets(dt, pos, settings, audio);
            }
            Page::Main => {}
        }
        action
    }

    fn update_offsets(&mut self, dt: f32, pos: f64, settings: &mut Settings, audio: &Audio) {
        let o = &mut self.off;
        let b = pos / OFFSET_BEAT_MS;
        // The loop wrapped: carry arrows over and restart the test notes.
        if pos < o.last_local - 1000.0 {
            let last_b = o.last_local / OFFSET_BEAT_MS;
            for a in &mut o.arrows {
                a.0 = b + (a.0 - last_b);
            }
            o.arrow_beat = if o.calibrating { o.arrows.last().map(|a| a.0).unwrap_or(b) } else { 4.0 };
            o.notes.clear();
        }
        o.last_local = pos;
        if o.should == 1 && o.calibrating {
            o.lerp_time = (o.lerp_time + dt * 2.0).min(1.0);
            o.applied = o.last_offset + (o.temp_offset as f64 - o.last_offset) * o.lerp_time as f64;
            o.count_text = format!("Current Offset: {}ms", o.applied as i32);
            let applied = o.applied;
            let enough = o.differences.len() >= 8;
            for a in &mut o.arrows {
                if a.0 * OFFSET_BEAT_MS - applied - pos < -180.0 {
                    a.1 -= dt * 5.0;
                }
            }
            o.arrows.retain(|a| a.1 > 0.0 && enough);
            while b >= o.arrow_beat - 1.0 {
                o.arrow_beat = (o.arrow_beat - o.arrow_beat.rem_euclid(2.0)) + 2.0;
                if enough {
                    o.arrows.push((o.arrow_beat, 1.0));
                }
            }
        } else if o.should == 1 {
            while b >= o.arrow_beat - 2.0 && b < 124.0 {
                o.arrow_beat += 1.0;
                o.notes.push(TestNote { time: o.arrow_beat * OFFSET_BEAT_MS, lane: o.last_dir, hit: false });
                if (o.arrow_beat % 8.0).floor() == 0.0 {
                    o.notes.push(TestNote { time: o.arrow_beat * OFFSET_BEAT_MS, lane: 2, hit: false });
                }
                o.last_dir = (o.last_dir + 1) % 4;
            }
            if b >= 124.0 && o.last_dir != 0 {
                o.last_dir = 0;
            }
            o.notes.retain(|n| pos - n.time < 2000.0);
        } else {
            for a in &mut o.arrows {
                a.1 -= dt * 5.0;
            }
            o.arrows.retain(|a| a.1 > 0.0);
        }
        o.lerped = (o.lerped + dt / 2.0).min(1.0);
        if o.should == 1 {
            o.offset_lerp = (o.offset_lerp + dt / 2.0).min(1.0);
        } else if o.should == -1 {
            o.offset_lerp -= dt / 3.0;
            if o.offset_lerp <= 0.0 {
                o.calibrating = false;
                o.offset_lerp = 0.0;
                o.should = 0;
            }
        }
        if o.scale_mod < 1.0 {
            o.scale_mod = (o.scale_mod + dt / 2.0).min(1.0);
        }
        let _ = (settings, audio);
    }

    fn control_y(&self, row: usize) -> f32 {
        let mut y = if self.ctl_devices { 120.0 } else { 30.0 };
        for (i, (header, _)) in CONTROL_ROWS.iter().enumerate() {
            if header.is_some() {
                y += 70.0;
            }
            if i == row {
                return y;
            }
            y += 70.0;
        }
        y
    }

    /// Is item `idx` hidden by the accept flicker right now?
    fn flickered(&self, idx: usize, page: Page) -> bool {
        match self.flicker {
            Some((start, i, p)) if i == idx => {
                let same = matches!(
                    (p, page),
                    (Pending::Main(_), Page::Main) | (Pending::Pref(_), Page::Prefs) | (Pending::Offset(_), Page::Offsets)
                );
                same && self.prompt.is_none() && !flicker_visible(self.time - start, 0.06)
            }
            _ => false,
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts, skin: &NoteSkin, settings: &Settings) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        // menuBG: 1.1x the screen width, centred, no scroll.
        let s = VIEW_W * 1.1 / self.bg_size.0;
        let (w, h) = (self.bg_size.0 * s, self.bg_size.1 * s);
        gfx.sprite(self.bg, (VIEW_W - w) / 2.0, (VIEW_H - h) / 2.0, s, WHITE);
        let bold = &fonts.bold;
        match self.page {
            Page::Main => {
                for (i, name) in MAIN_ITEMS.iter().enumerate() {
                    if self.flickered(i, Page::Main) {
                        continue;
                    }
                    let a = if i == self.main_sel { 1.0 } else { IDLE_ALPHA };
                    bold.draw_centered(gfx, name, VIEW_W / 2.0, 100.0 + i as f32 * 100.0, 1.0, [1.0, 1.0, 1.0, a]);
                }
            }
            Page::Prefs => self.draw_prefs(gfx, fonts, settings),
            Page::Controls => self.draw_controls(gfx, fonts, settings),
            Page::Offsets => self.draw_offsets(gfx, fonts, skin, settings),
        }
        if let Some(sel) = self.prompt {
            // Prompt: a dimmed screen, the yellow box, the text and two buttons.
            gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, 128.0 / 255.0]);
            gfx.rect(100.0, 100.0, VIEW_W - 200.0, VIEW_H - 200.0, YELLOW);
            draw_block(gfx, bold, "This will delete\n\nALL your save data.\n\nAre you sure?", 100.0);
            let yes_w = bold.width("Delete", 1.0);
            let by = VIEW_H - bold.max_height - 100.0;
            for (i, (label, x)) in [("Delete", VIEW_W - yes_w - 100.0), ("Cancel", 100.0)].iter().enumerate() {
                if let Some((start, j, Pending::Prompt(_))) = self.flicker {
                    if j == i && !flicker_visible(self.time - start, 0.06) {
                        continue;
                    }
                }
                let a = if i == sel { 1.0 } else { IDLE_ALPHA };
                bold.draw(gfx, label, *x, by, 1.0, [1.0, 1.0, 1.0, a]);
            }
        }
    }

    fn draw_prefs(&self, gfx: &mut Gfx, fonts: &Fonts, settings: &Settings) {
        let scroll = self.pref_scroll;
        for (i, p) in self.pref_list.iter().enumerate() {
            let y = 120.0 * i as f32 + 30.0 - scroll;
            if !(-150.0..VIEW_H + 50.0).contains(&y) {
                continue;
            }
            let selected = i == self.pref_sel;
            let mut x = if selected { 150.0 } else { 120.0 };
            match &p.kind {
                PrefKind::Check(get, _) => {
                    if let Some((atlas, st, ch)) = &self.checkbox {
                        let on = get(settings);
                        let (anim, t, off) = if on {
                            (ch, self.time - self.checked_at[i], [17.0, 70.0])
                        } else {
                            (st, 0.0, [0.0, 0.0])
                        };
                        if let Some(f) = anim.frame_at(t, 24.0, false) {
                            // setGraphicSize(width * 0.7): scaled about the frame's centre.
                            let f0 = &atlas.frames[st.frames.first().copied().unwrap_or(0)];
                            let (ox, oy) = (f0.frame_w * 0.15, f0.frame_h * 0.15);
                            atlas.draw(gfx, f, ox - off[0], 120.0 * i as f32 - scroll + oy - off[1], 0.7, WHITE);
                        }
                    }
                }
                PrefKind::Num { get, pct, .. } => {
                    let v = get(settings);
                    let text = if *pct { format!("{v}%") } else { v.to_string() };
                    let tw = fonts.default.draw(gfx, &text, 15.0, y, 1.0, WHITE);
                    x += tw - 75.0;
                }
                PrefKind::Enum { names, get, .. } => {
                    let tw = fonts.default.draw(gfx, names[get(settings)], 15.0, y, 1.0, WHITE);
                    x += tw - 75.0;
                }
            }
            if self.flickered(i, Page::Prefs) {
                continue;
            }
            let a = if selected { 1.0 } else { IDLE_ALPHA };
            fonts.bold.draw(gfx, p.name, x, y, 1.0, [1.0, 1.0, 1.0, a]);
        }
        // Description: VCR 32 in a 1180-wide centred FlxText. `createPrefDescription`
        // centres it (and sizes the box) for the first description only; later
        // descriptions keep that top edge and grow downwards past the box.
        if let Some(vcr) = &fonts.vcr {
            let wrap = |gfx: &mut Gfx, desc: &str| -> Vec<String> {
                let mut lines: Vec<String> = Vec::new();
                for para in desc.split('\n') {
                    let mut cur = String::new();
                    for w in para.split(' ') {
                        let cand = if cur.is_empty() { w.to_owned() } else { format!("{cur} {w}") };
                        if !cur.is_empty() && vcr.width(gfx, &cand, 32.0) > 1180.0 {
                            lines.push(std::mem::replace(&mut cur, w.to_owned()));
                        } else {
                            cur = cand;
                        }
                    }
                    lines.push(cur);
                }
                lines
            };
            let line_h = vcr.line_height(32.0);
            let first = wrap(gfx, self.pref_list[0].desc);
            let first_h = vcr.text_height(32.0, first.len());
            let ty = ((VIEW_H - first_h) / 2.0).floor() + 270.0;
            gfx.rect(40.0, ty - 10.0, 1200.0, first_h + 25.0, [0.0, 0.0, 0.0, 0.6]);
            for (i, l) in wrap(gfx, self.pref_list[self.pref_sel].desc).iter().enumerate() {
                let lw = vcr.width(gfx, l, 32.0);
                draw_outlined(gfx, vcr, l, 50.0 + (1176.0 - lw) / 2.0, ty + i as f32 * line_h, 32.0, 3.0);
            }
        }
    }

    fn draw_controls(&self, gfx: &mut Gfx, fonts: &Fonts, settings: &Settings) {
        let scroll = self.ctl_scroll;
        if self.ctl_devices {
            // devicesBg: FlxG.width × 100, 0xFFFAFD6D, with the two device items.
            gfx.rect(0.0, -scroll, VIEW_W, 100.0, [0xFA as f32 / 255.0, 0xFD as f32 / 255.0, 0x6D as f32 / 255.0, 1.0]);
            let h = fonts.bold.max_height;
            for (i, name) in ["Keyboard", "Gamepad"].iter().enumerate() {
                let w = fonts.bold.width(name, 1.0);
                let x = if i == 0 { VIEW_W / 2.0 - w - 30.0 } else { VIEW_W / 2.0 + 30.0 };
                let picked = self.ctl_device_pick.is_some_and(|at| i == self.ctl_device && !super::flicker_visible(self.time - at, 0.06));
                if picked {
                    continue;
                }
                let a = if i == self.ctl_device { 1.0 } else { IDLE_ALPHA };
                fonts.bold.draw(gfx, name, x, (100.0 - h) / 2.0 - scroll, 1.0, [1.0, 1.0, 1.0, a]);
            }
        }
        let mut y = if self.ctl_devices { 120.0 } else { 30.0 } - scroll;
        for (row, (header, control)) in CONTROL_ROWS.iter().enumerate() {
            if let Some(h) = header {
                fonts.bold.draw_centered(gfx, h, VIEW_W / 2.0, y, 1.0, WHITE);
                y += 70.0;
            }
            if (-100.0..VIEW_H + 50.0).contains(&y) {
                let grid = !self.ctl_on_devices && self.ctl_device_pick.is_none();
                let row_sel = grid && self.ctl_sel / 2 == row;
                let a = if row_sel { 1.0 } else { IDLE_ALPHA };
                fonts.bold.draw(gfx, control.label(), 50.0, y, 1.0, [1.0, 1.0, 1.0, a]);
                let keys = if self.ctl_pad { settings.controls.pad_keys(*control) } else { settings.controls.keys(*control) };
                for col in 0..2 {
                    let label = keys.get(col).map(|&k| crate::settings::key_label(k)).unwrap_or("---");
                    let a = if grid && self.ctl_sel == row * 2 + col { 1.0 } else { IDLE_ALPHA };
                    fonts.default.draw(gfx, label, 750.0 + col as f32 * 300.0, y, 1.0, [1.0, 1.0, 1.0, a]);
                }
            }
            y += 70.0;
        }
        // `Prompt` is a substate: its 0x80000000 background dims the page.
        if self.rebinding.is_some() || self.popup {
            gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, 128.0 / 255.0]);
        }
        // selectDevice rewrites the prompts once a device is picked.
        let (prompt, popup) = match (self.ctl_devices, self.ctl_pad) {
            (false, _) => ("\nPress any key to rebind\n\n\nBackspace to unbind\n    Escape to cancel", "\nYou cannot unbind\nthat key!\n\n\nEscape to exit"),
            (true, false) => ("\nPress any key to rebind\n\n\n\n    Escape to cancel", "\nYou cannot unbind\nthat key!\n\n\nEscape to exit"),
            (true, true) => ("\nPress any button\n   to rebind\n\n\n Back to cancel", "\nYou cannot unbind\nthat button!\n\n\nBack to exit"),
        };
        if self.rebinding.is_some() {
            gfx.rect(100.0, 100.0, VIEW_W - 200.0, VIEW_H - 200.0, YELLOW);
            draw_block(gfx, &fonts.bold, prompt, 100.0);
        } else if self.popup {
            gfx.rect(100.0, 100.0, VIEW_W - 200.0, VIEW_H - 200.0, YELLOW);
            draw_block(gfx, &fonts.bold, popup, 100.0);
        }
    }

    fn draw_offsets(&self, gfx: &mut Gfx, fonts: &Fonts, skin: &NoteSkin, settings: &Settings) {
        let o = &self.off;
        let lerped = ease_cube_in_out(o.lerped);
        let ol = ease_cube_in_out(o.offset_lerp);
        gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.0, 0.0, 0.0, lerp(0.0, 0.5, lerped)]);
        let y_lerp = lerp(-480.0, 100.0, lerped);
        let x_lerp = lerp(0.0, VIEW_W, ol);
        for (i, name) in OFFSET_ITEMS.iter().enumerate() {
            let y = y_lerp + 120.0 * i as f32 + 30.0;
            let a = if i == o.sel { 1.0 } else { IDLE_ALPHA };
            let iw = fonts.bold.width(name, 1.0);
            let x = if i == 0 {
                let text = settings.global_offset.to_string();
                let tw = fonts.default.width(&text, 1.0);
                let lx = x_lerp + VIEW_W / 2.0 - (tw + iw + 20.0) / 2.0;
                fonts.default.draw(gfx, &text, lx, y, 1.0, WHITE);
                lx + tw + 20.0
            } else {
                x_lerp + VIEW_W / 2.0 - iw / 2.0
            };
            if !self.flickered(i, Page::Offsets) {
                fonts.bold.draw(gfx, name, x, y, 1.0, [1.0, 1.0, 1.0, a]);
            }
        }
        if o.offset_lerp <= 0.0 {
            return;
        }
        let Some(vcr) = &fonts.vcr else { return };
        let (cx, cy) = (VIEW_W / 2.0, VIEW_H / 2.0);
        if o.calibrating {
            if let Some(r) = self.latency_receptor {
                let (w, h) = gfx.texture_size(r);
                let s = ol * o.scale_mod;
                gfx.sprite(r, cx - w * s / 2.0, cy - h * s / 2.0, s, [1.0, 1.0, 1.0, ol]);
            }
            if let Some(a) = self.latency_arrow {
                let (w, h) = gfx.texture_size(a);
                let pos = o.last_local;
                for (beat, alpha) in &o.arrows {
                    let ms = beat * OFFSET_BEAT_MS - o.applied;
                    let y = cy + ((ms - pos) * funkin_core::constants::PIXELS_PER_MS * 2.0) as f32;
                    gfx.sprite(a, cx - w / 2.0, y - h / 2.0, 1.0, [1.0, 1.0, 1.0, *alpha]);
                }
            }
            let cw = vcr.width(gfx, &o.count_text, 32.0);
            draw_outlined_alpha(gfx, vcr, &o.count_text, cx - cw / 2.0, 600.0, 32.0, 4.0, ol);
        } else {
            // The test strumline, centred, with the notes scrolling to it.
            let down = settings.downscroll;
            let strum_cy = if down { VIEW_H - 76.0 } else { 76.0 };
            let x0 = cx - 2.0 * 112.0;
            let lane_cx = |lane: usize| x0 + lane as f32 * 112.0 + 52.0;
            for lane in 0..4 {
                let (state, since) = o.strums[lane];
                skin.draw_receptor(gfx, lane, state, self.time - since, lane_cx(lane), strum_cy, ol);
            }
            let pos = o.last_local;
            let px = funkin_core::constants::PIXELS_PER_MS as f32;
            for n in o.notes.iter().filter(|n| !n.hit) {
                let dy = (n.time - pos) as f32 * px;
                let y = if down { strum_cy - dy } else { strum_cy + dy };
                let missed = pos - n.time > funkin_core::constants::HIT_WINDOW_MS;
                let a = if missed { 0.3 } else { 1.0 };
                if (-200.0..VIEW_H + 200.0).contains(&y) {
                    skin.draw_note(gfx, n.lane, lane_cx(n.lane), y, a * ol);
                }
            }
        }
        let jy = if o.calibrating { 100.0 } else if settings.downscroll { VIEW_H - 425.0 } else { 350.0 };
        for (i, line) in o.jump_text.split('\n').enumerate() {
            let lw = vcr.width(gfx, line, 32.0);
            draw_outlined_alpha(gfx, vcr, line, cx - lw / 2.0, jy + i as f32 * vcr.line_height(32.0), 32.0, 4.0, ol);
        }
    }
}

/// An `AtlasText` block: left-aligned lines, the block centred on screen.
fn draw_block(gfx: &mut Gfx, font: &crate::gfx::text::Font, text: &str, y: f32) {
    let w = font.width(text, 1.0);
    let x = (VIEW_W - w) / 2.0;
    for (i, line) in text.split('\n').enumerate() {
        font.draw(gfx, line, x, y + i as f32 * font.max_height, 1.0, WHITE);
    }
}

fn draw_outlined(gfx: &mut Gfx, font: &TtfFont, text: &str, x: f32, y: f32, size: f32, border: f32) {
    draw_outlined_alpha(gfx, font, text, x, y, size, border, 1.0);
}

/// `FlxTextBorderStyle.OUTLINE` in black.
#[allow(clippy::too_many_arguments)]
pub fn draw_outlined_alpha(gfx: &mut Gfx, font: &TtfFont, text: &str, x: f32, y: f32, size: f32, border: f32, alpha: f32) {
    if alpha <= 0.0 {
        return;
    }
    // FlxText OUTLINE: `borderSize` rings (1 px apart) of 8 offset copies.
    let rings = if border > 0.0 { (border as i32).max(1) } else { 0 };
    for r in 1..=rings {
        let d = border / rings as f32 * r as f32;
        for (dx, dy) in [(-1.0, -1.0), (0.0, -1.0), (1.0, -1.0), (-1.0, 0.0), (1.0, 0.0), (-1.0, 1.0), (0.0, 1.0), (1.0, 1.0)] {
            font.draw(gfx, text, x + dx * d, y + dy * d, size, [0.0, 0.0, 0.0, alpha]);
        }
    }
    font.draw(gfx, text, x, y, size, [1.0, 1.0, 1.0, alpha]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rebind_rules() {
        let mut c = Controls::default();
        // A clash in the note group swaps keys.
        assert!(rebind(&mut c, Control::NoteLeft, 0, Some(KeyCode::KeyD)));
        assert_eq!(c.keys(Control::NoteLeft)[0], KeyCode::KeyD);
        assert_eq!(c.keys(Control::NoteRight)[0], KeyCode::KeyA);
        // Unbinding shifts the other key left.
        assert!(rebind(&mut c, Control::NoteDown, 0, None));
        assert_eq!(c.keys(Control::NoteDown), &[KeyCode::ArrowDown]);
        // The last UI key can't go.
        assert!(rebind(&mut c, Control::UiUp, 0, None));
        assert!(!rebind(&mut c, Control::UiUp, 0, None));
        // Binding column 1 of an empty row lands in column 0.
        assert!(rebind(&mut c, Control::Reset, 0, None));
        assert!(rebind(&mut c, Control::Reset, 1, Some(KeyCode::KeyT)));
        assert_eq!(c.keys(Control::Reset), &[KeyCode::KeyT]);
    }

    #[test]
    fn deadzone() {
        assert_eq!(deadzone_target(0.0, 300.0, 70.0, 160.0, 400.0, 0.0), 0.0);
        assert_eq!(deadzone_target(0.0, 600.0, 70.0, 160.0, 400.0, 0.0), 110.0);
        assert_eq!(deadzone_target(500.0, 100.0, 70.0, 160.0, 400.0, 0.0), 0.0);
    }
}
