//! Player settings (`Preferences.hx` + `Save.options`) and key bindings
//! (`Controls.hx`), saved as JSON in the data folder.
//!
//! Some settings are new to Rusted Mic: **Middlescroll** (the player's notes in
//! the middle of the screen), **Hide Opponent Notes**, **Ghost Tapping** (the
//! original only has it on mobile builds), and Psych Engine's **time bar** and
//! **health bar**.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use winit::keyboard::KeyCode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugDisplay {
    Off,
    Simple,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VSync {
    Off,
    On,
    Adaptive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub naughtyness: bool,
    pub downscroll: bool,
    /// New: the player strumline sits in the middle of the screen.
    pub middlescroll: bool,
    /// New: the opponent's strumline and notes are not drawn.
    pub hide_opponent_notes: bool,
    /// New on desktop: pressing a key with no note to hit isn't penalized.
    pub ghost_tapping: bool,
    /// New: Psych Engine's song time bar at the top of the screen.
    pub psych_time_bar: bool,
    /// New: Psych Engine's health bar, in the characters' colours.
    pub psych_health_bar: bool,
    /// Percent (0..=100).
    pub strumline_background: i32,
    /// New: splashes on sick hits (V-Slice's `Strumline.showNotesplash`).
    pub note_splashes: bool,
    /// New: smooth texture filtering; off draws everything nearest-neighbour.
    pub antialiasing: bool,
    pub flashing_lights: bool,
    pub zoom_camera: bool,
    pub subtitles: bool,
    pub debug_display: DebugDisplay,
    /// Percent (0..=100).
    pub debug_display_bg: i32,
    pub auto_pause: bool,
    pub auto_fullscreen: bool,
    pub vsync: VSync,
    pub unlocked_framerate: bool,
    pub framerate: i32,
    pub screenshot_hide_mouse: bool,
    pub screenshot_fancy_preview: bool,
    pub screenshot_preview_on_save: bool,
    pub discord_rpc: bool,
    /// Global input/audio offset in ms (`Preferences.globalOffset`).
    pub global_offset: i32,
    /// Master volume, 0..=10 steps of 10% (`FlxG.sound.volume`).
    pub volume: i32,
    pub muted: bool,
    pub controls: Controls,
    /// Discord application id for Rich Presence (see `discord.rs`).
    pub discord_app_id: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            naughtyness: true,
            downscroll: false,
            middlescroll: false,
            hide_opponent_notes: false,
            ghost_tapping: false,
            psych_time_bar: false,
            psych_health_bar: false,
            strumline_background: 0,
            note_splashes: true,
            antialiasing: true,
            flashing_lights: true,
            zoom_camera: true,
            subtitles: true,
            debug_display: DebugDisplay::Off,
            debug_display_bg: 50,
            auto_pause: true,
            auto_fullscreen: false,
            vsync: VSync::Off,
            unlocked_framerate: false,
            framerate: 60,
            screenshot_hide_mouse: true,
            screenshot_fancy_preview: true,
            screenshot_preview_on_save: true,
            discord_rpc: true,
            global_offset: 0,
            volume: 10,
            muted: false,
            controls: Controls::default(),
            discord_app_id: String::new(),
        }
    }
}

/// `%LOCALAPPDATA%\Rusted Mic` on Windows, `~/.local/share/Rusted Mic` on
/// Linux (or `RUSTED_MIC_DATA_DIR`).
pub fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("RUSTED_MIC_DATA_DIR") {
        return PathBuf::from(d);
    }
    dirs::data_local_dir().unwrap_or_else(|| PathBuf::from(".")).join("Rusted Mic")
}

pub fn settings_path() -> PathBuf {
    data_dir().join("settings.json")
}

impl Settings {
    pub fn load() -> Self {
        let p = settings_path();
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str::<Settings>(&s)
                .map(|mut s| {
                    s.controls.fill_missing();
                    s.clamp();
                    s
                })
                .unwrap_or_else(|e| {
                    log::warn!("{}: {e}; using defaults", p.display());
                    Self::default()
                }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let p = settings_path();
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        match serde_json::to_string_pretty(self) {
            Ok(s) => {
                // Write then rename, so a crash never leaves half a file.
                let tmp = p.with_extension("json.tmp");
                if let Err(e) = std::fs::write(&tmp, s).and_then(|_| std::fs::rename(&tmp, &p)) {
                    log::warn!("could not save {}: {e}", p.display());
                }
            }
            Err(e) => log::warn!("settings: {e}"),
        }
    }

    fn clamp(&mut self) {
        self.strumline_background = self.strumline_background.clamp(0, 100);
        self.debug_display_bg = self.debug_display_bg.clamp(0, 100);
        self.framerate = self.framerate.clamp(30, 500);
        self.global_offset = self.global_offset.clamp(-1500, 1500);
        self.volume = self.volume.clamp(0, 10);
    }

    /// The gain applied to all sound (`FlxG.sound.volume`, with Flixel's sound curve).
    pub fn master_gain(&self) -> f32 {
        if self.muted {
            0.0
        } else {
            let v = self.volume as f32 / 10.0;
            v * v
        }
    }
}

/// Every bindable action (`Control.hx`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Control {
    NoteLeft,
    NoteDown,
    NoteUp,
    NoteRight,
    UiUp,
    UiDown,
    UiLeft,
    UiRight,
    Accept,
    Back,
    Pause,
    Reset,
    CutsceneAdvance,
    FreeplayFavorite,
    FreeplayLeft,
    FreeplayRight,
    FreeplayCharSelect,
    FreeplayJumpToTop,
    FreeplayJumpToBottom,
    WindowFullscreen,
    WindowScreenshot,
    VolumeUp,
    VolumeDown,
    VolumeMute,
    /// Debug menu / chart editor / stage editor: listed and rebindable as in
    /// the original, but there are no editors to open.
    DebugMenu,
    DebugChart,
    DebugStage,
    DebugDisplay,
}

impl Control {
    pub const ALL: [Control; 28] = [
        Control::NoteLeft,
        Control::NoteDown,
        Control::NoteUp,
        Control::NoteRight,
        Control::UiUp,
        Control::UiDown,
        Control::UiLeft,
        Control::UiRight,
        Control::Accept,
        Control::Back,
        Control::Pause,
        Control::Reset,
        Control::CutsceneAdvance,
        Control::FreeplayFavorite,
        Control::FreeplayLeft,
        Control::FreeplayRight,
        Control::FreeplayCharSelect,
        Control::FreeplayJumpToTop,
        Control::FreeplayJumpToBottom,
        Control::WindowFullscreen,
        Control::WindowScreenshot,
        Control::VolumeUp,
        Control::VolumeDown,
        Control::VolumeMute,
        Control::DebugMenu,
        Control::DebugChart,
        Control::DebugStage,
        Control::DebugDisplay,
    ];

    /// `Controls.getDefaultKeybinds(Solo, control)`.
    pub fn defaults(self) -> Vec<KeyCode> {
        use KeyCode as K;
        match self {
            Control::UiUp | Control::NoteUp => vec![K::KeyW, K::ArrowUp],
            Control::UiDown | Control::NoteDown => vec![K::KeyS, K::ArrowDown],
            Control::UiLeft | Control::NoteLeft => vec![K::KeyA, K::ArrowLeft],
            Control::UiRight | Control::NoteRight => vec![K::KeyD, K::ArrowRight],
            Control::Accept => vec![K::KeyZ, K::Space, K::Enter],
            Control::Back => vec![K::KeyX, K::Backspace, K::Escape],
            Control::Pause => vec![K::KeyP, K::Enter, K::Escape],
            Control::Reset => vec![K::KeyR],
            Control::CutsceneAdvance => vec![K::KeyZ, K::Enter],
            Control::FreeplayFavorite => vec![K::KeyF],
            Control::FreeplayLeft => vec![K::KeyQ],
            Control::FreeplayRight => vec![K::KeyE],
            Control::FreeplayCharSelect => vec![K::Tab],
            Control::FreeplayJumpToTop => vec![K::Home],
            Control::FreeplayJumpToBottom => vec![K::End],
            Control::WindowFullscreen => vec![K::F11],
            Control::WindowScreenshot => vec![K::F3],
            Control::VolumeUp => vec![K::Equal, K::NumpadAdd],
            Control::VolumeDown => vec![K::Minus, K::NumpadSubtract],
            Control::VolumeMute => vec![K::Digit0, K::Numpad0],
            Control::DebugMenu => vec![K::Backquote],
            Control::DebugChart | Control::DebugStage => vec![],
            Control::DebugDisplay => vec![K::F6],
        }
    }

    /// The label in the controls menu (`InputItem`).
    pub fn label(self) -> &'static str {
        match self {
            Control::NoteLeft => "LEFT",
            Control::NoteDown => "DOWN",
            Control::NoteUp => "UP",
            Control::NoteRight => "RIGHT",
            Control::UiUp => "UP",
            Control::UiDown => "DOWN",
            Control::UiLeft => "LEFT",
            Control::UiRight => "RIGHT",
            Control::Accept => "ACCEPT",
            Control::Back => "BACK",
            Control::Pause => "PAUSE",
            Control::Reset => "RESET",
            Control::CutsceneAdvance => "ADVANCE",
            Control::FreeplayFavorite => "FAVORITE",
            Control::FreeplayLeft => "LEFT",
            Control::FreeplayRight => "RIGHT",
            Control::FreeplayCharSelect => "CHAR SELECT",
            Control::FreeplayJumpToTop => "JUMP TO TOP",
            Control::FreeplayJumpToBottom => "JUMP TO BOTTOM",
            Control::WindowFullscreen => "FULLSCREEN",
            Control::WindowScreenshot => "SCREENSHOT",
            Control::VolumeUp => "UP",
            Control::VolumeDown => "DOWN",
            Control::VolumeMute => "MUTE",
            Control::DebugMenu => "MENU",
            Control::DebugChart => "CHART",
            Control::DebugStage => "STAGE",
            Control::DebugDisplay => "DISPLAY",
        }
    }
}

/// Key bindings: each control's keys in order (the menu edits the first two).
#[derive(Debug, Clone, PartialEq)]
pub struct Controls {
    pub binds: BTreeMap<Control, Vec<KeyCode>>,
}

impl Default for Controls {
    fn default() -> Self {
        Self { binds: Control::ALL.iter().map(|&c| (c, c.defaults())).collect() }
    }
}

impl Serialize for Controls {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let m: BTreeMap<Control, Vec<&str>> =
            self.binds.iter().map(|(c, keys)| (*c, keys.iter().filter_map(|&k| key_id(k)).collect())).collect();
        m.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Controls {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Unknown controls (from newer versions) are ignored rather than failing.
        let m: BTreeMap<String, Vec<String>> = BTreeMap::deserialize(d)?;
        let mut binds = BTreeMap::new();
        for (name, keys) in m {
            let Ok(c) = serde_json::from_value::<Control>(serde_json::Value::String(name)) else { continue };
            binds.insert(c, keys.iter().filter_map(|k| key_from_id(k)).collect());
        }
        let mut c = Controls { binds };
        c.fill_missing();
        Ok(c)
    }
}

impl Controls {
    fn fill_missing(&mut self) {
        for c in Control::ALL {
            self.binds.entry(c).or_insert_with(|| c.defaults());
        }
    }

    pub fn keys(&self, c: Control) -> &[KeyCode] {
        self.binds.get(&c).map(|v| v.as_slice()).unwrap_or(&[])
    }

    pub fn is(&self, c: Control, code: KeyCode) -> bool {
        self.keys(c).contains(&code)
    }

    /// The note lane a key plays, if any.
    pub fn note_lane(&self, code: KeyCode) -> Option<usize> {
        [Control::NoteLeft, Control::NoteDown, Control::NoteUp, Control::NoteRight]
            .iter()
            .position(|&c| self.is(c, code))
    }

    /// Menus are written against fixed keys; turn a pressed key into the key
    /// a menu expects for the control it's bound to: arrows, Enter (accept),
    /// Escape (back), and F13..F18 for the freeplay controls. Keys bound to
    /// none of these return `None`, so unbinding a key really disables it.
    pub fn menu_key(&self, code: KeyCode) -> Option<KeyCode> {
        use KeyCode as K;
        const MAP: [(Control, KeyCode); 12] = [
            (Control::UiUp, K::ArrowUp),
            (Control::UiDown, K::ArrowDown),
            (Control::UiLeft, K::ArrowLeft),
            (Control::UiRight, K::ArrowRight),
            (Control::Accept, K::Enter),
            (Control::Back, K::Escape),
            (Control::FreeplayFavorite, K::F13),
            (Control::FreeplayLeft, K::F14),
            (Control::FreeplayRight, K::F15),
            (Control::FreeplayCharSelect, K::F16),
            (Control::FreeplayJumpToTop, K::F17),
            (Control::FreeplayJumpToBottom, K::F18),
        ];
        MAP.iter().find(|(c, _)| self.is(*c, code)).map(|&(_, k)| k)
    }
}

macro_rules! keys {
    ($($code:ident => $id:literal, $label:literal;)*) => {
        const KEYS: &[(KeyCode, &str, &str)] = &[$((KeyCode::$code, $id, $label)),*];
    };
}

keys! {
    KeyA => "A", "A"; KeyB => "B", "B"; KeyC => "C", "C"; KeyD => "D", "D"; KeyE => "E", "E";
    KeyF => "F", "F"; KeyG => "G", "G"; KeyH => "H", "H"; KeyI => "I", "I"; KeyJ => "J", "J";
    KeyK => "K", "K"; KeyL => "L", "L"; KeyM => "M", "M"; KeyN => "N", "N"; KeyO => "O", "O";
    KeyP => "P", "P"; KeyQ => "Q", "Q"; KeyR => "R", "R"; KeyS => "S", "S"; KeyT => "T", "T";
    KeyU => "U", "U"; KeyV => "V", "V"; KeyW => "W", "W"; KeyX => "X", "X"; KeyY => "Y", "Y";
    KeyZ => "Z", "Z";
    Digit0 => "ZERO", "0"; Digit1 => "ONE", "1"; Digit2 => "TWO", "2"; Digit3 => "THREE", "3";
    Digit4 => "FOUR", "4"; Digit5 => "FIVE", "5"; Digit6 => "SIX", "6"; Digit7 => "SEVEN", "7";
    Digit8 => "EIGHT", "8"; Digit9 => "NINE", "9";
    Numpad0 => "NUMPADZERO", "#0"; Numpad1 => "NUMPADONE", "#1"; Numpad2 => "NUMPADTWO", "#2";
    Numpad3 => "NUMPADTHREE", "#3"; Numpad4 => "NUMPADFOUR", "#4"; Numpad5 => "NUMPADFIVE", "#5";
    Numpad6 => "NUMPADSIX", "#6"; Numpad7 => "NUMPADSEVEN", "#7"; Numpad8 => "NUMPADEIGHT", "#8";
    Numpad9 => "NUMPADNINE", "#9"; NumpadAdd => "NUMPADPLUS", "#+"; NumpadSubtract => "NUMPADMINUS", "#-";
    NumpadMultiply => "NUMPADMULTIPLY", "#*"; NumpadDivide => "NUMPADSLASH", "Numpadslash";
    NumpadDecimal => "NUMPADPERIOD", "#."; NumpadEnter => "NUMPADENTER", "Enter";
    ArrowLeft => "LEFT", "Left"; ArrowDown => "DOWN", "Down"; ArrowUp => "UP", "Up"; ArrowRight => "RIGHT", "Right";
    Space => "SPACE", "Space"; Enter => "ENTER", "Enter"; Escape => "ESCAPE", "Escape"; Backspace => "BACKSPACE", "BckSpc";
    Tab => "TAB", "Tab"; CapsLock => "CAPSLOCK", "Caps";
    ShiftLeft => "SHIFT", "Shift"; ShiftRight => "RSHIFT", "Shift";
    ControlLeft => "CONTROL", "Ctrl"; ControlRight => "RCONTROL", "Ctrl";
    AltLeft => "ALT", "Alt"; AltRight => "RALT", "Alt";
    Minus => "MINUS", "Minus"; Equal => "PLUS", "Plus"; BracketLeft => "LBRACKET", "["; BracketRight => "RBRACKET", "]";
    Backslash => "BACKSLASH", "\\"; Semicolon => "SEMICOLON", ";"; Quote => "QUOTE", "'";
    Comma => "COMMA", ","; Period => "PERIOD", "."; Slash => "SLASH", "/"; Backquote => "GRAVEACCENT", "`";
    Home => "HOME", "Home"; End => "END", "End"; PageUp => "PAGEUP", "PgUp"; PageDown => "PAGEDOWN", "PgDown";
    Insert => "INSERT", "Insert"; Delete => "DELETE", "Delete";
    F1 => "F1", "F1"; F2 => "F2", "F2"; F3 => "F3", "F3"; F4 => "F4", "F4"; F5 => "F5", "F5"; F6 => "F6", "F6";
    F7 => "F7", "F7"; F8 => "F8", "F8"; F9 => "F9", "F9"; F10 => "F10", "F10"; F11 => "F11", "F11"; F12 => "F12", "F12";
}

/// Save-file name of a key (Flixel `FlxKey` names), if it can be bound.
pub fn key_id(code: KeyCode) -> Option<&'static str> {
    KEYS.iter().find(|(k, _, _)| *k == code).map(|(_, id, _)| *id)
}

pub fn key_from_id(id: &str) -> Option<KeyCode> {
    KEYS.iter().find(|(_, i, _)| i.eq_ignore_ascii_case(id)).map(|(k, _, _)| *k)
}

/// How a key is shown in the controls menu (`InputFormatter`).
pub fn key_label(code: KeyCode) -> &'static str {
    KEYS.iter().find(|(k, _, _)| *k == code).map(|(_, _, l)| *l).unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut s = Settings::default();
        s.controls.binds.insert(Control::NoteLeft, vec![KeyCode::KeyJ]);
        s.middlescroll = true;
        let j = serde_json::to_string(&s).unwrap();
        let back: Settings = serde_json::from_str(&j).unwrap();
        assert_eq!(back.controls, s.controls);
        assert!(back.middlescroll);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let s: Settings = serde_json::from_str(r#"{"downscroll": true, "controls": {"Accept": ["Q"]}}"#).unwrap();
        assert!(s.downscroll);
        assert_eq!(s.controls.keys(Control::Accept), &[KeyCode::KeyQ]);
        assert_eq!(s.controls.keys(Control::Back), Control::Back.defaults().as_slice());
        assert_eq!(s.framerate, 60);
    }

    #[test]
    fn menu_keys_follow_bindings() {
        let mut c = Controls::default();
        assert_eq!(c.menu_key(KeyCode::KeyW), Some(KeyCode::ArrowUp));
        c.binds.insert(Control::UiUp, vec![KeyCode::ArrowUp]);
        assert_eq!(c.menu_key(KeyCode::KeyW), None);
        assert_eq!(c.note_lane(KeyCode::KeyD), Some(3));
    }
}
