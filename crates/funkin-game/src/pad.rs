//! Gamepads (`FlxG.gamepads`), through gilrs on Windows and Linux.
//!
//! Every gamepad input becomes a key code of its own (key codes no keyboard
//! here sends, see [`PAD_KEYS`]), so the bindings, menus and gameplay treat a
//! button exactly like a key. Sticks count as four digital directions each
//! (`LEFT_STICK_DIGITAL_UP` and so on).

use std::time::{Instant, SystemTime};

use gilrs::{Axis, Button, EventType, Gilrs};
use winit::keyboard::KeyCode;

/// `FlxGamepadInputID`s and the key codes that stand for them: (code, save id, label).
pub const PAD_KEYS: [(KeyCode, &str, &str); 24] = [
    (KeyCode::F19, "A", "A"),
    (KeyCode::F20, "B", "B"),
    (KeyCode::F21, "X", "X"),
    (KeyCode::F22, "Y", "Y"),
    (KeyCode::F23, "LEFT_SHOULDER", "Lb"),
    (KeyCode::F24, "RIGHT_SHOULDER", "Rb"),
    (KeyCode::F25, "LEFT_TRIGGER", "Lt"),
    (KeyCode::F26, "RIGHT_TRIGGER", "Rt"),
    (KeyCode::F27, "BACK", "Back"),
    (KeyCode::F28, "START", "Start"),
    (KeyCode::F29, "LEFT_STICK_CLICK", "Ls"),
    (KeyCode::F30, "RIGHT_STICK_CLICK", "Rs"),
    (KeyCode::F31, "DPAD_UP", "Up"),
    (KeyCode::F32, "DPAD_DOWN", "Down"),
    (KeyCode::F33, "DPAD_LEFT", "Left"),
    (KeyCode::F34, "DPAD_RIGHT", "Right"),
    (KeyCode::F35, "LEFT_STICK_DIGITAL_UP", "L Up"),
    (KeyCode::Lang1, "LEFT_STICK_DIGITAL_DOWN", "L Down"),
    (KeyCode::Lang2, "LEFT_STICK_DIGITAL_LEFT", "L Left"),
    (KeyCode::Lang3, "LEFT_STICK_DIGITAL_RIGHT", "L Right"),
    (KeyCode::Lang4, "RIGHT_STICK_DIGITAL_UP", "R Up"),
    (KeyCode::Lang5, "RIGHT_STICK_DIGITAL_DOWN", "R Down"),
    (KeyCode::Hiragana, "RIGHT_STICK_DIGITAL_LEFT", "R Left"),
    (KeyCode::Katakana, "RIGHT_STICK_DIGITAL_RIGHT", "R Right"),
];

fn code(id: &str) -> KeyCode {
    PAD_KEYS.iter().find(|(_, i, _)| *i == id).map(|(k, _, _)| *k).expect("pad id")
}

/// Is this one of the gamepad key codes?
pub fn is_pad(k: KeyCode) -> bool {
    PAD_KEYS.iter().any(|(c, _, _)| *c == k)
}

pub fn pad_id(k: KeyCode) -> Option<&'static str> {
    PAD_KEYS.iter().find(|(c, _, _)| *c == k).map(|(_, id, _)| *id)
}

pub fn pad_from_id(id: &str) -> Option<KeyCode> {
    PAD_KEYS.iter().find(|(_, i, _)| i.eq_ignore_ascii_case(id)).map(|(k, _, _)| *k)
}

pub fn pad_label(k: KeyCode) -> Option<&'static str> {
    PAD_KEYS.iter().find(|(c, _, _)| *c == k).map(|(_, _, l)| *l)
}

/// `Controls.getDefaultGamepadBinds`, by save id.
pub fn defaults(c: crate::settings::Control) -> Vec<KeyCode> {
    use crate::settings::Control as C;
    let ids: &[&str] = match c {
        C::Accept => &["A"],
        C::Back => &["B"],
        C::UiUp => &["DPAD_UP", "LEFT_STICK_DIGITAL_UP"],
        C::UiDown => &["DPAD_DOWN", "LEFT_STICK_DIGITAL_DOWN"],
        C::UiLeft => &["DPAD_LEFT", "LEFT_STICK_DIGITAL_LEFT"],
        C::UiRight => &["DPAD_RIGHT", "LEFT_STICK_DIGITAL_RIGHT"],
        C::NoteUp => &["DPAD_UP", "Y", "LEFT_STICK_DIGITAL_UP", "RIGHT_STICK_DIGITAL_UP"],
        C::NoteDown => &["DPAD_DOWN", "A", "LEFT_STICK_DIGITAL_DOWN", "RIGHT_STICK_DIGITAL_DOWN"],
        C::NoteLeft => &["DPAD_LEFT", "X", "LEFT_STICK_DIGITAL_LEFT", "RIGHT_STICK_DIGITAL_LEFT"],
        C::NoteRight => &["DPAD_RIGHT", "B", "LEFT_STICK_DIGITAL_RIGHT", "RIGHT_STICK_DIGITAL_RIGHT"],
        C::Pause => &["START"],
        C::Reset => &["BACK"],
        C::CutsceneAdvance => &["A"],
        C::FreeplayFavorite => &["Y"],
        C::FreeplayLeft => &["LEFT_SHOULDER"],
        C::FreeplayRight => &["RIGHT_SHOULDER"],
        C::FreeplayCharSelect => &["X"],
        C::FreeplayJumpToTop => &["RIGHT_STICK_DIGITAL_UP"],
        C::FreeplayJumpToBottom => &["RIGHT_STICK_DIGITAL_DOWN"],
        _ => &[],
    };
    ids.iter().map(|i| code(i)).collect()
}

/// A stick direction counts as pressed past this, and released under `RELEASE`.
const PRESS: f32 = 0.5;
const RELEASE: f32 = 0.4;

pub struct Pads {
    gilrs: Option<Gilrs>,
    /// Digital stick directions held: left up/down/left/right, right up/down/left/right.
    sticks: [bool; 8],
    /// The same for a d-pad that reports as an axis.
    dpad: [bool; 4],
    /// Dev only (`RUSTED_MIC_DEV=fakepad`): act as if a gamepad is connected.
    pub fake: bool,
}

impl Pads {
    pub fn new() -> Self {
        let gilrs = match Gilrs::new() {
            Ok(g) => Some(g),
            Err(gilrs::Error::NotImplemented(g)) => {
                log::info!("gamepads: not supported on this system");
                Some(g)
            }
            Err(e) => {
                log::warn!("gamepads: {e}");
                None
            }
        };
        if let Some(g) = &gilrs {
            for (_, p) in g.gamepads() {
                log::info!("gamepad: {}", p.name());
            }
        }
        Self { gilrs, sticks: [false; 8], dpad: [false; 4], fake: false }
    }

    /// `FlxG.gamepads.numActiveGamepads > 0`.
    pub fn connected(&self) -> bool {
        self.fake || self.gilrs.as_ref().is_some_and(|g| g.gamepads().any(|(_, p)| p.is_connected()))
    }

    /// Inputs since the last call: (key code, pressed, when).
    pub fn poll(&mut self) -> Vec<(KeyCode, bool, Instant)> {
        let mut out = Vec::new();
        let Some(g) = &mut self.gilrs else { return out };
        let (now, sys_now) = (Instant::now(), SystemTime::now());
        while let Some(ev) = g.next_event() {
            let at = sys_now.duration_since(ev.time).ok().and_then(|d| now.checked_sub(d)).unwrap_or(now);
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(k) = button(b) {
                        out.push((k, true, at));
                    }
                }
                EventType::ButtonReleased(b, _) => {
                    if let Some(k) = button(b) {
                        out.push((k, false, at));
                    }
                }
                EventType::AxisChanged(axis, v, _) => {
                    // gilrs: up is positive.
                    let (base, neg, pos) = match axis {
                        Axis::LeftStickX => (0, 2, 3),
                        Axis::LeftStickY => (0, 1, 0),
                        Axis::RightStickX => (4, 2, 3),
                        Axis::RightStickY => (4, 1, 0),
                        Axis::DPadX => (8, 2, 3),
                        Axis::DPadY => (8, 1, 0),
                        _ => continue,
                    };
                    for (dir, on) in [(neg, -v), (pos, v)] {
                        if base == 8 {
                            // A d-pad reported as an axis.
                            let k = [code("DPAD_UP"), code("DPAD_DOWN"), code("DPAD_LEFT"), code("DPAD_RIGHT")][dir];
                            let was = self.dpad[dir];
                            let now_on = if was { on > RELEASE } else { on > PRESS };
                            if now_on != was {
                                self.dpad[dir] = now_on;
                                out.push((k, now_on, at));
                            }
                            continue;
                        }
                        let i = base + dir;
                        let was = self.sticks[i];
                        let now_on = if was { on > RELEASE } else { on > PRESS };
                        if now_on != was {
                            self.sticks[i] = now_on;
                            let id = ["LEFT_STICK_DIGITAL_UP", "LEFT_STICK_DIGITAL_DOWN", "LEFT_STICK_DIGITAL_LEFT", "LEFT_STICK_DIGITAL_RIGHT", "RIGHT_STICK_DIGITAL_UP", "RIGHT_STICK_DIGITAL_DOWN", "RIGHT_STICK_DIGITAL_LEFT", "RIGHT_STICK_DIGITAL_RIGHT"][i];
                            out.push((code(id), now_on, at));
                        }
                    }
                }
                EventType::Connected => log::info!("gamepad connected: {}", g.gamepad(ev.id).name()),
                EventType::Disconnected => {
                    log::info!("gamepad disconnected");
                    // Let go of everything it held.
                    for (i, held) in self.sticks.iter_mut().enumerate() {
                        if std::mem::take(held) {
                            let id = ["LEFT_STICK_DIGITAL_UP", "LEFT_STICK_DIGITAL_DOWN", "LEFT_STICK_DIGITAL_LEFT", "LEFT_STICK_DIGITAL_RIGHT", "RIGHT_STICK_DIGITAL_UP", "RIGHT_STICK_DIGITAL_DOWN", "RIGHT_STICK_DIGITAL_LEFT", "RIGHT_STICK_DIGITAL_RIGHT"][i];
                            out.push((code(id), false, at));
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

}

fn button(b: Button) -> Option<KeyCode> {
    Some(code(match b {
        Button::South => "A",
        Button::East => "B",
        Button::West => "X",
        Button::North => "Y",
        Button::LeftTrigger => "LEFT_SHOULDER",
        Button::RightTrigger => "RIGHT_SHOULDER",
        Button::LeftTrigger2 => "LEFT_TRIGGER",
        Button::RightTrigger2 => "RIGHT_TRIGGER",
        Button::Select => "BACK",
        Button::Start => "START",
        Button::LeftThumb => "LEFT_STICK_CLICK",
        Button::RightThumb => "RIGHT_STICK_CLICK",
        Button::DPadUp => "DPAD_UP",
        Button::DPadDown => "DPAD_DOWN",
        Button::DPadLeft => "DPAD_LEFT",
        Button::DPadRight => "DPAD_RIGHT",
        _ => return None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for (k, id, _) in PAD_KEYS {
            assert_eq!(pad_from_id(id), Some(k));
            assert_eq!(pad_id(k), Some(id));
        }
        assert!(!is_pad(KeyCode::KeyA));
        assert_eq!(defaults(crate::settings::Control::Accept), vec![KeyCode::F19]);
    }
}
