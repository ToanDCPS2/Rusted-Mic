//! Runs the game's `.hxc` scripts (and mods') through `funkin-script`:
//! loads `assets/scripts`, binds the song's stage, song and character script
//! classes like Polymod does, forwards PlayState's script events to them, and
//! exposes the engine (props, characters, sounds, tweens, timers) as Haxe objects.
//!
//! Engine APIs that aren't bound yet resolve to inert stand-ins, so a script
//! keeps running past them; each one is logged once.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use funkin_script::{Host, HostRef, Instance, Interp, Value};

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::Audio;
use crate::gfx::Gfx;
use crate::world::World;

// Host object kinds.
const K_CLASS: u32 = 1;
const K_STAGE: u32 = 2;
const K_SPRITE: u32 = 3;
const K_ANIM: u32 = 4;
const K_POINT: u32 = 5;
const K_PLAYSTATE: u32 = 6;
const K_SONG: u32 = 7;
const K_TWEEN: u32 = 8;
const K_TIMER: u32 = 9;
const K_STUB: u32 = 10;
const K_SCAN: u32 = 11;
const K_CAMERA: u32 = 12;
const K_CONDUCTOR: u32 = 13;
const K_SOUND: u32 = 14;
/// `animation.onFinish` (id = sprite) / `onFrameChange` (sprite | 1 << 40) / `onFrameLabel` (sprite | 2 << 40).
const K_SIGNAL: u32 = 15;
/// `PlayState.instance.cameraFollowPoint`.
const K_FOLLOW: u32 = 20;
/// `VideoCutscene.onVideoStarted` .. `onVideoEnded` (ids 0..5, see [`VIDEO_SIGNALS`]).
const K_VSIGNAL: u32 = 21;
/// `PlayState.playerStrumline` (id 0) / `opponentStrumline` (1); `| 2` for one of its members.
const K_STRUM: u32 = 22;
/// A shader (`new AdjustColorShader()`...; id indexes `HostState::shaders`).
const K_SHADER: u32 = 23;
/// `new ShaderFilter(shader)` (id = the shader's) / `new BlurFilter()` ([`NO_FILTER`]).
const K_FILTER: u32 = 24;
const NO_FILTER: u64 = u64::MAX;
/// `PlayState.instance.comboPopUps`.
const K_POPUPS: u32 = 25;
/// A shader's parameter (`shader.name`, id = shader << 20 | index into `HostState::param_names`), for `.value` / `.input`.
const K_SPARAM: u32 = 26;
/// A sprite's `pixels` (id = the texture).
const K_BITMAP: u32 = 27;
/// A `FunkTrail` (index into the world's trails).
const K_TRAIL: u32 = 28;
/// `FlxTimerManager`: timers started with it can be cleared together.
const K_TMGR: u32 = 29;
/// `animation.curAnim` (id = the sprite).
const K_CURANIM: u32 = 30;
/// `FlxSkewedSprite.transformMatrix` (id = the sprite).
const K_MATRIX: u32 = 31;
/// `FlxTypedSpriteGroup` (index into `HostState::groups`).
const K_GROUP: u32 = 32;
/// `PlayState.instance.controls`.
const K_CONTROLS: u32 = 33;
/// `FlxText` (index into `HostState::texts`).
const K_STEXT: u32 = 34;
/// `Conductor.beatHit`: the song's beat signal.
const K_BEATSIG: u32 = 35;
/// `FlxG.keys` (0), `.pressed` (1), `.justPressed` (2), `.justReleased` (3).
const K_KEYS: u32 = 36;
/// A camera's `scroll` (id = camera) or `targetOffset` (id = camera | 1 << 8).
const K_CAMPT: u32 = 37;
/// `FlxG.cameras`.
const K_CAMLIST: u32 = 38;
/// PlayState's HUD objects (`HUD_*` ids); `id | 1 << 8` is an icon's `animation`.
const K_HUD: u32 = 39;
/// A strumline's child group: `side << 8 | SG_*`.
const K_SGROUP: u32 = 40;
/// A receptor (`StrumlineNote`): `side << 8 | lane`.
const K_RECEPTOR: u32 = 41;
/// A `FunkinVideoSprite`'s `bitmap` (hxvlc's FlxVideoSprite), by sprite id.
const K_VBITMAP: u32 = 45;
/// A character's `cameraFocusPoint` (id: the character's sprite id).
const K_CFOCUS: u32 = 46;
/// A character's `originalPosition` (id = the character's sprite id).
const K_ORIGPOS: u32 = 47;
/// A `NoteSprite`: `side << 32 | note index`.
const K_NOTE: u32 = 42;
/// A note's `holdNoteSprite` (`SustainTrail`), same id as the note.
const K_HOLD: u32 = 43;
/// A note's `scale` (same id as the note), or with `SCALE_LINE` a strumline's.
const K_NSCALE: u32 = 44;
const SCALE_LINE: u64 = 1 << 62;

/// A strumline's child groups (`strumline.members` in this order).
pub const SG_STRUMS: usize = 0;
pub const SG_NOTES: usize = 1;
pub const SG_HOLDS: usize = 2;
pub const SG_SPLASHES: usize = 3;
pub const SG_COVERS: usize = 4;
pub const SG_BG: usize = 5;

/// A note as scripts see it this frame (V-Slice coordinates: sprite top-left).
#[derive(Clone, Default)]
pub struct NoteView {
    pub idx: usize,
    pub dir: usize,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub has_hold: bool,
    pub hold_x: f32,
    pub hold_y: f32,
    pub sustain: f64,
    pub full_sustain: f64,
    pub hit: bool,
    pub missed: bool,
    pub strum_time: f64,
}

/// One `Strumline` (0 = player, 1 = opponent): what scripts changed, and what the
/// engine published for them to read.
pub struct StrumState {
    /// `x` / `y` moved from where the engine puts the strumline.
    pub off: [f32; 2],
    pub alpha: f32,
    pub visible: bool,
    /// `scale.set(...)`: the receptors' scale (absolute, like FlxSpriteGroup's).
    pub scale: Option<[f32; 2]>,
    /// The camera it draws on (1 = camHUD, 3+ = a script FlxCamera).
    pub cam: u64,
    /// Child groups (`SG_*`).
    pub part_visible: [bool; 6],
    pub part_alpha: [f32; 6],
    /// Receptors moved by scripts, and their own alpha / visibility.
    pub lane_off: [[f32; 2]; 4],
    pub lane_alpha: [f32; 4],
    pub lane_visible: [bool; 4],
    // Published by PlayState each frame.
    /// The engine's strumline x / y.
    pub base: [f32; 2],
    pub width: f32,
    pub height: f32,
    /// Receptor top-left and size without script offsets.
    pub receptors: [[f32; 4]; 4],
    pub anims: [&'static str; 4],
    pub key_held: [bool; 4],
    /// Notes in view (not yet hit), and every hold trail in view.
    pub notes: Vec<NoteView>,
    pub holds: Vec<NoteView>,
    // Per-note changes, by note index.
    pub note_x: HashMap<usize, f32>,
    pub note_scale: HashMap<usize, [f32; 2]>,
    pub note_alpha: HashMap<usize, f32>,
    pub hold_x: HashMap<usize, f32>,
    pub hold_alpha: HashMap<usize, f32>,
    /// `y` moves this frame (the engine places notes again every frame).
    pub dy: HashMap<usize, f32>,
    pub hold_dy: HashMap<usize, f32>,
}

impl Default for StrumState {
    fn default() -> Self {
        Self {
            off: [0.0; 2],
            alpha: 1.0,
            visible: true,
            scale: None,
            cam: 1,
            part_visible: [true; 6],
            part_alpha: [1.0; 6],
            lane_off: [[0.0; 2]; 4],
            lane_alpha: [1.0; 4],
            lane_visible: [true; 4],
            base: [0.0; 2],
            width: 448.0,
            height: 104.0,
            receptors: [[0.0; 4]; 4],
            anims: ["static"; 4],
            key_held: [false; 4],
            notes: vec![],
            holds: vec![],
            note_x: HashMap::new(),
            note_scale: HashMap::new(),
            note_alpha: HashMap::new(),
            hold_x: HashMap::new(),
            hold_alpha: HashMap::new(),
            dy: HashMap::new(),
            hold_dy: HashMap::new(),
        }
    }
}

impl StrumState {
    fn view(&self, idx: usize) -> Option<&NoteView> {
        self.notes.iter().find(|n| n.idx == idx).or_else(|| self.holds.iter().find(|n| n.idx == idx))
    }
}

/// `event.note` for note `idx` on `side` (0 = player).
pub fn note_ref(side: usize, idx: usize) -> Value {
    Value::Host(HostRef { kind: K_NOTE, id: (side as u64) << 32 | idx as u64 })
}
pub const HUD_BAR_BG: usize = 0;
pub const HUD_BAR: usize = 1;
pub const HUD_ICON_P1: usize = 2;
pub const HUD_ICON_P2: usize = 3;
pub const HUD_SCORE: usize = 4;

/// A script signal (`add`, `addOnce`, `remove`, `removeAll`, `dispatch`) the engine fires.
#[derive(Clone)]
pub(crate) struct Signal {
    pub obj: Value,
    list: Rc<RefCell<Vec<(Value, bool)>>>,
}

impl Signal {
    pub fn new() -> Self {
        let list: Rc<RefCell<Vec<(Value, bool)>>> = Rc::default();
        let add = |once: bool| {
            let l = list.clone();
            Value::native(move |_, _, _, a| {
                // hxvlc's `add(fn, once = false)` / FlxSignal's `addOnce(fn)`.
                let once = once || arg(&a, 1).truthy();
                l.borrow_mut().push((arg(&a, 0), once));
                Ok(Value::Null)
            })
        };
        let l = list.clone();
        let remove = Value::native(move |_, _, _, a| {
            let f = arg(&a, 0);
            l.borrow_mut().retain(|(g, _)| !g.equals(&f));
            Ok(Value::Null)
        });
        let l = list.clone();
        let remove_all = Value::native(move |_, _, _, _| {
            l.borrow_mut().clear();
            Ok(Value::Null)
        });
        let l = list.clone();
        let dispatch = Value::native(move |interp, host, _, a| {
            let fs: Vec<Value> = l.borrow().iter().map(|(f, _)| f.clone()).collect();
            l.borrow_mut().retain(|(_, once)| !once);
            for f in fs {
                interp.call(host, &f, a.clone())?;
            }
            Ok(Value::Null)
        });
        let obj = Value::object(vec![("add", add(false)), ("addOnce", add(true)), ("remove", remove), ("removeAll", remove_all), ("dispatch", dispatch)]);
        Self { obj, list }
    }

    /// Call every listener (once-listeners are dropped first).
    pub fn fire(&self, interp: &mut Interp, host: &mut dyn Host) {
        let fs: Vec<Value> = self.list.borrow().iter().map(|(f, _)| f.clone()).collect();
        self.list.borrow_mut().retain(|(_, once)| !once);
        for f in fs {
            if let Err(e) = interp.call(host, &f, vec![]) {
                log::warn!("script signal: {e}");
            }
        }
    }
}

/// A `FunkinVideoSprite`: the video, its state and `bitmap`'s signals.
pub(crate) struct VideoSprite {
    video: Option<crate::video::Video>,
    playing: bool,
    paused: bool,
    volume: f32,
    /// onEndReached, onFormatSetup, onEncounteredError, onOpening
    signals: [Signal; 4],
    formatted: bool,
}

/// An `FlxBar`'s value: its own, or read from `parent.variable` every frame.
pub(crate) struct BarBind {
    parent: Value,
    variable: String,
    min: f64,
    max: f64,
    value: f64,
}

/// What scripts did to one of PlayState's HUD objects (`iconP1`, `scoreText`, ...),
/// and where the engine last drew it (for reads like `healthBarBG.y`).
pub struct HudElem {
    pub visible: bool,
    pub alpha: f32,
    /// x / y set by a script (`setPosition`, `x = ...`).
    pub pos: [Option<f32>; 2],
    pub flip: Option<bool>,
    /// HealthIcon `autoUpdate`: off, the icon keeps the script's position and face.
    pub auto_update: bool,
    /// `animation.play(name)` on an icon: "idle", "losing", "winning".
    pub anim: Option<String>,
    pub z: Option<i32>,
    /// HealthIcon `bopEvery` / `bopAngle`.
    pub bop_every: i64,
    pub bop_angle: f32,
    /// The last drawn rectangle (x, y, w, h).
    pub rect: std::cell::Cell<[f32; 4]>,
}

impl Default for HudElem {
    fn default() -> Self {
        Self { visible: true, alpha: 1.0, pos: [None; 2], flip: None, auto_update: true, anim: None, z: None, bop_every: 4, bop_angle: 0.0, rect: Default::default() }
    }
}

/// Flixel key names and codes (`FlxKey`, OpenFL keyboard codes).
const FLX_KEYS: &[(&str, i64)] = &[
    ("SPACE", 32), ("ENTER", 13), ("ESCAPE", 27), ("BACKSPACE", 8), ("TAB", 9), ("SHIFT", 16), ("CONTROL", 17), ("ALT", 18),
    ("LEFT", 37), ("UP", 38), ("RIGHT", 39), ("DOWN", 40), ("ZERO", 48), ("ONE", 49), ("TWO", 50), ("THREE", 51), ("FOUR", 52),
    ("FIVE", 53), ("SIX", 54), ("SEVEN", 55), ("EIGHT", 56), ("NINE", 57), ("A", 65), ("B", 66), ("C", 67), ("D", 68), ("E", 69),
    ("F", 70), ("G", 71), ("H", 72), ("I", 73), ("J", 74), ("K", 75), ("L", 76), ("M", 77), ("N", 78), ("O", 79), ("P", 80),
    ("Q", 81), ("R", 82), ("S", 83), ("T", 84), ("U", 85), ("V", 86), ("W", 87), ("X", 88), ("Y", 89), ("Z", 90),
    ("F1", 112), ("F2", 113), ("F3", 114), ("F4", 115), ("F5", 116), ("F6", 117), ("F7", 118), ("F8", 119), ("F9", 120),
    ("F10", 121), ("F11", 122), ("F12", 123), ("SEMICOLON", 186), ("PLUS", 187), ("COMMA", 188), ("MINUS", 189), ("PERIOD", 190),
    ("SLASH", 191), ("GRAVEACCENT", 192), ("LBRACKET", 219), ("BACKSLASH", 220), ("RBRACKET", 221), ("QUOTE", 222),
    ("DELETE", 46), ("HOME", 36), ("END", 35), ("PAGEUP", 33), ("PAGEDOWN", 34), ("INSERT", 45),
];

/// The Flixel code of a physical key.
pub fn flx_key_code(code: winit::keyboard::KeyCode) -> Option<i64> {
    use winit::keyboard::KeyCode as K;
    let name = format!("{code:?}");
    let n = match code {
        K::Space => "SPACE",
        K::Enter | K::NumpadEnter => "ENTER",
        K::Escape => "ESCAPE",
        K::Backspace => "BACKSPACE",
        K::Tab => "TAB",
        K::ShiftLeft | K::ShiftRight => "SHIFT",
        K::ControlLeft | K::ControlRight => "CONTROL",
        K::AltLeft | K::AltRight => "ALT",
        K::ArrowLeft => "LEFT",
        K::ArrowUp => "UP",
        K::ArrowRight => "RIGHT",
        K::ArrowDown => "DOWN",
        K::Semicolon => "SEMICOLON",
        K::Equal => "PLUS",
        K::Comma => "COMMA",
        K::Minus => "MINUS",
        K::Period => "PERIOD",
        K::Slash => "SLASH",
        K::Backquote => "GRAVEACCENT",
        K::BracketLeft => "LBRACKET",
        K::Backslash => "BACKSLASH",
        K::BracketRight => "RBRACKET",
        K::Quote => "QUOTE",
        K::Delete => "DELETE",
        K::Home => "HOME",
        K::End => "END",
        K::PageUp => "PAGEUP",
        K::PageDown => "PAGEDOWN",
        K::Insert => "INSERT",
        _ => {
            // KeyA -> A, Digit1 -> ONE, F5 -> F5.
            if let Some(l) = name.strip_prefix("Key") {
                return FLX_KEYS.iter().find(|k| k.0 == l).map(|k| k.1);
            }
            if let Some(d) = name.strip_prefix("Digit") {
                return d.parse::<i64>().ok().map(|d| 48 + d);
            }
            return FLX_KEYS.iter().find(|k| k.0 == name).map(|k| k.1);
        }
    };
    FLX_KEYS.iter().find(|k| k.0 == n).map(|k| k.1)
}

fn flx_key(v: &Value) -> Option<i64> {
    match v {
        Value::Int(i) => Some(*i),
        Value::Float(f) => Some(*f as i64),
        Value::Str(s) => FLX_KEYS.iter().find(|k| k.0.eq_ignore_ascii_case(s)).map(|k| k.1),
        _ => None,
    }
}
const VIDEO_SIGNALS: [&str; 5] = ["onVideoStarted", "onVideoPaused", "onVideoResumed", "onVideoRestarted", "onVideoEnded"];
/// The conversation (id 0; id 1 is its music), its box, speaker and the box's text.
pub(crate) const K_CONV: u32 = 16;
pub(crate) const K_DBOX: u32 = 17;
pub(crate) const K_SPEAKER: u32 = 18;
pub(crate) const K_TEXT: u32 = 19;
pub(crate) const K_SPRITE_PUB: u32 = K_SPRITE;

/// Engine classes scripts extend that are bound to an object by the id their constructor passes up.
const DIALOGUE_BASES: [&str; 3] = ["Conversation", "DialogueBox", "Speaker"];

/// Sprite ids: props are `index`, characters `CHAR | slot`.
const CHAR: u64 = 1 << 32;

/// Engine classes scripts name (`K_CLASS` ids index this).
const CLASSES: &[&str] = &[
    "FlxG", "Paths", "PlayState", "FunkinSound", "FlxTween", "FlxEase", "FlxColor", "FlxTimer", "Conductor", "CharacterType", "PlayStatePlaylist", "Std", "FlxMath", "FlxPoint", "FunkinSprite", "FunkinMemory", "HapticUtil", "VideoCutscene", "Constants", "Preferences", "ConversationState", "Countdown", "RetroCameraFade", "CutsceneType", "FlxAngle", "Assets", "MathUtil", "CharacterDataParser", "WiggleEffectType", "FullScreenScaleMode", "FlxTextBorderStyle", "Save", "SongRegistry", "ModuleHandler", "Scoring", "FlxAnimationUtil", "InputUtil", "DateUtil", "HealthIcon", "FlxBarFillDirection", "Objects",
];

/// Engine sprite classes a script can make (`new FlxSprite()`) or extend.
const SPRITE_CLASSES: [&str; 9] = ["FlxSprite", "FunkinSprite", "FlxSkewedSprite", "FlxAtlasSprite", "FlxBackdrop", "FlxTiledSprite", "FlxObject", "StageProp", "Bopper"];

/// Group classes: members are added to the stage with the group.
const GROUP_CLASSES: [&str; 5] = ["FlxTypedSpriteGroup", "FlxSpriteGroup", "FlxTypedGroup", "FlxGroup", "FlxSpriteContainer"];

pub(crate) struct Tween {
    id: u64,
    target: Value,
    /// (field, from, to); `from` is read when the tween starts.
    fields: Vec<(String, Option<f64>, f64)>,
    /// `FlxTween.color`: (from, to) ARGB.
    color: Option<(u32, u32)>,
    /// `FlxTween.num`: the callback receiving the value.
    num: Option<(f64, f64, Value)>,
    /// `FlxTween.quadPath`: the points (x, y) moved along.
    path: Option<Vec<(f64, f64)>>,
    dur: f64,
    delay: f64,
    t: f64,
    ease: String,
    on_update: Value,
    on_complete: Value,
    /// `FlxTweenType`: 1 persist/oneshot, 2 looping, 4 pingpong.
    looping: bool,
    pingpong: bool,
    active: bool,
}

/// `FunkinSound.load(...)`: a sound a script can start, stop and time.
pub(crate) struct ScriptSound {
    pcm: Option<std::sync::Arc<crate::audio::Pcm>>,
    volume: f32,
    looped: bool,
    playing: Option<(u64, std::time::Instant)>,
}

#[derive(Default)]
pub(crate) struct AnimSignals {
    /// (callback, once)
    finish: Vec<(Value, bool)>,
    frame: Vec<(Value, bool)>,
    label: Vec<(Value, bool)>,
    /// `finishCallback = f`.
    finish_callback: Value,
    /// (animation, frame, finished) at the last check.
    last: Option<(String, usize, bool)>,
}

/// `RetroCameraFade`: a stepped fade to or from black or white.
#[derive(Clone, Copy)]
pub(crate) struct CamFade {
    /// -1 black .. 1 white at the start and end.
    from: f32,
    to: f32,
    steps: f64,
    time: f64,
    t: f64,
}

impl CamFade {
    /// Signed fade: negative darkens, positive whitens.
    pub fn value(&self) -> f32 {
        let iv = self.time / self.steps.max(1.0);
        let k = if iv > 0.0 { (self.t / iv).floor() } else { self.steps + 1.0 };
        // The first timer tick repeats the start value, as in the original.
        let p = ((k - 1.0) / self.steps).clamp(0.0, 1.0) as f32;
        self.from + (self.to - self.from) * p
    }
}

/// `FlxCamera.fade(color, duration, fadeIn, onComplete)`.
#[derive(Clone)]
pub(crate) struct FlxFade {
    color: [f32; 4],
    duration: f64,
    /// Fading in from the colour (else out to it, staying covered).
    fade_in: bool,
    t: f64,
    on_complete: Value,
}

impl FlxFade {
    fn alpha(&self) -> f32 {
        let k = (self.t / self.duration.max(1e-6)).min(1.0) as f32;
        if self.fade_in { 1.0 - k } else { k }
    }
}

pub(crate) struct Timer {
    id: u64,
    /// The `FlxTimerManager` it was made with.
    mgr: Option<u64>,
    time: f64,
    t: f64,
    loops: i64,
    done: i64,
    cb: Value,
    active: bool,
}

/// `FlxTypedSpriteGroup`: members drawn at the group's zIndex.
#[derive(Default)]
pub(crate) struct Group {
    members: Vec<Value>,
    z: i32,
    added: bool,
}

/// `FlxText`, drawn on camCutscene.
pub(crate) struct ScriptText {
    x: f32,
    y: f32,
    text: String,
    size: f32,
    color: [f32; 4],
    border_color: [f32; 4],
    border: f32,
    outline: bool,
    alpha: f32,
    visible: bool,
    font: Option<Rc<crate::gfx::ttf::TtfFont>>,
    added: bool,
    /// `fieldWidth` (0 = as wide as the text) and alignment: 0 left, 1 center, 2 right.
    field_width: f32,
    align: u8,
}

/// State the host keeps between calls.
#[derive(Default)]
pub struct HostState {
    pub(crate) tweens: Vec<Tween>,
    pub(crate) timers: Vec<Timer>,
    /// `new FlxTimer(manager)`: timer id -> manager id.
    pub(crate) timer_mgr: HashMap<u64, u64>,
    pub(crate) groups: Vec<Group>,
    /// Character id -> the script class for it (bound when `fetchCharacter` makes one).
    pub(crate) char_classes: HashMap<String, String>,
    pub(crate) texts: Vec<ScriptText>,
    /// `controls.CUTSCENE_ADVANCE`: pressed since the last update.
    pub cutscene_advance: bool,
    /// The first key bound to CUTSCENE_ADVANCE (`getDialogueNameFromToken`).
    pub advance_key: String,
    /// Keys held, pressed and released since the last update (Flixel codes).
    pub keys_down: HashSet<i64>,
    pub keys_pressed: HashSet<i64>,
    pub keys_released: HashSet<i64>,
    /// `PlayState.instance.isBotPlayMode`.
    pub botplay: bool,
    /// `PlayState.instance.isPracticeMode`.
    pub practice: bool,
    /// A script set `PlayState.instance.health` (to `health`).
    pub health_set: bool,
    /// `PlayState.instance.mayPauseGame = false`.
    pub no_pause: bool,
    /// `PlayState.instance.isInCutscene`.
    pub in_cutscene: bool,
    pub(crate) next_id: u64,
    /// `super('id')` arguments seen while scanning script classes.
    pub(crate) scanned: HashMap<u64, String>,
    /// What `construct_base` binds the next `super(...)` to.
    pub(crate) binding: Option<HostRef>,
    pub(crate) stubs: Vec<String>,
    /// Script-class sprites that were `add`ed: their `update(elapsed)` runs each frame.
    pub(crate) updatables: Vec<Value>,
    pub(crate) sounds: Vec<ScriptSound>,
    /// Animation callbacks per sprite, and what they last saw.
    pub(crate) signals: HashMap<u64, AnimSignals>,
    /// The bound stage, song and character scripts, in dispatch order.
    pub(crate) bound: Vec<Value>,
    /// `Module` scripts by id (`ModuleHandler.getModule`).
    pub(crate) modules: Vec<(String, Value)>,
    /// `NoteKind` scripts by note kind, and whether they take the characters' animation (`noAnim`).
    pub(crate) note_kinds: HashMap<String, (Value, bool)>,
    /// `SongEvent` scripts by event kind.
    pub(crate) event_scripts: HashMap<String, Value>,
    /// Callbacks on `Conductor.beatHit`.
    pub(crate) beat_listeners: Vec<Value>,
    /// PlayState's HUD objects, by `HUD_*` id.
    pub hud: [HudElem; 5],
    /// `healthBar.value` (the drawn, lerped health).
    pub hud_health: f64,
    /// Video sprites whose `load` failed: `onEncounteredError` fires next update.
    pub(crate) video_errors: Vec<u64>,
    /// `FlxTween.num` tweens' current values (`twn.value`).
    pub(crate) tween_values: HashMap<u64, f64>,
    /// `FunkinVideoSprite`s by sprite id.
    pub(crate) videos: HashMap<u64, VideoSprite>,
    /// `FlxBar`s by sprite id: what they show.
    pub(crate) bars: HashMap<u64, BarBind>,
    /// Static fields scripts set on engine classes.
    pub(crate) class_statics: HashMap<(String, String), Value>,
    /// `HealthIcon.BOP_SCALE` set by a script.
    pub icon_bop_scale: Option<f32>,
    /// `playerStrumline` / `opponentStrumline`.
    pub strum: [StrumState; 2],
    /// `Countdown.skipCountdown()` was called.
    pub skip_countdown: bool,
    /// A note's chart data (`noteData`, `kind`, `getParam`), by K_NOTE id.
    pub note_info: HashMap<u64, Value>,
    /// Plain data fields scripts keep on engine sprites (`ignoreExclusionPref`, ...).
    pub(crate) sprite_extra: HashMap<(u64, String), Value>,
    /// The conversation on screen (`PlayState.startConversation`).
    pub(crate) dialogue: Option<crate::dialogue::Conversation>,
    /// (base, id) -> script class extending a dialogue class.
    pub(crate) dialogue_classes: HashMap<(String, String), String>,
    pub song_id: String,
    pub stage_id: String,
    pub step_ms: f64,
    pub beat_ms: f64,
    pub song_pos: f64,
    /// `PlayState.health` (0..2).
    pub health: f64,
    pub beat: i64,
    pub step: i64,
    pub bpm: f64,
    pub is_story: bool,
    /// `Preferences.naughtyness`.
    pub naughty: bool,
    pub difficulty: String,
    pub variation: String,
    /// `VideoCutscene.play(path)`, for PlayState to open.
    pub video_request: Option<String>,
    /// A cutscene video is showing.
    pub video_playing: bool,
    /// `CutsceneType` of the video: 0 STARTING, 1 ENDING, 2 MIDSONG.
    pub video_type: u8,
    /// `VideoCutscene.hideVideo()` (the video keeps playing, unseen).
    pub video_hidden: bool,
    /// The black screen behind a video (hidden by `hideVideo` / `showVideo`).
    pub video_black: bool,
    /// `VideoCutscene.finishVideo()`: end it now and run its cutscene's follow-up.
    pub video_finish: bool,
    /// `VideoCutscene.destroyVideo()`: end it with no follow-up.
    pub video_destroy: bool,
    /// Callbacks on the `VideoCutscene` signals: (callback, once).
    pub(crate) video_signals: [Vec<(Value, bool)>; 5],
    /// `PlayState.instance.endSong()`.
    pub end_song: bool,
    /// A script called `dispatch()` on a video signal.
    pub(crate) video_signal_fire: Option<usize>,
    /// `PlayState.instance.startCountdown()`.
    pub start_countdown: bool,
    /// `RetroCameraFade` on camGame (0) and camCutscene (1).
    pub(crate) cam_fades: [Option<CamFade>; 2],
    /// `camGame` / `camHUD` / `camCutscene` `.fade(...)`.
    pub(crate) flx_fades: [Option<FlxFade>; 3],
    /// `PlayState.instance.comboPopUps.offsets`.
    pub popup_offsets: [f32; 2],
    /// Script property setters running now (sprite, field), so `super.set_x` reaches the engine.
    pub(crate) in_setter: Vec<(u64, String)>,
    /// Shader parameter names scripts reached as fields (`shader.funnyShit`).
    pub(crate) param_names: Vec<String>,
    /// `camHUD.visible` (false hides the HUD; the countdown shows it again).
    pub hud_hidden: bool,
    /// 1 - `camHUD.alpha`.
    pub hud_fade: f32,
    /// The camera flash requested by `camGame.flash` (colour, seconds).
    pub flash: Option<([f32; 4], f32)>,
    /// Shaders scripts made, by id.
    pub(crate) shaders: Vec<Rc<RefCell<crate::shaders::ShaderInst>>>,
    /// `FlxG.camera.filters`: the game camera's shader filters, in order.
    pub cam_filters: Vec<Rc<RefCell<crate::shaders::ShaderInst>>>,
}

/// The engine as scripts see it during one call.
pub struct Ctx<'a> {
    pub gfx: Option<&'a mut Gfx>,
    /// The song's texture scope (art scripts load is freed with the song's).
    pub tex_scope: usize,
    pub world: Option<&'a mut World>,
    pub audio: &'a Audio,
    pub assets: &'a Assets,
    pub state: &'a mut HostState,
}

/// Bound script instances for one song.
pub struct Scripts {
    interp: Interp,
    pub state: HostState,
    assets: Assets,
    tex_scope: usize,
    /// Hooks that failed (logged once per script and hook).
    failed: HashSet<String>,
}

/// `CharacterType` as scripts compare it (`CharacterType.BF` is `"bf"`).
fn char_type_name(k: Option<crate::world::CharType>) -> &'static str {
    match k {
        Some(crate::world::CharType::Bf) => "bf",
        Some(crate::world::CharType::Dad) => "dad",
        Some(crate::world::CharType::Gf) => "gf",
        None => "other",
    }
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Null)
}

fn color_to_int(c: [f32; 4]) -> i64 {
    let b = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    (((b(c[3]) << 24) | (b(c[0]) << 16) | (b(c[1]) << 8) | b(c[2])) as i64) & 0xFFFF_FFFF
}

/// FlxTextAlign ("left", "center", "right", "justify") as 0 / 1 / 2.
fn text_align(s: &str) -> u8 {
    match s.to_ascii_lowercase().as_str() {
        "center" => 1,
        "right" => 2,
        _ => 0,
    }
}

fn int_to_color(v: i64) -> [f32; 4] {
    let v = v as u32;
    [((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, ((v >> 24) & 255) as f32 / 255.0]
}

/// `FlxColor.fromString`.
fn parse_color(s: &str) -> Option<i64> {
    let named = match s.to_lowercase().as_str() {
        "white" => Some(0xFFFFFFFF),
        "black" => Some(0xFF000000),
        "red" => Some(0xFFFF0000),
        "green" => Some(0xFF008000),
        "blue" => Some(0xFF0000FF),
        "yellow" => Some(0xFFFFFF00),
        "transparent" => Some(0),
        _ => None,
    };
    if named.is_some() {
        return named;
    }
    let h = s.trim().trim_start_matches('#').trim_start_matches("0x");
    let v = u32::from_str_radix(h, 16).ok()?;
    Some(if h.len() <= 6 { (0xFF00_0000 | v) as i64 } else { v as i64 })
}

/// Load every `.hxc` under `scripts/`.
fn load_all(interp: &mut Interp, assets: &Assets) {
    let mut stack = vec![assets.root().join("scripts")];
    let mut n = 0;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = funkin_core::vfs::read_dir(&dir) else { continue };
        for e in entries {
            if e.is_dir {
                stack.push(e.path);
            } else if e.name.ends_with(".hxc") {
                match funkin_core::vfs::read_to_string(&e.path) {
                    Ok(src) => match interp.load(&src, &e.path.display().to_string()) {
                        Ok(()) => n += 1,
                        Err(err) => log::warn!("script: {err}"),
                    },
                    Err(err) => log::warn!("script {}: {err}", e.path.display()),
                }
            }
        }
    }
    log::info!("loaded {n} scripts");
}

const CHAR_BASES: [&str; 7] = ["SparrowCharacter", "MultiSparrowCharacter", "AnimateAtlasCharacter", "MultiAnimateAtlasCharacter", "PackerCharacter", "BaseCharacter", "MultiAtlasCharacter"];

impl Scripts {
    /// Load the scripts and bind those for this song's stage, song and characters.
    pub fn load(gfx: &mut Gfx, assets: &Assets, audio: &Audio, world: Option<&mut World>, mut state: HostState, tex_scope: usize) -> Self {
        let defines: HashSet<String> = ["desktop", "sys", "cpp", if cfg!(windows) { "windows" } else { "linux" }].iter().map(|s| s.to_string()).collect();
        let mut interp = Interp::new(defines);
        load_all(&mut interp, assets);
        let mut world = world;
        let char_ids: Vec<Option<String>> = match &world {
            Some(w) => w.chars.iter().map(|c| c.as_ref().map(|c| c.id.clone())).collect(),
            None => vec![None, None, None],
        };
        let mut s = Self { interp, state: HostState::default(), assets: assets.clone(), tex_scope, failed: HashSet::new() };
        std::mem::swap(&mut s.state, &mut state);
        {
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            s.interp.link(&mut ctx);
        }
        // Polymod: instantiate each scripted class to learn the id its constructor passes up.
        // `SongRegistry.fetchEntry`: a variation uses its own song script if it has one.
        let song_keys = {
            let v = &s.state.variation;
            let mut k = vec![];
            if v != "default" && !v.is_empty() {
                k.push(format!("{}:{v}", s.state.song_id));
            }
            k.push(s.state.song_id.clone());
            k
        };
        let mut want: Vec<(&[&str], Vec<String>, HostRef)> =
            vec![(&["Stage"], vec![s.state.stage_id.clone()], HostRef { kind: K_STAGE, id: 0 }), (&["Song"], song_keys, HostRef { kind: K_SONG, id: 0 })];
        for (i, id) in char_ids.iter().enumerate() {
            if let Some(id) = id {
                want.push((&CHAR_BASES, vec![id.clone()], HostRef { kind: K_SPRITE, id: CHAR | i as u64 }));
            }
        }
        for (bases, keys, target) in want {
            let classes: Vec<_> = bases.iter().flat_map(|b| s.interp.classes_extending(b)).collect();
            let mut found = Vec::new();
            for cls in classes {
                let scan_id = s.state.next_id;
                s.state.next_id += 1;
                s.state.binding = Some(HostRef { kind: K_SCAN, id: scan_id });
                let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
                s.interp.quiet = true;
                let probe = s.interp.instantiate(&mut ctx, &cls, vec![]);
                s.interp.quiet = false;
                if let (Ok(_), Some(k)) = (probe, s.state.scanned.get(&scan_id).cloned()) {
                    found.push((cls, k));
                }
            }
            let Some((cls, id)) = keys.iter().find_map(|k| found.iter().find(|(_, f)| f == k).cloned()) else { continue };
            s.state.binding = Some(target);
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            match s.interp.instantiate(&mut ctx, &cls, vec![]) {
                Ok(inst) => {
                    log::info!("script {} bound to {id}", cls.decl.name);
                    s.state.bound.push(Value::Instance(inst));
                }
                Err(e) => log::warn!("script {}: {e}", cls.decl.name),
            }
        }
        // Characters a script fetches get their script class when they're made.
        let char_cls: Vec<_> = CHAR_BASES.iter().flat_map(|b| s.interp.classes_extending(b)).collect();
        for cls in char_cls {
            let scan_id = s.state.next_id;
            s.state.next_id += 1;
            s.state.binding = Some(HostRef { kind: K_SCAN, id: scan_id });
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            s.interp.quiet = true;
            let probe = s.interp.instantiate(&mut ctx, &cls, vec![]);
            s.interp.quiet = false;
            if let (Ok(_), Some(id)) = (probe, s.state.scanned.get(&scan_id).cloned()) {
                s.state.char_classes.entry(id).or_insert_with(|| cls.decl.name.to_string());
            }
        }
        // Dialogue classes are made later; learn which id each one is for now.
        for base in DIALOGUE_BASES {
            for cls in s.interp.classes_extending(base) {
                let scan_id = s.state.next_id;
                s.state.next_id += 1;
                s.state.binding = Some(HostRef { kind: K_SCAN, id: scan_id });
                let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
                s.interp.quiet = true;
                let probe = s.interp.instantiate(&mut ctx, &cls, vec![]);
                s.interp.quiet = false;
                if let (Ok(_), Some(id)) = (probe, s.state.scanned.get(&scan_id).cloned()) {
                    s.state.dialogue_classes.insert((base.to_owned(), id), cls.decl.name.to_string());
                }
            }
        }
        s.state.binding = None;
        // Modules (ModuleHandler): made once, dispatched to before the stage and song.
        let mut mods: Vec<(i64, String, Value, bool)> = Vec::new();
        for cls in s.interp.classes_extending("Module") {
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            match s.interp.instantiate(&mut ctx, &cls, vec![]) {
                Ok(inst) => {
                    let f = |n: &str| inst.fields.borrow().get(n).cloned().unwrap_or(Value::Null);
                    let id = f("moduleId").to_string();
                    // `{state: PlayState}`: only active there.
                    let state = f("state").field("state").unwrap_or(Value::Null);
                    let play = match &state {
                        Value::Null => true,
                        Value::Host(HostRef { kind: K_CLASS, id }) => CLASSES[*id as usize] == "PlayState",
                        _ => false,
                    };
                    mods.push((f("priority").i(), id, Value::Instance(inst), play));
                }
                Err(e) => log::warn!("module {}: {e}", cls.decl.name),
            }
        }
        mods.sort_by_key(|m| m.0);
        let active: Vec<Value> = mods.iter().filter(|m| m.3).map(|m| m.2.clone()).collect();
        log::info!("{} modules ({} in play)", mods.len(), active.len());
        s.state.bound.splice(0..0, active);
        s.state.modules = mods.into_iter().map(|m| (m.1, m.2)).collect();
        for cls in s.interp.classes_extending("NoteKind") {
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            match s.interp.instantiate(&mut ctx, &cls, vec![]) {
                Ok(inst) => {
                    let (id, no_anim) = {
                        let f = inst.fields.borrow();
                        (f.get("noteKind").map(|v| v.to_string()).unwrap_or_default(), f.get("noAnim").is_some_and(|v| v.truthy()))
                    };
                    s.state.note_kinds.insert(id, (Value::Instance(inst), no_anim));
                }
                Err(e) => log::warn!("note kind {}: {e}", cls.decl.name),
            }
        }
        for cls in s.interp.classes_extending("SongEvent") {
            let mut ctx = Ctx { gfx: Some(&mut *gfx), tex_scope, world: world.as_deref_mut(), audio, assets, state: &mut s.state };
            match s.interp.instantiate(&mut ctx, &cls, vec![]) {
                Ok(inst) => {
                    let id = inst.fields.borrow().get("id").map(|v| v.to_string()).unwrap_or_default();
                    s.state.event_scripts.insert(id, Value::Instance(inst));
                }
                Err(e) => log::warn!("song event {}: {e}", cls.decl.name),
            }
        }
        s
    }

    /// A note kind whose script animates the characters itself (`noAnim`).
    pub fn note_kind_no_anim(&self, kind: &str) -> bool {
        self.state.note_kinds.get(kind).is_some_and(|(_, n)| *n)
    }

    /// A chart event a `SongEvent` script handles (`handleEvent(data)`). Returns whether one did.
    pub fn song_event(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio, kind: &str, data: Value) -> bool {
        let Some(ev) = self.state.event_scripts.get(kind).cloned() else { return false };
        let (interp, mut ctx, failed) = self.ctx(gfx, world, audio);
        if let Err(e) = interp.call_method(&mut ctx, &ev, "handleEvent", vec![data]) {
            if failed.insert(format!("event:{kind}:{e}")) {
                log::warn!("song event {kind}: {e}");
            }
        }
        true
    }

    /// `ScriptedFunkinSprite.scriptInit(class, x, y)`: make a script class that
    /// extends a sprite. Returns the instance and its sprite (a prop of `world`).
    pub fn new_script_sprite(&mut self, gfx: &mut Gfx, world: &mut World, audio: &Audio, class: &str, x: f64, y: f64) -> Option<(Value, usize)> {
        let cls = self.interp.class(class)?;
        let (interp, mut ctx, _) = self.ctx(gfx, Some(world), audio);
        match interp.instantiate(&mut ctx, &cls, vec![Value::Float(x), Value::Float(y)]) {
            Ok(inst) => match inst.base_ref() {
                Some(HostRef { kind: K_SPRITE, id }) if id & CHAR == 0 => Some((Value::Instance(inst), id as usize)),
                _ => {
                    log::warn!("script {class} is not a sprite");
                    None
                }
            },
            Err(e) => {
                log::warn!("script {class}: {e}");
                None
            }
        }
    }

    pub fn in_dialogue(&self) -> bool {
        self.state.dialogue.is_some()
    }

    /// The accept key during a conversation.
    pub fn advance_dialogue(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio) {
        let (interp, mut ctx, _) = self.ctx(gfx, world, audio);
        crate::dialogue::advance(interp, &mut ctx);
    }

    /// The `RetroCameraFade` overlay for camGame (0) or camCutscene (1).
    pub fn draw_fade(&self, gfx: &mut Gfx, cam: usize) {
        if let Some(f) = &self.state.cam_fades[cam] {
            let v = f.value();
            if v.abs() > 0.001 {
                let c = if v < 0.0 { 0.0 } else { 1.0 };
                gfx.set_transform(crate::gfx::Affine::IDENTITY);
                gfx.rect(0.0, 0.0, crate::gfx::VIEW_W, crate::gfx::VIEW_H, [c, c, c, v.abs()]);
            }
        }
    }

    /// A camera's `fade` (0 camGame, 1 camHUD, 2 camCutscene).
    pub fn draw_flx_fade(&self, gfx: &mut Gfx, cam: usize) {
        if let Some(f) = &self.state.flx_fades[cam] {
            let a = f.alpha() * f.color[3];
            if a > 0.001 {
                gfx.set_transform(crate::gfx::Affine::IDENTITY);
                gfx.rect(0.0, 0.0, crate::gfx::VIEW_W, crate::gfx::VIEW_H, [f.color[0], f.color[1], f.color[2], a]);
            }
        }
    }

    /// Script `FlxText`s (camCutscene).
    pub fn draw_texts(&self, gfx: &mut Gfx) {
        for t in &self.state.texts {
            let Some(font) = &t.font else { continue };
            if !t.added || !t.visible || t.alpha <= 0.001 || t.text.is_empty() {
                continue;
            }
            gfx.set_transform(crate::gfx::Affine::IDENTITY);
            let lh = font.line_height(t.size);
            // FlxText keeps a 2px gutter around the text.
            let (x0, y0) = (t.x + 2.0, t.y + 2.0);
            // Lines sit in the field (fieldWidth, or the widest line) by alignment.
            let widths: Vec<f32> = t.text.lines().map(|l| font.width(gfx, l, t.size)).collect();
            let field = if t.field_width > 0.0 { t.field_width - 4.0 } else { widths.iter().copied().fold(0.0, f32::max) };
            for pass in 0..2 {
                for (i, line) in t.text.lines().enumerate() {
                    let y = y0 + i as f32 * lh;
                    let x0 = x0 + match t.align {
                        1 => ((field - widths[i]) / 2.0).floor(),
                        2 => field - widths[i],
                        _ => 0.0,
                    };
                    if pass == 0 {
                        if !t.outline || t.border <= 0.0 {
                            continue;
                        }
                        let b = t.border;
                        let c = [t.border_color[0], t.border_color[1], t.border_color[2], t.border_color[3] * t.alpha];
                        for (dx, dy) in [(-b, -b), (0.0, -b), (b, -b), (-b, 0.0), (b, 0.0), (-b, b), (0.0, b), (b, b)] {
                            font.draw(gfx, line, x0 + dx, y + dy, t.size, c);
                        }
                    } else {
                        font.draw(gfx, line, x0, y, t.size, [t.color[0], t.color[1], t.color[2], t.color[3] * t.alpha]);
                    }
                }
            }
        }
    }

    pub fn draw_dialogue(&self, gfx: &mut Gfx) {
        if let Some(c) = &self.state.dialogue {
            c.draw(gfx);
        }
    }

    fn ctx<'a>(&'a mut self, gfx: &'a mut Gfx, world: Option<&'a mut World>, audio: &'a Audio) -> (&'a mut Interp, Ctx<'a>, &'a mut HashSet<String>) {
        let Self { interp, state, assets, failed, tex_scope } = self;
        (interp, Ctx { gfx: Some(gfx), tex_scope: *tex_scope, world, audio, assets, state }, failed)
    }

    /// Call `name(event)` on every bound script that defines it.
    pub fn dispatch(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio, name: &str, event: Value) {
        let (interp, mut ctx, failed) = self.ctx(gfx, world, audio);
        if name == "onBeatHit" {
            let beat = Value::Int(ctx.state.beat);
            for cb in ctx.state.beat_listeners.clone() {
                if let Err(e) = interp.call(&mut ctx, &cb, vec![beat.clone()]) {
                    if failed.insert(format!("beatHit:{e}")) {
                        log::warn!("script Conductor.beatHit: {e}");
                    }
                }
            }
        }
        let bound = ctx.state.bound.clone();
        for b in &bound {
            let Value::Instance(inst) = b else { continue };
            if !inst.class.has_method(name) {
                continue;
            }
            if let Err(e) = interp.call_method(&mut ctx, b, name, vec![event.clone()]) {
                let key = format!("{}.{name}", inst.class.decl.name);
                if failed.insert(key.clone()) {
                    log::warn!("script {key}: {e}");
                }
            }
        }
    }

    /// Dispatch a note event (`onNoteHit`, `onNoteMiss`, `onNoteGhostMiss`):
    /// other scripts first, then the characters' (dad, bf, gf), as the stage
    /// does. Returns which character slots have a script that took it; the
    /// rest get the engine's animation from the caller.
    pub fn dispatch_note(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio, name: &str, event: Value) -> [bool; 3] {
        let (interp, mut ctx, failed) = self.ctx(gfx, world, audio);
        // The note's own kind script hears about it first (`NoteKindManager`).
        let kind = event.field("kind").map(|k| k.to_string()).unwrap_or_default();
        if let Some((nk, _)) = ctx.state.note_kinds.get(&kind).cloned() {
            if matches!(&nk, Value::Instance(i) if i.class.has_method(name)) {
                if let Err(e) = interp.call_method(&mut ctx, &nk, name, vec![event.clone()]) {
                    if failed.insert(format!("notekind:{kind}.{name}")) {
                        log::warn!("note kind {kind}.{name}: {e}");
                    }
                }
            }
        }
        let bound = ctx.state.bound.clone();
        let slot_of = |b: &Value| match b {
            Value::Instance(i) => i.base_ref().filter(|h| h.kind == K_SPRITE && h.id & CHAR != 0).map(|h| (h.id & 0xFF) as usize),
            _ => None,
        };
        let mut order: Vec<&Value> = bound.iter().filter(|b| slot_of(b).is_none()).collect();
        for s in [1, 0, 2] {
            order.extend(bound.iter().filter(|b| slot_of(b) == Some(s)));
        }
        let mut handled = [false; 3];
        for b in order {
            let Value::Instance(inst) = b else { continue };
            if !inst.class.has_method(name) {
                continue;
            }
            if let Some(s) = slot_of(b) {
                handled[s] = true;
            }
            if let Err(e) = interp.call_method(&mut ctx, b, name, vec![event.clone()]) {
                let key = format!("{}.{name}", inst.class.decl.name);
                if failed.insert(key.clone()) {
                    log::warn!("script {key}: {e}");
                }
            }
        }
        handled
    }

    /// What the level's scripts (its stage, song and characters, and the
    /// modules in play) take over of the HUD, read from their source before
    /// any of their events run. See [`crate::hud_scan`].
    pub fn hud_takeover(&self) -> crate::hud_scan::HudTakeover {
        let mut out = crate::hud_scan::HudTakeover::default();
        let mut seen: HashSet<Rc<str>> = HashSet::new();
        for b in &self.state.bound {
            let Value::Instance(inst) = b else { continue };
            let module = inst.class.host_base.borrow().as_deref() == Some("Module");
            // The class and its scripted ancestors.
            let mut cls = Some(inst.class.clone());
            while let Some(c) = cls {
                let file = c.decl.file.clone();
                if seen.insert(file.clone()) {
                    if let Ok(src) = funkin_core::vfs::read_to_string(std::path::Path::new(&*file)) {
                        let t = crate::hud_scan::scan(&src, module, &self.state.song_id);
                        if t.any() {
                            log::info!("{} takes over the HUD: {t:?}", c.decl.name);
                        }
                        out.health_bar |= t.health_bar;
                        out.strumlines |= t.strumlines;
                    }
                }
                cls = c.parent.borrow().clone();
            }
        }
        out
    }

    /// The stage's `buildStage` (the props are already built natively; this runs the script's additions).
    pub fn create(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio) {
        let mut world = world;
        let json_props: Vec<(usize, String)> =
            world.as_deref().map(|w| w.props.iter().enumerate().filter(|(_, p)| !p.name.is_empty()).map(|(i, p)| (i, p.name.clone())).collect()).unwrap_or_default();
        self.dispatch(gfx, world.as_deref_mut(), audio, "buildStage", Value::Null);
        {
            // Stage.buildStage adds each prop through `addProp`, which stage scripts override.
            let (interp, mut ctx, failed) = self.ctx(gfx, world.as_deref_mut(), audio);
            let bound = ctx.state.bound.clone();
            for b in &bound {
                let Value::Instance(inst) = b else { continue };
                if !inst.class.has_method("addProp") || inst.base_ref().map(|r| r.kind) != Some(K_STAGE) {
                    continue;
                }
                for (i, name) in &json_props {
                    if let Err(e) = interp.call_method(&mut ctx, b, "addProp", vec![Ctx::sprite_ref(*i as u64), Value::str(name)]) {
                        let key = format!("{}.addProp", inst.class.decl.name);
                        if failed.insert(key.clone()) {
                            log::warn!("script {key}: {e}");
                        }
                    }
                }
            }
        }
        self.dispatch(gfx, world.as_deref_mut(), audio, "onCreate", event(vec![]));
        self.dispatch(gfx, world.as_deref_mut(), audio, "onSongLoaded", event(vec![]));
        // PlayState.initCharacters: `currentStage.addCharacter(character, type)` for gf, bf, dad.
        let (interp, mut ctx, failed) = self.ctx(gfx, world, audio);
        let bound = ctx.state.bound.clone();
        for (slot, ty) in [(2u64, "gf"), (0, "bf"), (1, "dad")] {
            if !ctx.world.as_deref().is_some_and(|w| w.chars[slot as usize].is_some()) {
                continue;
            }
            for b in &bound {
                let Value::Instance(inst) = b else { continue };
                if !inst.class.has_method("addCharacter") {
                    continue;
                }
                if let Err(e) = interp.call_method(&mut ctx, b, "addCharacter", vec![Ctx::sprite_ref(CHAR | slot), Value::str(ty)]) {
                    let key = format!("{}.addCharacter", inst.class.decl.name);
                    if failed.insert(key.clone()) {
                        log::warn!("script {key}: {e}");
                    }
                }
            }
            // Stage.addCharacter ends with `ScriptEvent(ADDED)` to the character itself.
            if let Some(cs) = ctx.char_script(CHAR | slot) {
                if matches!(&cs, Value::Instance(i) if i.class.has_method("onAdd")) {
                    if let Err(e) = interp.call_method(&mut ctx, &cs, "onAdd", vec![event(vec![])]) {
                        if failed.insert(format!("onAdd:{e}")) {
                            log::warn!("script onAdd: {e}");
                        }
                    }
                }
            }
        }
    }

    /// Character scripts that override `playAnimation` hear about the
    /// animations the engine played on their character (dances, sings).
    fn sync_char_anims(interp: &mut Interp, ctx: &mut Ctx, failed: &mut HashSet<String>) {
        // Fetched characters (props) with a script that overrides playAnimation.
        let fetched: Vec<(u64, Value)> = ctx
            .state
            .bound
            .iter()
            .filter_map(|b| match b {
                Value::Instance(i) if i.class.has_method("playAnimation") => match i.base_ref() {
                    Some(HostRef { kind: K_SPRITE, id }) if id & CHAR == 0 => Some((id, b.clone())),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        for (id, inst) in fetched {
            let Some(a) = ctx.actor(id) else { continue };
            let log = a.anim_log.replace(Vec::new()).unwrap_or_default();
            for (name, restart, ignore) in log {
                if let Err(e) = interp.call_method(ctx, &inst, "playAnimation", vec![Value::from(name), Value::Bool(restart), Value::Bool(ignore), Value::Bool(false)]) {
                    if failed.insert(format!("playAnimation:{e}")) {
                        log::warn!("script playAnimation: {e}");
                    }
                }
            }
            if let Some(a) = ctx.actor(id) {
                a.anim_log = Some(Vec::new());
            }
        }
        for slot in 0..3u64 {
            let id = CHAR | slot;
            let Some(inst) = ctx.char_script(id) else { continue };
            let Value::Instance(i) = &inst else { continue };
            if !i.class.has_method("playAnimation") {
                continue;
            }
            let Some(c) = ctx.world.as_deref_mut().and_then(|w| w.chars[slot as usize].as_mut()) else { continue };
            let log = c.actor.anim_log.replace(Vec::new()).unwrap_or_default();
            for (name, restart, ignore) in log {
                if let Err(e) = interp.call_method(ctx, &inst, "playAnimation", vec![Value::from(name), Value::Bool(restart), Value::Bool(ignore), Value::Bool(false)]) {
                    if failed.insert(format!("playAnimation:{e}")) {
                        log::warn!("script {}.playAnimation: {e}", i.class.decl.name);
                    }
                }
            }
            // What the script itself played (through `super.playAnimation`) isn't sent back.
            if let Some(c) = ctx.world.as_deref_mut().and_then(|w| w.chars[slot as usize].as_mut()) {
                c.actor.anim_log = Some(Vec::new());
            }
        }
    }

    /// Per frame: tweens, timers, then `onUpdate`.
    /// Play `FunkinVideoSprite`s: show each frame on the sprite, and fire
    /// `onFormatSetup` (first picture), `onEndReached` and `onEncounteredError`.
    fn update_videos(interp: &mut Interp, ctx: &mut Ctx, dt: f64) {
        let mut fire: Vec<Signal> = vec![];
        for id in std::mem::take(&mut ctx.state.video_errors) {
            if let Some(v) = ctx.state.videos.get(&id) {
                fire.push(v.signals[2].clone());
            }
        }
        let ids: Vec<u64> = ctx.state.videos.keys().copied().collect();
        for id in ids {
            let audio = ctx.audio;
            let Some(gfx) = ctx.gfx.as_deref_mut() else { break };
            let Some(v) = ctx.state.videos.get_mut(&id) else { continue };
            if !v.playing || v.paused {
                continue;
            }
            let Some(vid) = &mut v.video else { continue };
            let ended = vid.update(dt as f32, gfx, audio);
            let tex = vid.texture();
            let mut first = false;
            if let Some(t) = tex {
                if !v.formatted {
                    v.formatted = true;
                    first = true;
                    fire.push(v.signals[3].clone());
                    fire.push(v.signals[1].clone());
                }
                let (w, h) = gfx.texture_size(t);
                if first {
                    if let Some(a) = ctx.world.as_deref_mut().and_then(|w| w.props.get_mut(id as usize)) {
                        a.take_graphics(Actor::from_texture("", t, w, h));
                    }
                }
            }
            if ended {
                vid.stop(audio);
                v.playing = false;
                fire.push(v.signals[0].clone());
            }
        }
        for s in fire {
            s.fire(interp, ctx);
        }
    }

    /// `FlxBar`s with a parent read `parent.variable` each frame, like `updateValueFromParent`.
    fn poll_bars(interp: &mut Interp, ctx: &mut Ctx) {
        let bound: Vec<(u64, Value, String)> =
            ctx.state.bars.iter().filter(|(_, b)| !b.parent.is_null() && !b.variable.is_empty()).map(|(&id, b)| (id, b.parent.clone(), b.variable.clone())).collect();
        for (id, parent, var) in bound {
            // A dotted path ("stats.hp") walks fields.
            let mut v = parent;
            for part in var.split('.') {
                v = match interp.get(ctx, &v, part) {
                    Ok(x) => x,
                    Err(_) => Value::Null,
                };
            }
            if v.is_null() {
                continue;
            }
            if let Some(b) = ctx.state.bars.get_mut(&id) {
                b.value = v.f().clamp(b.min.min(b.max), b.max.max(b.min));
            }
            ctx.sync_bar(id);
        }
    }

    pub fn update(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio, dt: f64) {
        let mut world = world;
        {
            let (interp, mut ctx, failed) = self.ctx(gfx, world.as_deref_mut(), audio);
            Self::sync_char_anims(interp, &mut ctx, failed);
            run_tweens(interp, &mut ctx, failed, dt);
            run_timers(interp, &mut ctx, failed, dt);
            run_signals(interp, &mut ctx, failed);
            Self::poll_bars(interp, &mut ctx);
            Self::update_videos(interp, &mut ctx, dt);
            for f in ctx.state.cam_fades.iter_mut().flatten() {
                f.t += dt;
            }
            for i in 0..3 {
                let Some(f) = &mut ctx.state.flx_fades[i] else { continue };
                let was = f.t < f.duration;
                f.t += dt;
                if was && f.t >= f.duration {
                    let cb = std::mem::take(&mut f.on_complete);
                    if f.fade_in {
                        ctx.state.flx_fades[i] = None;
                    }
                    if !cb.is_null() {
                        if let Err(e) = interp.call(&mut ctx, &cb, vec![]) {
                            if failed.insert(format!("fade:{e}")) {
                                log::warn!("script camera fade callback: {e}");
                            }
                        }
                    }
                }
            }
            if ctx.state.dialogue.is_some() {
                crate::dialogue::update(interp, &mut ctx, dt);
                if ctx.state.dialogue.as_ref().is_some_and(|c| c.finished) {
                    ctx.state.dialogue = None;
                }
            }
            // Character scripts' `onAnimationFinished(name)` (FlxAnimationController.finishCallback).
            for slot in 0..3u64 {
                let id = CHAR | slot;
                let Some(inst) = ctx.char_script(id) else { continue };
                if !matches!(&inst, Value::Instance(i) if i.class.has_method("onAnimationFinished")) {
                    continue;
                }
                let Some(name) = ctx.world.as_deref_mut().and_then(|w| w.chars[slot as usize].as_mut()).and_then(|c| c.actor.take_finished()) else { continue };
                if let Err(e) = interp.call_method(&mut ctx, &inst, "onAnimationFinished", vec![Value::from(name)]) {
                    if failed.insert(format!("animfinished:{e}")) {
                        log::warn!("script onAnimationFinished: {e}");
                    }
                }
            }
            // Character scripts' `update(elapsed)` (BaseCharacter.update).
            let chars: Vec<Value> = ctx
                .state
                .bound
                .iter()
                .filter(|b| matches!(b, Value::Instance(i) if i.class.has_method("update") && i.base_ref().is_some_and(|r| r.kind == K_SPRITE)))
                .cloned()
                .collect();
            for c in &chars {
                if let Err(e) = interp.call_method(&mut ctx, c, "update", vec![Value::Float(dt)]) {
                    if failed.insert(format!("charupdate:{e}")) {
                        log::warn!("script character update: {e}");
                    }
                }
            }
            // Script-class sprites on the stage (FlxSprite.update).
            let ups = ctx.state.updatables.clone();
            for u in &ups {
                if let Err(e) = interp.call_method(&mut ctx, u, "update", vec![Value::Float(dt)]) {
                    if failed.insert(format!("update:{e}")) {
                        log::warn!("script sprite update: {e}");
                    }
                }
            }
        }
        self.dispatch(gfx, world, audio, "onUpdate", event(vec![("elapsed", Value::Float(dt))]));
    }

    /// Dispatch a `VideoCutscene` signal (index into [`VIDEO_SIGNALS`]).
    pub fn video_signal(&mut self, gfx: &mut Gfx, world: Option<&mut World>, audio: &Audio, which: usize) {
        let (interp, mut ctx, failed) = self.ctx(gfx, world, audio);
        let list = std::mem::take(&mut ctx.state.video_signals[which]);
        let calls: Vec<Value> = list.iter().map(|(c, _)| c.clone()).collect();
        // Callbacks added while dispatching stay for next time.
        let mut keep: Vec<(Value, bool)> = list.into_iter().filter(|(_, once)| !once).collect();
        keep.append(&mut ctx.state.video_signals[which]);
        ctx.state.video_signals[which] = keep;
        for cb in calls {
            if let Err(e) = interp.call(&mut ctx, &cb, vec![]) {
                if failed.insert(format!("video:{e}")) {
                    log::warn!("script {}: {e}", VIDEO_SIGNALS[which]);
                }
            }
        }
    }

    pub fn camera_flash(&mut self) -> Option<([f32; 4], f32)> {
        self.state.flash.take()
    }

    #[allow(dead_code)]
    pub fn assets(&self) -> &Assets {
        &self.assets
    }
}

/// A `ScriptEvent`: the fields plus `cancel()` / `cancelEvent()`.
/// A `HitNoteScriptEvent` / `NoteScriptEvent` / `GhostMissNoteScriptEvent`.
/// `dir`, `mustHit` and `kind` are also kept flat for the engine's own handler.
/// [`note_event`] with the note's chart params (`note.getParam(name)`).
pub fn note_event_params(kind: &str, dir: usize, must_hit: bool, judgement: Option<&str>, params: &[funkin_core::song::NoteParamData]) -> Value {
    let kind_v = if kind.is_empty() { Value::Null } else { Value::str(kind) };
    let pv: Vec<(String, Value)> = params.iter().map(|p| (p.name.clone(), json_to_value(&p.value))).collect();
    let pv = Rc::new(pv);
    let p2 = pv.clone();
    let get_param = Value::native(move |_, _, _, a| {
        let n = arg(&a, 0).to_string();
        Ok(p2.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone()).unwrap_or(Value::Null))
    });
    let param_list = Value::array(pv.iter().map(|(n, v)| Value::object(vec![("name", Value::str(n)), ("value", v.clone())])).collect());
    let data = Value::object(vec![
        ("params", param_list),
        ("kind", kind_v.clone()),
        ("data", Value::Int(dir as i64 + if must_hit { 0 } else { 4 })),
        ("getDirection", Value::native(move |_, _, _, _| Ok(Value::Int(dir as i64)))),
        ("getMustHitNote", Value::native(move |_, _, _, _| Ok(Value::Bool(must_hit)))),
    ]);
    let note = Value::object(vec![("kind", kind_v), ("noteData", data), ("direction", Value::Int(dir as i64)), ("getParam", get_param)]);
    event(vec![
        ("note", note),
        ("judgement", judgement.map(Value::str).unwrap_or(Value::Null)),
        ("healthChange", Value::Float(0.0)),
        ("dir", Value::Int(dir as i64)),
        ("mustHit", Value::Bool(must_hit)),
        ("kind", Value::str(kind)),
        ("hasPossibleNotes", Value::Bool(false)),
    ])
}

pub fn event(fields: Vec<(&str, Value)>) -> Value {
    let mut f = fields;
    f.push(("eventCanceled", Value::Bool(false)));
    f.push((
        "cancel",
        Value::native(|_, _, this, _| {
            if let Some(t) = this {
                t.set_field("eventCanceled", Value::Bool(true));
            }
            Ok(Value::Null)
        }),
    ));
    f.push((
        "cancelEvent",
        Value::native(|_, _, this, _| {
            if let Some(t) = this {
                t.set_field("eventCanceled", Value::Bool(true));
            }
            Ok(Value::Null)
        }),
    ));
    Value::object(f)
}

/// `SongEventData` with its typed getters.
pub fn song_event_data(kind: &str, time: f64, value: &serde_json::Value) -> Value {
    let v = json_to_value(value);
    let get = move |conv: fn(&Value) -> Value| {
        Value::native(move |_, _, this, a| {
            let key = arg(&a, 0).to_string();
            let val = this.and_then(|t| t.field("value")).unwrap_or(Value::Null);
            let x = match &val {
                Value::Object(_) => val.field(&key).unwrap_or(Value::Null),
                // A bare value is the event's default field.
                other => other.clone(),
            };
            Ok(if x.is_null() { Value::Null } else { conv(&x) })
        })
    };
    Value::object(vec![
        ("eventKind", Value::str(kind)),
        ("time", Value::Float(time)),
        ("value", v),
        ("getFloat", get(|x| Value::Float(x.f()))),
        ("getInt", get(|x| Value::Int(x.i()))),
        ("getBool", get(|x| Value::Bool(x.truthy()))),
        ("getString", get(|x| Value::from(x.to_string()))),
        ("getArray", get(|x| x.clone())),
        ("getBoolArray", get(|x| x.clone())),
        ("getDynamic", get(|x| x.clone())),
    ])
}

fn value_to_json(v: &Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        Value::Null => J::Null,
        Value::Bool(b) => J::Bool(*b),
        Value::Int(i) => J::from(*i),
        Value::Float(f) => serde_json::Number::from_f64(*f).map(J::Number).unwrap_or(J::Null),
        Value::Str(s) => J::String(s.to_string()),
        Value::Array(a) => J::Array(a.borrow().iter().map(value_to_json).collect()),
        Value::Object(o) => J::Object(o.borrow().iter().map(|(k, v)| (k.to_string(), value_to_json(v))).collect()),
        Value::Map(m) => {
            let m = m.borrow();
            J::Object(m.keys.iter().zip(&m.values).map(|(k, v)| (k.to_string(), value_to_json(v))).collect())
        }
        _ => J::Null,
    }
}

/// The same function or object (signal `remove`).
fn same_value(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Func(x), Value::Func(y)) => Rc::ptr_eq(x, y) || (Rc::ptr_eq(&x.def, &y.def) && match (&x.this, &y.this) {
            (Some(p), Some(q)) => Rc::ptr_eq(p, q),
            (None, None) => true,
            _ => false,
        }),
        (Value::Native(x), Value::Native(y)) => Rc::ptr_eq(x, y),
        _ => false,
    }
}

thread_local! {
    /// `Save.instance` (its `modOptions` live in `mod_options.json`).
    static SAVE: Value = {
        let opts = Value::Map(Rc::new(RefCell::new(funkin_script::value::MapData::default())));
        let p = crate::settings::data_dir().join("mod_options.json");
        if let Ok(j) = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str::<serde_json::Value>(&t).map_err(|e| e.to_string())) {
            if let (Value::Map(m), serde_json::Value::Object(o)) = (&opts, &j) {
                for (k, v) in o {
                    m.borrow_mut().set(Value::str(k), json_to_value(v));
                }
            }
        }
        let o2 = opts.clone();
        let flush = Value::native(move |_, _, _, _| {
            let dir = crate::settings::data_dir();
            let r = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(dir.join("mod_options.json"), serde_json::to_string_pretty(&value_to_json(&o2)).unwrap_or_default()));
            if let Err(e) = r {
                log::warn!("mod_options.json: {e}");
            }
            Ok(Value::Null)
        });
        Value::object(vec![("modOptions", opts), ("flush", flush)])
    };
}

fn save_instance() -> Value {
    SAVE.with(|s| s.clone())
}

/// `new Conductor()`: a conductor a script drives itself (`update(time)`),
/// with `onBeatHit` / `onStepHit` signals.
fn script_conductor() -> Value {
    type Cbs = Rc<RefCell<Vec<Value>>>;
    let signal = |cbs: &Cbs| {
        let (c1, c2) = (cbs.clone(), cbs.clone());
        Value::object(vec![
            (
                "add",
                Value::native(move |_, _, _, a| {
                    c1.borrow_mut().push(arg(&a, 0));
                    Ok(Value::Null)
                }),
            ),
            (
                "remove",
                Value::native(move |_, _, _, a| {
                    let f = arg(&a, 0);
                    c2.borrow_mut().retain(|x| !same_value(x, &f));
                    Ok(Value::Null)
                }),
            ),
        ])
    };
    let beats: Cbs = Rc::new(RefCell::new(vec![]));
    let steps: Cbs = Rc::new(RefCell::new(vec![]));
    let (b2, s2) = (beats.clone(), steps.clone());
    let set_bpm = |this: &Value, bpm: f64| {
        this.set_field("bpm", Value::Float(bpm));
        this.set_field("beatLengthMs", Value::Float(60000.0 / bpm));
        this.set_field("stepLengthMs", Value::Float(15000.0 / bpm));
    };
    Value::object(vec![
        ("bpm", Value::Float(100.0)),
        ("beatLengthMs", Value::Float(600.0)),
        ("stepLengthMs", Value::Float(150.0)),
        ("songPosition", Value::Float(0.0)),
        ("currentBeat", Value::Int(-1)),
        ("currentStep", Value::Int(-1)),
        ("onBeatHit", signal(&beats)),
        ("onStepHit", signal(&steps)),
        (
            "mapTimeChanges",
            Value::native(move |_, _, this, a| {
                let first = match arg(&a, 0) {
                    Value::Array(v) => v.borrow().first().cloned(),
                    _ => None,
                };
                if let (Some(this), Some(bpm)) = (this, first.and_then(|t| t.field("bpm")).map(|b| b.f()).filter(|b| *b > 0.0)) {
                    set_bpm(this, bpm);
                }
                Ok(Value::Null)
            }),
        ),
        (
            "update",
            Value::native(move |interp, host, this, a| {
                let Some(this) = this else { return Ok(Value::Null) };
                let pos = arg(&a, 0).f();
                let beat_ms = this.field("beatLengthMs").map(|v| v.f()).unwrap_or(600.0);
                this.set_field("songPosition", Value::Float(pos));
                let step = (pos / (beat_ms / 4.0)).floor() as i64;
                let beat = step.div_euclid(4);
                let mut cur_step = this.field("currentStep").map(|v| v.i()).unwrap_or(-1);
                let mut cur_beat = this.field("currentBeat").map(|v| v.i()).unwrap_or(-1);
                this.set_field("currentStep", Value::Int(step));
                this.set_field("currentBeat", Value::Int(beat));
                while cur_step < step {
                    cur_step += 1;
                    for cb in s2.borrow().clone() {
                        interp.call(host, &cb, vec![Value::Int(cur_step)])?;
                    }
                }
                while cur_beat < beat {
                    cur_beat += 1;
                    for cb in b2.borrow().clone() {
                        interp.call(host, &cb, vec![Value::Int(cur_beat)])?;
                    }
                }
                Ok(Value::Null)
            }),
        ),
    ])
}

/// A `FlxPoint`: `x`, `y` and its common methods.
fn flx_point(x: f64, y: f64) -> Value {
    fn xy(this: Option<&Value>) -> (f64, f64) {
        let f = |n| this.and_then(|t| t.field(n)).map(|v| v.f()).unwrap_or(0.0);
        (f("x"), f("y"))
    }
    fn put(this: Option<&Value>, x: f64, y: f64) -> Value {
        if let Some(t) = this {
            t.set_field("x", Value::Float(x));
            t.set_field("y", Value::Float(y));
            return t.clone();
        }
        Value::Null
    }
    Value::object(vec![
        ("x", Value::Float(x)),
        ("y", Value::Float(y)),
        ("set", Value::native(|_, _, t, a| Ok(put(t, arg(&a, 0).f(), if a.len() > 1 { arg(&a, 1).f() } else { arg(&a, 0).f() })))),
        (
            "add",
            Value::native(|_, _, t, a| {
                let (x, y) = xy(t);
                Ok(put(t, x + arg(&a, 0).f(), y + arg(&a, 1).f()))
            }),
        ),
        (
            "subtract",
            Value::native(|_, _, t, a| {
                let (x, y) = xy(t);
                Ok(put(t, x - arg(&a, 0).f(), y - arg(&a, 1).f()))
            }),
        ),
        (
            "scale",
            Value::native(|_, _, t, a| {
                let (x, y) = xy(t);
                let sy = if a.len() > 1 { arg(&a, 1).f() } else { arg(&a, 0).f() };
                Ok(put(t, x * arg(&a, 0).f(), y * sy))
            }),
        ),
        (
            "copyFrom",
            Value::native(|_, _, t, a| {
                let o = arg(&a, 0);
                Ok(put(t, o.field("x").map(|v| v.f()).unwrap_or(0.0), o.field("y").map(|v| v.f()).unwrap_or(0.0)))
            }),
        ),
        (
            "clone",
            Value::native(|_, _, t, _| {
                let (x, y) = xy(t);
                Ok(flx_point(x, y))
            }),
        ),
        (
            "copyTo",
            Value::native(|_, _, t, a| {
                let (x, y) = xy(t);
                let o = arg(&a, 0);
                o.set_field("x", Value::Float(x));
                o.set_field("y", Value::Float(y));
                Ok(o)
            }),
        ),
        ("put", Value::native(|_, _, _, _| Ok(Value::Null))),
        ("destroy", Value::native(|_, _, _, _| Ok(Value::Null))),
    ])
}

pub fn json_to_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Bool(*b),
        serde_json::Value::Number(n) => match n.as_i64() {
            Some(i) => Value::Int(i),
            None => Value::Float(n.as_f64().unwrap_or(0.0)),
        },
        serde_json::Value::String(s) => Value::str(s),
        serde_json::Value::Array(a) => Value::array(a.iter().map(json_to_value).collect()),
        serde_json::Value::Object(o) => Value::object(o.iter().map(|(k, v)| (k.as_str(), json_to_value(v))).collect()),
    }
}

fn run_tweens(interp: &mut Interp, ctx: &mut Ctx, failed: &mut HashSet<String>, dt: f64) {
    let mut i = 0;
    while i < ctx.state.tweens.len() {
        let tw = &mut ctx.state.tweens[i];
        if !tw.active {
            ctx.state.tweens.remove(i);
            continue;
        }
        tw.t += dt;
        let id = tw.id;
        let local = tw.t - tw.delay;
        if local < 0.0 {
            i += 1;
            continue;
        }
        let mut k = if tw.dur > 0.0 { (local / tw.dur).min(1.0) } else { 1.0 };
        let finished = k >= 1.0;
        if tw.pingpong && ((local / tw.dur.max(1e-6)) as i64) % 2 == 1 {
            k = 1.0 - (local / tw.dur.max(1e-6)).fract();
        }
        let e = funkin_core::camera::ease(&tw.ease, k);
        let target = tw.target.clone();
        let fields: Vec<(String, Option<f64>, f64)> = tw.fields.clone();
        let color = tw.color;
        let num = tw.num.clone();
        let path = tw.path.clone();
        let on_update = tw.on_update.clone();
        let mut writes = Vec::new();
        for (j, (name, from, to)) in fields.iter().enumerate() {
            let from = match from {
                Some(f) => *f,
                None => {
                    let f = interp.get(ctx, &target, name).map(|v| v.f()).unwrap_or(0.0);
                    ctx.state.tweens[i].fields[j].1 = Some(f);
                    f
                }
            };
            writes.push((name.clone(), from + (to - from) * e));
        }
        for (name, v) in writes {
            let _ = interp.set(ctx, &target, &name, Value::Float(v));
        }
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            // Quadratic segments (start, control, end, control, end, ...).
            let segs = ((p.len() - 1) / 2).max(1);
            let at = (e * segs as f64).min(segs as f64 - 1e-9).max(0.0);
            let si = at as usize;
            let u = at - si as f64;
            let pt = |n: usize| p[n.min(p.len() - 1)];
            let (a, c, b) = (pt(si * 2), pt(si * 2 + 1), pt(si * 2 + 2));
            let q = |a: f64, c: f64, b: f64| (1.0 - u) * (1.0 - u) * a + 2.0 * (1.0 - u) * u * c + u * u * b;
            let _ = interp.set(ctx, &target, "x", Value::Float(q(a.0, c.0, b.0)));
            let _ = interp.set(ctx, &target, "y", Value::Float(q(a.1, c.1, b.1)));
        }
        if let Some((a, b)) = color {
            let (ca, cb) = (int_to_color(a as i64), int_to_color(b as i64));
            let c: [f32; 4] = std::array::from_fn(|n| ca[n] + (cb[n] - ca[n]) * e as f32);
            let _ = interp.set(ctx, &target, "color", Value::Int(color_to_int(c)));
        }
        let tw_val = Value::Host(HostRef { kind: K_TWEEN, id });
        if let Some((a, b)) = num.as_ref().map(|(a, b, _)| (*a, *b)) {
            // `FlxTween.num`'s current value, for `twn.value` in onUpdate.
            ctx.state.tween_values.insert(id, a + (b - a) * e);
        }
        if let Some((a, b, cb)) = num.filter(|(_, _, cb)| !cb.is_null()) {
            if let Err(err) = interp.call(ctx, &cb, vec![Value::Float(a + (b - a) * e)]) {
                if failed.insert(format!("tween:{err}")) {
                    log::warn!("script tween: {err}");
                }
            }
        }
        if !on_update.is_null() {
            let _ = interp.call(ctx, &on_update, vec![tw_val.clone()]);
        }
        // Callbacks may have removed or added tweens: find ours again.
        let Some(pos) = ctx.state.tweens.iter().position(|t| t.id == id) else { continue };
        i = pos;
        if finished {
            let tw = &mut ctx.state.tweens[i];
            if tw.looping || tw.pingpong {
                tw.t = tw.delay;
            } else {
                let cb = tw.on_complete.clone();
                ctx.state.tweens.remove(i);
                if !cb.is_null() {
                    if let Err(err) = interp.call(ctx, &cb, vec![tw_val]) {
                        if failed.insert(format!("tween:{err}")) {
                            log::warn!("script tween onComplete: {err}");
                        }
                    }
                }
                i = ctx.state.tweens.iter().position(|t| t.id > id).unwrap_or(ctx.state.tweens.len());
                continue;
            }
        }
        i += 1;
    }
}

/// `animation.onFrameChange` / `onFinish` / `finishCallback`.
fn run_signals(interp: &mut Interp, ctx: &mut Ctx, failed: &mut HashSet<String>) {
    let ids: Vec<u64> = ctx.state.signals.keys().copied().collect();
    for id in ids {
        let Some(a) = ctx.actor(id) else {
            ctx.state.signals.remove(&id);
            continue;
        };
        let now = (a.current().to_owned(), a.frame_number(), a.finished());
        let label = a.frame_label();
        if now.0.is_empty() {
            continue;
        }
        let Some(sig) = ctx.state.signals.get_mut(&id) else { continue };
        let last = sig.last.replace(now.clone());
        let changed = last.as_ref().map(|l| l.0 != now.0 || l.1 != now.1).unwrap_or(true);
        let finished_now = now.2 && last.as_ref().map(|l| !l.2 || l.0 != now.0).unwrap_or(true);
        let mut calls: Vec<(Value, Vec<Value>)> = Vec::new();
        if changed {
            for (cb, _) in &sig.frame {
                calls.push((cb.clone(), vec![Value::str(&now.0), Value::Int(now.1 as i64), Value::Int(now.1 as i64)]));
            }
            sig.frame.retain(|(_, once)| !once);
            if let Some(l) = &label {
                for (cb, _) in &sig.label {
                    calls.push((cb.clone(), vec![Value::str(l)]));
                }
                sig.label.retain(|(_, once)| !once);
            }
        }
        if finished_now {
            for (cb, _) in &sig.finish {
                calls.push((cb.clone(), vec![Value::str(&now.0)]));
            }
            sig.finish.retain(|(_, once)| !once);
            if !sig.finish_callback.is_null() {
                calls.push((sig.finish_callback.clone(), vec![Value::str(&now.0)]));
            }
        }
        for (cb, args) in calls {
            if let Err(e) = interp.call(ctx, &cb, args) {
                if failed.insert(format!("anim:{e}")) {
                    log::warn!("script animation callback: {e}");
                }
            }
        }
    }
}

fn run_timers(interp: &mut Interp, ctx: &mut Ctx, failed: &mut HashSet<String>, dt: f64) {
    let ids: Vec<u64> = ctx.state.timers.iter().map(|t| t.id).collect();
    for id in ids {
        let Some(t) = ctx.state.timers.iter_mut().find(|t| t.id == id) else { continue };
        if !t.active {
            continue;
        }
        t.t += dt;
        if t.t < t.time {
            continue;
        }
        t.t -= t.time;
        t.done += 1;
        let finished = t.loops > 0 && t.done >= t.loops;
        if finished {
            t.active = false;
        }
        let cb = t.cb.clone();
        if !cb.is_null() {
            if let Err(err) = interp.call(ctx, &cb, vec![Value::Host(HostRef { kind: K_TIMER, id })]) {
                if failed.insert(format!("timer:{err}")) {
                    log::warn!("script timer: {err}");
                }
            }
        }
    }
    ctx.state.timers.retain(|t| t.active);
}

impl Ctx<'_> {
    fn stub(&mut self, name: &str) -> Value {
        let id = match self.state.stubs.iter().position(|s| s == name) {
            Some(i) => i,
            None => {
                self.state.stubs.push(name.to_owned());
                self.state.stubs.len() - 1
            }
        };
        Value::Host(HostRef { kind: K_STUB, id: id as u64 })
    }

    fn actor(&mut self, id: u64) -> Option<&mut crate::actor::Actor> {
        if id & crate::dialogue::DLG != 0 {
            return self.state.dialogue.as_mut()?.actor(id);
        }
        let w = self.world.as_deref_mut()?;
        if id & CHAR != 0 {
            w.chars.get_mut((id & 0xFF) as usize)?.as_mut().map(|c| &mut c.actor)
        } else {
            w.props.get_mut(id as usize)
        }
    }

    fn sprite_ref(id: u64) -> Value {
        Value::Host(HostRef { kind: K_SPRITE, id })
    }

    /// The engine sprite behind a value (a sprite, or a script instance extending one).
    fn sprite_id(v: &Value) -> Option<u64> {
        match v {
            Value::Host(HostRef { kind: K_SPRITE, id }) => Some(*id),
            Value::Instance(i) => Self::sprite_id(&i.base.borrow()),
            _ => None,
        }
    }

    /// `PlayState.instance.currentStage`: the stage script when there is one, so
    /// its overrides (`addCharacter`, ...) run.
    fn current_stage(&self) -> Value {
        let stage = HostRef { kind: K_STAGE, id: 0 };
        self.state.bound.iter().find(|b| matches!(b, Value::Instance(i) if i.base_ref() == Some(stage))).cloned().unwrap_or(Value::Host(stage))
    }

    /// The group behind a value (a group, or a script class extending one).
    fn group_id(v: &Value) -> Option<usize> {
        match v {
            Value::Host(HostRef { kind: K_GROUP, id }) => Some(*id as usize),
            Value::Instance(i) => Self::group_id(&i.base.borrow()),
            _ => None,
        }
    }

    fn load_font(&self, file: &str) -> Option<Rc<crate::gfx::ttf::TtfFont>> {
        self.assets.ttf_path(file).and_then(|p| crate::gfx::ttf::TtfFont::load(&p).ok()).map(Rc::new)
    }

    fn new_text(&mut self, x: f64, y: f64, field_width: f64, text: String, size: f64) -> Value {
        let font = self.load_font("nokiafc22.ttf").or_else(|| self.load_font("vcr.ttf"));
        self.state.texts.push(ScriptText {
            x: x as f32,
            y: y as f32,
            text,
            size: size as f32,
            color: [1.0; 4],
            border_color: [0.0, 0.0, 0.0, 1.0],
            border: 1.0,
            outline: false,
            alpha: 1.0,
            visible: true,
            font,
            added: false,
            field_width: field_width.max(0.0) as f32,
            align: 0,
        });
        Value::Host(HostRef { kind: K_STEXT, id: self.state.texts.len() as u64 - 1 })
    }

    fn text_get(&mut self, id: u64, field: &str) -> Value {
        let Some(t) = self.state.texts.get(id as usize) else { return Value::Null };
        match field {
            "x" => Value::Float(t.x as f64),
            "y" => Value::Float(t.y as f64),
            "text" => Value::str(&t.text),
            "size" => Value::Float(t.size as f64),
            "alpha" => Value::Float(t.alpha as f64),
            "visible" => Value::Bool(t.visible),
            "borderSize" => Value::Float(t.border as f64),
            "color" => Value::Int(color_to_int(t.color)),
            "height" => {
                let lines = t.text.lines().count().max(1) as f32;
                Value::Float((t.font.as_ref().map(|f| f.line_height(t.size)).unwrap_or(t.size) * lines) as f64 + 4.0)
            }
            "fieldWidth" => Value::Float(t.field_width as f64),
            "alignment" => Value::str(["left", "center", "right"][t.align as usize]),
            "width" if t.field_width > 0.0 => Value::Float(t.field_width as f64),
            "width" => {
                let (font, size, text) = (t.font.clone(), t.size, t.text.clone());
                let w = match (font, self.gfx.as_deref_mut()) {
                    (Some(f), Some(g)) => text.lines().map(|l| f.width(g, l, size)).fold(0.0, f32::max),
                    _ => size * text.len() as f32 * 0.6,
                };
                Value::Float(w as f64 + 4.0)
            }
            "scrollFactor" => Value::object(vec![
                ("x", Value::Float(0.0)),
                ("y", Value::Float(0.0)),
                ("set", Value::native(|_, _, _, _| Ok(Value::Null))),
            ]),
            _ => Value::Null,
        }
    }

    fn text_set(&mut self, id: u64, field: &str, v: &Value) {
        let Some(t) = self.state.texts.get_mut(id as usize) else { return };
        match field {
            "x" => t.x = v.f() as f32,
            "y" => t.y = v.f() as f32,
            "text" => t.text = v.to_string(),
            "size" => t.size = v.f() as f32,
            "alpha" => t.alpha = v.f().clamp(0.0, 1.0) as f32,
            "visible" => t.visible = v.truthy(),
            "borderSize" => t.border = v.f() as f32,
            "color" => t.color = int_to_color(v.i()),
            "fieldWidth" => t.field_width = v.f().max(0.0) as f32,
            "alignment" => t.align = text_align(&v.to_string()),
            _ => {}
        }
    }

    /// `FlxText.setFormat(font, size, color, alignment, borderStyle, borderColor)`.
    fn text_format(&mut self, id: u64, a: &[Value]) {
        let font = match arg(a, 0) {
            Value::Str(f) => {
                let f = f.to_string();
                let name = f.rsplit(['/', '\\']).next().unwrap_or(&f).to_owned();
                self.load_font(&name)
            }
            _ => None,
        };
        let Some(t) = self.state.texts.get_mut(id as usize) else { return };
        if font.is_some() {
            t.font = font;
        }
        if a.len() > 1 {
            t.size = arg(a, 1).f() as f32;
        }
        if a.len() > 2 && !arg(a, 2).is_null() {
            t.color = int_to_color(arg(a, 2).i());
        }
        if a.len() > 3 && !arg(a, 3).is_null() {
            t.align = text_align(&arg(a, 3).to_string());
        }
        // FlxTextBorderStyle: NONE, SHADOW, OUTLINE, OUTLINE_FAST.
        if a.len() > 4 {
            t.outline = arg(a, 4).to_string().contains("OUTLINE");
        }
        if a.len() > 5 {
            t.border_color = int_to_color(arg(a, 5).i());
        }
    }

    /// `new FlxSprite(x, y)`: an empty sprite, drawn once `add`ed.
    fn new_sprite(&mut self, x: f64, y: f64) -> Option<u64> {
        let w = self.world.as_deref_mut()?;
        let mut a = Actor::solid("", [1.0, 1.0, 1.0, 0.0], 1.0, 1.0);
        a.x = x as f32;
        a.y = y as f32;
        a.should_bop = false;
        // Flixel's offset stays zero until the script calls updateHitbox.
        a.offset = Some([0.0, 0.0]);
        Some(w.new_prop(a) as u64)
    }

    /// Load `key` into a sprite: `kind` is `image`, `sparrow` or `animateatlas`.
    fn load_into(&mut self, id: u64, kind: &str, key: &str) -> Result<(), String> {
        let level = self.world.as_deref().and_then(|w| w.stage.directory.clone());
        let assets = self.assets;
        let scope = self.tex_scope;
        let gfx = self.gfx.as_deref_mut().ok_or("no renderer")?;
        let actor = gfx.with_scope(scope, |g| {
            if kind == "image" {
                Actor::image(g, assets, "", key, level.as_deref(), true)
            } else {
                Actor::animated(g, assets, "", kind, key, level.as_deref(), &[], true)
            }
        })?;
        self.actor(id).ok_or("sprite is gone")?.take_graphics(actor);
        Ok(())
    }

    fn load_logged(&mut self, interp: &mut Interp, id: u64, kind: &str, key: &str) {
        if let Err(e) = self.load_into(id, kind, key) {
            interp.warn_once(format!("could not load {kind} {key}: {e}"));
        }
    }

    /// `add(obj)` on the stage or PlayState.
    fn add_obj(&mut self, v: &Value, add: bool) {
        if let Value::Host(HostRef { kind: K_STEXT, id }) = v {
            if let Some(t) = self.state.texts.get_mut(*id as usize) {
                t.added = add;
            }
            return;
        }
        if let Some(g) = Self::group_id(v) {
            let Some(grp) = self.state.groups.get_mut(g) else { return };
            grp.added = add;
            let (members, z) = (grp.members.clone(), grp.z);
            for m in &members {
                if add {
                    if let Some(a) = Self::sprite_id(m).and_then(|id| self.actor(id)) {
                        a.z_index = z;
                    }
                }
                self.add_obj(m, add);
            }
            return;
        }
        if let Value::Host(HostRef { kind: K_TRAIL, id }) = v {
            if let Some(w) = self.world.as_deref_mut() {
                w.show_trail(*id as usize, add);
            }
            return;
        }
        let Some(id) = Self::sprite_id(v) else { return };
        if id & CHAR != 0 {
            return;
        }
        if let Some(w) = self.world.as_deref_mut() {
            if add {
                w.add_prop(id as usize);
            } else {
                w.remove_prop(id as usize);
            }
        }
        if let Value::Instance(inst) = v {
            let listed = self.state.updatables.iter().any(|u| u.equals(v));
            if add && !listed && inst.class.has_method("update") {
                self.state.updatables.push(v.clone());
            } else if !add {
                self.state.updatables.retain(|u| !u.equals(v));
            }
        }
    }

    /// `FunkinSprite.create*` and `new FlxSprite(x, y, graphic)`.
    /// A text file by asset key (`Paths.frag` gives `@shaders/name.frag`).
    fn text_asset(&self, key: &str) -> Option<String> {
        let rel = key.strip_prefix("@").unwrap_or(key);
        let rel = rel.split_once(':').map_or(rel, |(_, k)| k);
        let path = self.assets.root().join(rel);
        String::from_utf8(funkin_core::vfs::read(&path).ok()?).ok()
    }

    /// `Paths.image(key)` as a file, looked up in the stage's level library first.
    fn image_file(&self, key: &str) -> std::path::PathBuf {
        let level = self.world.as_deref().and_then(|w| w.stage.directory.clone());
        self.assets.level_image(key, level.as_deref(), "png")
    }

    /// A character as a free-standing sprite (`CharacterDataParser.fetchCharacter`).
    fn fetch_character(&mut self, id: &str) -> Result<Value, String> {
        let assets = self.assets;
        let scope = self.tex_scope;
        let gfx = self.gfx.as_deref_mut().ok_or("no renderer")?;
        let actor = gfx.with_scope(scope, |g| crate::world::character_actor(g, assets, id))?;
        let w = self.world.as_deref_mut().ok_or("no stage")?;
        Ok(Self::sprite_ref(w.new_prop(actor) as u64))
    }

    /// A fetched character's script class, bound to its sprite (the script
    /// instance is what `fetchCharacter` returns, so its overrides run).
    fn bind_fetched(&mut self, interp: &mut Interp, id: &str, v: Value) -> Value {
        let (Some(name), Some(sid)) = (self.state.char_classes.get(id).cloned(), Self::sprite_id(&v)) else { return v };
        let Some(cls) = interp.class(&name) else { return v };
        let prev = self.state.binding.replace(HostRef { kind: K_SPRITE, id: sid });
        let r = interp.instantiate(self, &cls, vec![]);
        self.state.binding = prev;
        match r {
            Ok(inst) => {
                log::info!("script {name} bound to fetched {id}");
                if let Some(a) = self.actor(sid) {
                    a.anim_log = Some(Vec::new());
                }
                let iv = Value::Instance(inst);
                self.state.bound.push(iv.clone());
                iv
            }
            Err(e) => {
                log::warn!("script {name}: {e}");
                v
            }
        }
    }

    /// A character's `cameraFocusPoint`.
    fn char_focus(&self, id: u64) -> Option<[f64; 2]> {
        self.world.as_deref().and_then(|w| w.chars.get((id & 0xFF) as usize)?.as_ref().map(|c| c.focus))
    }

    /// The bound script class of a character sprite (`class X extends BaseCharacter`).
    fn char_script(&self, id: u64) -> Option<Value> {
        self.state.bound.iter().find(|b| matches!(b, Value::Instance(i) if i.base_ref() == Some(HostRef { kind: K_SPRITE, id }))).cloned()
    }

    /// A new engine shader object.
    fn make_shader(&mut self, class: crate::shaders::Class, src: Option<String>) -> Value {
        let id = self.state.shaders.len() as u64;
        let inst = crate::shaders::ShaderInst::new(id, class, src, |name| self.text_asset(&format!("shaders/{name}.frag")));
        self.state.shaders.push(Rc::new(RefCell::new(inst)));
        Value::Host(HostRef { kind: K_SHADER, id })
    }

    /// The shader a value is (a shader object, or a script class extending one).
    fn shader_of(&self, v: &Value) -> Option<Rc<RefCell<crate::shaders::ShaderInst>>> {
        let r = match v {
            Value::Host(h) => *h,
            Value::Instance(i) => i.base_ref()?,
            _ => return None,
        };
        match r.kind {
            K_SHADER | K_FILTER => self.state.shaders.get(r.id as usize).cloned(),
            _ => None,
        }
    }

    /// The game camera's view (left, top, right, bottom) in the world: `FlxCamera.viewLeft` ...
    fn camera_view(&self) -> [f64; 4] {
        let Some(cam) = self.world.as_deref().map(|w| &w.camera) else {
            return [0.0, 0.0, 1280.0, 720.0];
        };
        let z = cam.zoom();
        let (w, h) = (cam.view_w / z, cam.view_h / z);
        let left = cam.scroll[0] + (cam.view_w - w) / 2.0;
        let top = cam.scroll[1] + (cam.view_h - h) / 2.0;
        [left, top, left + w, top + h]
    }

    /// `camera.filters = [...]`: the shader filters among them.
    fn set_filters(&mut self, v: &Value) {
        let mut out = Vec::new();
        if let Value::Array(a) = v {
            for f in a.borrow().iter() {
                if let Value::Host(HostRef { kind: K_FILTER, id }) = f {
                    if *id != NO_FILTER {
                        if let Some(s) = self.state.shaders.get(*id as usize) {
                            out.push(s.clone());
                        }
                    }
                }
            }
        }
        self.state.cam_filters = out;
    }

    fn shader_get(&mut self, id: u64, field: &str) -> Option<Value> {
        let s = self.state.shaders.get(id as usize)?.clone();
        let s = s.borrow();
        if s.has_prop(field) {
            let v = s.prop(field);
            return Some(match field {
                "spriteMode" | "useAltMask" | "isCharacter" => Value::Bool(v != 0.0),
                "color" | "rainColor" | "pulseLightColor" | "numLightsSwag" => Value::Int(v as i64),
                _ => Value::Float(v),
            });
        }
        drop(s);
        // `shader.uniformName` (a ShaderParameter / ShaderInput).
        let i = match self.state.param_names.iter().position(|n| n == field) {
            Some(i) => i,
            None => {
                self.state.param_names.push(field.to_owned());
                self.state.param_names.len() - 1
            }
        };
        Some(Value::Host(HostRef { kind: K_SPARAM, id: (id << 20) | i as u64 }))
    }

    fn shader_set(&mut self, interp: &mut Interp, id: u64, field: &str, v: &Value) {
        let Some(s) = self.state.shaders.get(id as usize).cloned() else { return };
        let mut s = s.borrow_mut();
        let n = match v {
            Value::Bool(b) => *b as i32 as f64,
            v => v.f(),
        };
        match field {
            "attachedSprite" => s.auto_frame = !v.is_null(),
            "altMaskImage" => {
                if let Value::Str(k) = v {
                    s.set_image("altMask", self.image_file(k));
                }
            }
            "blurredScreen" | "mask" | "lightMap" | "puddleMap" => {}
            _ => {
                if !s.set_prop(field, n) {
                    interp.warn_once(format!("{}.{field} is not available to scripts yet", s.name()));
                }
            }
        }
    }

    fn shader_call(&mut self, interp: &mut Interp, id: u64, method: &str, a: &[Value]) -> Value {
        let Some(s) = self.state.shaders.get(id as usize).cloned() else { return Value::Null };
        let view = self.camera_view();
        let mut s = s.borrow_mut();
        let name = arg(a, 0).to_string();
        let nums = |v: &Value| -> Vec<f64> {
            match v {
                Value::Array(a) => a.borrow().iter().map(|x| if let Value::Bool(b) = x { *b as i32 as f64 } else { x.f() }).collect(),
                Value::Bool(b) => vec![*b as i32 as f64],
                v => vec![v.f()],
            }
        };
        match method {
            "setFloat" | "setInt" | "setBool" | "setFloatArray" | "setIntArray" | "setBoolArray" => {
                let mut v = nums(&arg(a, 1));
                if method == "setInt" {
                    v = v.into_iter().map(f64::trunc).collect();
                }
                s.set_uniform(&name, v);
            }
            "getFloat" | "getInt" => return s.uniform(&name).and_then(|v| v.first()).map(|v| if method == "getInt" { Value::Int(*v as i64) } else { Value::Float(*v) }).unwrap_or(Value::Null),
            "getBool" => return Value::Bool(s.uniform(&name).and_then(|v| v.first()).is_some_and(|v| *v != 0.0)),
            "getFloatArray" | "getIntArray" | "getBoolArray" => {
                return s.uniform(&name).map(|v| Value::array(v.iter().map(|x| Value::Float(*x)).collect())).unwrap_or(Value::Null)
            }
            "setSampler2D" | "setBitmapData" => match arg(a, 1) {
                Value::Str(k) => s.set_image(&name, self.image_file(&k)),
                Value::Host(HostRef { kind: K_BITMAP, id }) => s.set_texture(&name, crate::gfx::TexId::from_index(id as usize)),
                _ => {}
            },
            "loadAltMask" => s.set_image("altMask", self.image_file(&name)),
            "setAdjustColor" => {
                for (i, p) in ["baseBrightness", "baseHue", "baseContrast", "baseSaturation"].iter().enumerate() {
                    s.set_prop(p, arg(a, i).f());
                }
            }
            "setAmount" => {
                s.set_prop("amount", arg(a, 0).f());
            }
            "update" => {
                let t = s.prop("time") + arg(a, 0).f();
                s.set_prop("time", t);
            }
            "updateViewInfo" => s.view_info(arg(a, 0).f(), arg(a, 1).f(), view),
            "updateFrameInfo" | "onAttachedFrame" => s.auto_frame = true,
            "toString" => return Value::from(s.name().to_owned()),
            _ => interp.warn_once(format!("{}.{method}() is not available to scripts yet", s.name())),
        }
        Value::Null
    }

    fn make_sprite(&mut self, interp: &mut Interp, x: f64, y: f64, kind: &str, key: &str) -> Value {
        match self.new_sprite(x, y) {
            Some(id) => {
                if !key.is_empty() {
                    self.load_logged(interp, id, kind, key);
                }
                Self::sprite_ref(id)
            }
            None => self.stub("FlxSprite"),
        }
    }

    /// `Paths.sound(key)`: a bare key is looked up in the stage's level library first.
    pub(crate) fn sound_key(&self, key: &str) -> String {
        if !key.contains(':') {
            if let Some(level) = self.world.as_deref().and_then(|w| w.stage.directory.as_deref()) {
                let spec = format!("{level}:{key}");
                if funkin_core::vfs::exists(&self.assets.sound_path(&spec)) {
                    return spec;
                }
            }
        }
        key.to_owned()
    }

    /// A sound from `Paths.sound` / `Paths.music` (`@music/` keys).
    fn load_pcm(&self, key: &str) -> Option<std::sync::Arc<crate::audio::Pcm>> {
        let (lib, k) = key.split_once(':').unwrap_or(("", key));
        if let Some(m) = k.strip_prefix("@music/") {
            let lib = if lib.is_empty() { self.world.as_deref().and_then(|w| w.stage.directory.clone()).unwrap_or_default() } else { lib.to_owned() };
            let path = self.assets.root().join(&lib).join("music").join(format!("{m}.ogg"));
            let path = if funkin_core::vfs::exists(&path) { path } else { self.assets.root().join("shared").join("music").join(format!("{m}.ogg")) };
            return match crate::audio::Pcm::load_ogg(&path) {
                Ok(p) => Some(std::sync::Arc::new(p)),
                Err(e) => {
                    log::warn!("script music {key}: {e}");
                    None
                }
            };
        }
        if k.ends_with(".ogg") {
            // `FunkinSound.load('sounds/x/y.ogg')`: a path under the assets, as given.
            let root = self.assets.root();
            let mut tries = vec![];
            if !lib.is_empty() {
                tries.push(root.join(lib).join(k));
            }
            if let Some(level) = self.world.as_deref().and_then(|w| w.stage.directory.clone()) {
                tries.push(root.join(level).join(k));
            }
            tries.push(root.join("shared").join(k));
            tries.push(root.join(k));
            let Some(path) = tries.into_iter().find(|p| funkin_core::vfs::exists(p)) else {
                log::warn!("script sound {key}: not found");
                return None;
            };
            return match crate::audio::Pcm::load_ogg(&path) {
                Ok(p) => Some(std::sync::Arc::new(p)),
                Err(e) => {
                    log::warn!("script sound {key}: {e}");
                    None
                }
            };
        }
        self.assets.sound(&self.sound_key(key))
    }

    fn new_tween(&mut self, tw: impl FnOnce(u64) -> Tween) -> Value {
        let id = self.state.next_id;
        self.state.next_id += 1;
        self.state.tweens.push(tw(id));
        Value::Host(HostRef { kind: K_TWEEN, id })
    }

    /// FlxTween options: ease, onComplete, onUpdate, startDelay, type.
    fn tween_opts(opts: &Value) -> (String, Value, Value, f64, bool, bool) {
        let ease = opts.field("ease").map(|e| e.to_string()).filter(|e| e != "null").unwrap_or_else(|| "linear".into());
        let ty = opts.field("type").map(|t| t.i()).unwrap_or(0);
        (
            ease,
            opts.field("onComplete").unwrap_or(Value::Null),
            opts.field("onUpdate").unwrap_or(Value::Null),
            opts.field("startDelay").map(|d| d.f()).unwrap_or(0.0),
            ty == 2,
            ty == 4,
        )
    }

    fn class_get(&mut self, interp: &mut Interp, class: &str, field: &str) -> Option<Value> {
        if let Some(v) = self.state.class_statics.get(&(class.to_owned(), field.to_owned())) {
            return Some(v.clone());
        }
        let s = &*self.state;
        Some(match (class, field) {
            ("FlxBarFillDirection", d) => Value::str(d),
            ("FlxG", "random") => obj_random(),
            ("FlxG", "width") => Value::Int(1280),
            ("FlxG", "height") => Value::Int(720),
            ("FlxG", "elapsed") => Value::Float(1.0 / 60.0),
            ("FlxG", "camera") => Value::Host(HostRef { kind: K_CAMERA, id: 0 }),
            ("FlxG", "cameras") => Value::Host(HostRef { kind: K_CAMLIST, id: 0 }),
            ("FlxG", "keys") => Value::Host(HostRef { kind: K_KEYS, id: 0 }),
            ("FlxG", "onMobile") => Value::Bool(false),
            ("PlayState", "instance") => Value::Host(HostRef { kind: K_PLAYSTATE, id: 0 }),
            ("Conductor", "instance") => Value::Host(HostRef { kind: K_CONDUCTOR, id: 0 }),
            ("Conductor", "beatHit") => Value::Host(HostRef { kind: K_BEATSIG, id: 0 }),
            ("Save", "instance") => save_instance(),
            ("SongRegistry", "instance") => {
                // parseMusicData(id): `music/<id>/<id>-metadata.json`, in the stage's
                // library first (weekend1's cutscene music lives there), then the default one.
                let root = self.assets.root().to_path_buf();
                let level = self.world.as_deref().and_then(|w| w.stage.directory.clone());
                let parse = Value::native(move |_, _, _, a| {
                    let id = arg(&a, 0).to_string();
                    let rel = std::path::Path::new("music").join(&id).join(format!("{id}-metadata.json"));
                    let mut tries: Vec<std::path::PathBuf> = level.iter().map(|l| root.join(l).join(&rel)).collect();
                    tries.push(root.join(&rel));
                    for p in tries {
                        if let Ok(text) = funkin_core::vfs::read_to_string(&p) {
                            return Ok(match serde_json::from_str::<serde_json::Value>(&text) {
                                Ok(j) => json_to_value(&j),
                                Err(e) => {
                                    log::warn!("SongRegistry.parseMusicData({id}): {e}");
                                    Value::Null
                                }
                            });
                        }
                    }
                    Ok(Value::Null)
                });
                Value::object(vec![("parseMusicData", parse), (
                "parseMusicDataRaw",
                Value::native(|_, _, _, a| {
                    let text = arg(&a, 0).to_string();
                    Ok(match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(j) => json_to_value(&j),
                        Err(e) => {
                            log::warn!("SongRegistry.parseMusicDataRaw: {e}");
                            Value::Null
                        }
                    })
                }),
            )])
            }
            // funkin.play.scoring.Scoring (PBOT1 windows, ms).
            ("Scoring", "PBOT1_SICK_THRESHOLD") => Value::Float(45.0),
            ("Scoring", "PBOT1_GOOD_THRESHOLD") => Value::Float(90.0),
            ("Scoring", "PBOT1_BAD_THRESHOLD") => Value::Float(135.0),
            ("Scoring", "PBOT1_SHIT_THRESHOLD") => Value::Float(160.0),
            ("Scoring", "PBOT1_MISS_THRESHOLD") => Value::Float(160.0),
            ("PlayStatePlaylist", "isStoryMode") => Value::Bool(s.is_story),
            ("PlayStatePlaylist", "campaignDifficulty") => Value::str(&s.difficulty),
            ("CountdownStep", s @ ("BEFORE" | "THREE" | "TWO" | "ONE" | "GO" | "AFTER")) => Value::str(s),
            ("CharacterType", "BF") => Value::str("bf"),
            ("CharacterType", "DAD") => Value::str("dad"),
            ("CharacterType", "GF") => Value::str("gf"),
            ("CharacterType", "OTHER") => Value::str("other"),
            ("FlxColor", c) => match parse_color(c) {
                Some(v) => Value::Int(v),
                None => return self.flxcolor_fn(c),
            },
            ("FlxEase", e) => Value::str(e),
            ("CutsceneType", t) => Value::str(t),
            ("FlxTextBorderStyle", t) => Value::str(t),
            // The port always draws at 1280x720.
            ("FullScreenScaleMode", "wideScale" | "gameCutoutSize") => Value::object(vec![("x", Value::Float(1.0)), ("y", Value::Float(1.0))]),
            ("WiggleEffectType", t) => match ["DREAMY", "WAVY", "HEAT_WAVE_HORIZONTAL", "HEAT_WAVE_VERTICAL", "FLAG"].iter().position(|n| *n == t) {
                Some(i) => Value::Int(i as i64),
                None => return None,
            },
            ("VideoCutscene", sig) if VIDEO_SIGNALS.contains(&sig) => {
                Value::Host(HostRef { kind: K_VSIGNAL, id: VIDEO_SIGNALS.iter().position(|s| *s == sig).unwrap_or(0) as u64 })
            }
            ("Constants", "CENSOR_EXPLETIVES") => Value::Bool(false),
            ("Constants", "PIXEL_ART_SCALE") => Value::Float(6.0),
            ("ConversationState", st) => Value::str(st),
            ("Preferences", "naughtyness") => Value::Bool(s.naughty),
            ("Preferences", "flashingLights" | "zoomCamera" | "cameraZoomRate") => Value::Bool(true),
            ("Preferences", "downscroll" | "middlescroll" | "lowQuality") => Value::Bool(false),
            _ => {
                let _ = interp;
                return None;
            }
        })
    }

    fn flxcolor_fn(&mut self, name: &str) -> Option<Value> {
        Some(match name {
            "fromString" => Value::native(|_, _, _, a| Ok(parse_color(&arg(&a, 0).to_string()).map(Value::Int).unwrap_or(Value::Null))),
            "fromRGB" => Value::native(|_, _, _, a| {
                let alpha = if a.len() > 3 { arg(&a, 3).i() } else { 255 };
                Ok(Value::Int(((alpha & 255) << 24) | ((arg(&a, 0).i() & 255) << 16) | ((arg(&a, 1).i() & 255) << 8) | (arg(&a, 2).i() & 255)))
            }),
            "fromRGBFloat" => Value::native(|_, _, _, a| {
                let alpha = if a.len() > 3 { arg(&a, 3).f() } else { 1.0 };
                Ok(Value::Int(color_to_int([arg(&a, 0).f() as f32, arg(&a, 1).f() as f32, arg(&a, 2).f() as f32, alpha as f32])))
            }),
            "interpolate" => Value::native(|_, _, _, a| {
                let (x, y, t) = (int_to_color(arg(&a, 0).i()), int_to_color(arg(&a, 1).i()), arg(&a, 2).f() as f32);
                Ok(Value::Int(color_to_int(std::array::from_fn(|n| x[n] + (y[n] - x[n]) * t))))
            }),
            _ => return None,
        })
    }

    fn class_call(&mut self, interp: &mut Interp, class: &str, method: &str, a: Vec<Value>) -> Result<Option<Value>, String> {
        Ok(Some(match (class, method) {
            // funkin.util.MathUtil
            ("MathUtil", "smoothLerpPrecision" | "smoothLerp") => {
                let (base, target, dt, dur) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f(), arg(&a, 3).f());
                let precision = if a.len() > 4 { arg(&a, 4).f() } else { 0.01 };
                Value::Float(if dt == 0.0 {
                    base
                } else if base == target {
                    target
                } else {
                    target + (base - target) * precision.powf(dt / dur)
                })
            }
            ("MathUtil", "smoothLerpHalfLife") => {
                let (base, target, dt, half) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f(), arg(&a, 3).f());
                Value::Float(if dt == 0.0 || base == target { if base == target { target } else { base } } else { target + (base - target) * (-dt / half).exp2() })
            }
            ("MathUtil", "lerp") => {
                let (x, y, t) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f());
                Value::Float(x + t * (y - x))
            }
            ("MathUtil", "snap") => {
                let (b, t, th) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f());
                Value::Float(if (b - t).abs() <= th { t } else { b })
            }
            ("CharacterDataParser", "fetchCharacter") => {
                let id = arg(&a, 0).to_string();
                match self.fetch_character(&id) {
                    Ok(v) => self.bind_fetched(interp, &id, v),
                    Err(e) => {
                        interp.warn_once(format!("CharacterDataParser.fetchCharacter({id}): {e}"));
                        Value::Null
                    }
                }
            }
            // "lib:path" asset keys.
            ("Paths", "getLibrary") => {
                let k = arg(&a, 0).to_string();
                Value::from(k.split_once(':').map(|(l, _)| l.to_string()).unwrap_or_else(|| "preload".into()))
            }
            ("Paths", "stripLibrary") => {
                let k = arg(&a, 0).to_string();
                Value::from(k.split_once(':').map(|(_, p)| p.to_string()).unwrap_or(k))
            }
            ("DateUtil", "generateTimestamp") => {
                let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
                let (days, rem) = (secs.div_euclid(86400), secs.rem_euclid(86400));
                // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
                let z = days + 719468;
                let era = z.div_euclid(146097);
                let doe = z - era * 146097;
                let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
                let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
                let mp = (5 * doy + 2) / 153;
                let d = doy - (153 * mp + 2) / 5 + 1;
                let mo = if mp < 10 { mp + 3 } else { mp - 9 };
                let y = yoe + era * 400 + if mo <= 2 { 1 } else { 0 };
                Value::from(format!("{y:04}-{mo:02}-{d:02}-{:02}-{:02}-{:02}", rem / 3600, rem / 60 % 60, rem % 60))
            }
            ("Paths", "frag" | "vert") => Value::from(format!("@shaders/{}.{method}", arg(&a, 0))),
            ("Assets", "getText") => {
                let key = arg(&a, 0).to_string();
                match self.text_asset(&key) {
                    Some(t) => Value::from(t),
                    None => {
                        interp.warn_once(format!("Assets.getText: {key} not found"));
                        Value::Null
                    }
                }
            }
            ("Assets", "exists") => Value::Bool(self.text_asset(&arg(&a, 0).to_string()).is_some()),
            ("Assets", "getBitmapData") => arg(&a, 0),
            ("Paths", "sound" | "soundRandom" | "music" | "image" | "file" | "animateAtlas" | "getSparrowAtlas" | "video" | "videos" | "font" | "json" | "txt" | "xml") => {
                let key = if method == "soundRandom" {
                    let (lo, hi) = (arg(&a, 1).i(), arg(&a, 2).i());
                    let n = lo + (funkin_script::random() * (hi - lo + 1) as f64) as i64;
                    format!("{}{}", arg(&a, 0), n.min(hi))
                } else {
                    arg(&a, 0).to_string()
                };
                let key = if method == "music" { format!("@music/{key}") } else { key };
                let lib = arg(&a, 1);
                if method != "soundRandom" && !lib.is_null() {
                    Value::from(format!("{lib}:{key}"))
                } else {
                    Value::from(key)
                }
            }
            ("FunkinSprite", "create" | "createSparrow" | "createTextureAtlas" | "createPacker") => {
                let kind = match method {
                    "create" => "image",
                    "createTextureAtlas" => "animateatlas",
                    _ => "sparrow",
                };
                let mut key = arg(&a, 2).to_string();
                if a.len() > 3 && !arg(&a, 3).is_null() && !key.contains(':') {
                    key = format!("{}:{key}", arg(&a, 3));
                }
                self.make_sprite(interp, arg(&a, 0).f(), arg(&a, 1).f(), kind, &key)
            }
            ("FunkinMemory" | "HapticUtil", _) => Value::Null,
            ("FlxAnimationUtil", "addTextureAtlasAnimations" | "addAtlasAnimations") => {
                let anims: Vec<funkin_core::stage::AnimationData> = match arg(&a, 1) {
                    Value::Array(v) => v.borrow().iter().filter_map(|d| serde_json::from_value(value_to_json(d)).ok()).collect(),
                    _ => vec![],
                };
                if let Some(act) = Self::sprite_id(&arg(&a, 0)).and_then(|id| self.actor(id)) {
                    if method == "addTextureAtlasAnimations" {
                        act.add_atlas_anims(&anims);
                    } else {
                        for d in &anims {
                            act.add_sparrow_anim(&d.name, &d.prefix, &d.frame_indices, d.frame_rate as f32, d.looped, d.flip_x);
                        }
                    }
                }
                Value::Null
            }
            // funkin.util.tools.Objects: shallowCombine(a, b) = a's fields, then b's over them.
            ("Objects", "shallowCombine" | "merge") => {
                let out = Value::object(vec![]);
                for src in [arg(&a, 0), arg(&a, 1)] {
                    if let Value::Object(o) = &src {
                        for (k, v) in o.borrow().iter() {
                            out.set_field(k, v.clone());
                        }
                    }
                }
                out
            }
            ("Countdown", "skipCountdown") => {
                self.state.skip_countdown = true;
                Value::Null
            }
            ("ModuleHandler", "getModule") => {
                let id = arg(&a, 0).to_string();
                self.state.modules.iter().find(|(m, _)| *m == id).map(|(_, v)| v.clone()).unwrap_or(Value::Null)
            }
            ("ModuleHandler", _) => Value::Null,
            ("InputUtil", "getKeyName" | "format") => {
                let k = flx_key(&arg(&a, 0));
                let name = FLX_KEYS.iter().find(|x| Some(x.1) == k).map(|x| x.0).unwrap_or("?");
                Value::from(format!("{}{}", &name[..1], name[1..].to_lowercase()))
            }
            ("RetroCameraFade", "fadeBlack" | "fadeWhite" | "fadeToBlack" | "fadeFromWhite") => {
                let cam = match arg(&a, 0) {
                    Value::Host(HostRef { kind: K_CAMERA, id: 0 }) => 0,
                    _ => 1,
                };
                let (from, to) = match method {
                    "fadeBlack" => (-1.0, 0.0),
                    "fadeWhite" => (0.0, 1.0),
                    "fadeToBlack" => (0.0, -1.0),
                    _ => (1.0, 0.0),
                };
                let steps = if a.len() > 1 { arg(&a, 1).f() } else { 5.0 };
                let time = if a.len() > 2 { arg(&a, 2).f() } else { 1.0 };
                self.state.cam_fades[cam] = Some(CamFade { from, to, steps, time, t: 0.0 });
                Value::Null
            }
            ("Countdown", "performCountdown") => {
                self.state.start_countdown = true;
                Value::Null
            }
            ("VideoCutscene", "play") => {
                self.state.video_request = Some(arg(&a, 0).to_string());
                self.state.video_type = match arg(&a, 1).to_string().as_str() {
                    "ENDING" => 1,
                    "MIDSONG" => 2,
                    _ => 0,
                };
                self.state.video_hidden = false;
                self.state.video_black = true;
                self.state.hud_hidden = true;
                Value::Null
            }
            ("VideoCutscene", "isPlaying") => Value::Bool(self.state.video_playing || self.state.video_request.is_some()),
            ("VideoCutscene", "hideVideo" | "showVideo") => {
                self.state.video_hidden = method == "hideVideo";
                self.state.video_black = false;
                Value::Null
            }
            ("VideoCutscene", "finishVideo") => {
                self.state.video_finish = true;
                Value::Null
            }
            ("VideoCutscene", "destroyVideo" | "stopVideo") => {
                self.state.video_destroy = true;
                Value::Null
            }
            ("VideoCutscene", "pauseVideo" | "resumeVideo" | "restartVideo" | "fadeOutVideo") => Value::Null,
            ("FunkinSound", "load") => {
                let pcm = self.load_pcm(&arg(&a, 0).to_string());
                // Scripts pass `true` here too (hscript coerces it): full volume.
                let volume = match arg(&a, 1) {
                    Value::Bool(b) => b as i32 as f32,
                    Value::Null => 1.0,
                    v => v.f() as f32,
                };
                let looped = arg(&a, 2).truthy();
                self.state.sounds.push(ScriptSound { pcm, volume, looped, playing: None });
                let r = Value::Host(HostRef { kind: K_SOUND, id: self.state.sounds.len() as u64 - 1 });
                if arg(&a, 4).truthy() {
                    self.call(interp, HostRef { kind: K_SOUND, id: self.state.sounds.len() as u64 - 1 }, "play", vec![])?;
                }
                r
            }
            ("FunkinSound", "playOnce") => {
                let vol = if a.len() > 1 { arg(&a, 1).f() as f32 } else { 1.0 };
                // `playOnce(key, volume, onComplete, onLoad)`: onComplete when it ends.
                if let Some(pcm) = self.load_pcm(&arg(&a, 0).to_string()) {
                    let len = pcm.duration_ms() / 1000.0;
                    self.audio.play_sfx(pcm, vol);
                    let cb = arg(&a, 2);
                    if matches!(cb, Value::Func(_) | Value::Native(_)) {
                        let id = self.state.next_id;
                        self.state.next_id += 1;
                        // Called with no arguments (timers pass the timer).
                        let cb = Value::native(move |i, h, _, _| i.call(h, &cb, vec![]));
                        self.state.timers.push(Timer { id, mgr: None, time: len, t: 0.0, loops: 1, done: 0, cb, active: true });
                    }
                }
                Value::Null
            }
            ("FlxTween", "tween") => {
                let target = arg(&a, 0);
                let props = arg(&a, 1);
                let fields: Vec<(String, Option<f64>, f64)> = match &props {
                    Value::Object(o) => o.borrow().iter().map(|(k, v)| (k.to_string(), None, v.f())).collect(),
                    _ => vec![],
                };
                let dur = arg(&a, 2).f();
                let (ease, on_complete, on_update, delay, looping, pingpong) = Self::tween_opts(&arg(&a, 3));
                self.new_tween(|id| Tween { id, target, fields, color: None, num: None, path: None, dur, delay, t: 0.0, ease, on_update, on_complete, looping, pingpong, active: true })
            }
            ("FlxTween", "color") => {
                let target = arg(&a, 0);
                let dur = arg(&a, 1).f();
                let (from, to) = (arg(&a, 2).i() as u32, arg(&a, 3).i() as u32);
                let (ease, on_complete, on_update, delay, looping, pingpong) = Self::tween_opts(&arg(&a, 4));
                self.new_tween(|id| Tween { id, target, fields: vec![], color: Some((from, to)), num: None, path: None, dur, delay, t: 0.0, ease, on_update, on_complete, looping, pingpong, active: true })
            }
            ("FlxTween", "num") => {
                let (from, to, dur) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f());
                let (ease, on_complete, on_update, delay, looping, pingpong) = Self::tween_opts(&arg(&a, 3));
                let cb = arg(&a, 4);
                self.new_tween(|id| Tween { id, target: Value::Null, fields: vec![], color: None, num: Some((from, to, cb)), path: None, dur, delay, t: 0.0, ease, on_update, on_complete, looping, pingpong, active: true })
            }
            ("FlxTween", "angle") => {
                let target = arg(&a, 0);
                let fields = vec![("angle".to_owned(), Some(arg(&a, 1).f()), arg(&a, 2).f())];
                let dur = arg(&a, 3).f();
                let (ease, on_complete, on_update, delay, looping, pingpong) = Self::tween_opts(&arg(&a, 4));
                self.new_tween(|id| Tween { id, target, fields, color: None, num: None, path: None, dur, delay, t: 0.0, ease, on_update, on_complete, looping, pingpong, active: true })
            }
            ("FlxTween", "quadPath" | "linearPath") => {
                let target = arg(&a, 0);
                let mut pts: Vec<(f64, f64)> = match arg(&a, 1) {
                    Value::Array(v) => v.borrow().iter().map(|p| (p.field("x").map(|x| x.f()).unwrap_or(0.0), p.field("y").map(|y| y.f()).unwrap_or(0.0))).collect(),
                    _ => vec![],
                };
                if method == "linearPath" {
                    // A straight segment is a quad with its control at the midpoint.
                    let mut q = Vec::new();
                    for w in pts.windows(2) {
                        if q.is_empty() {
                            q.push(w[0]);
                        }
                        q.push(((w[0].0 + w[1].0) / 2.0, (w[0].1 + w[1].1) / 2.0));
                        q.push(w[1]);
                    }
                    pts = q;
                }
                let dur = arg(&a, 2).f();
                let (ease, on_complete, on_update, delay, looping, pingpong) = Self::tween_opts(&arg(&a, 4));
                let path = Some(pts);
                self.new_tween(|id| Tween { id, target, fields: vec![], color: None, num: None, path, dur, delay, t: 0.0, ease, on_update, on_complete, looping, pingpong, active: true })
            }
            ("FlxPoint", "get" | "weak") => flx_point(arg(&a, 0).f(), arg(&a, 1).f()),
            ("FlxTween", "cancelTweensOf") => {
                let target = arg(&a, 0);
                for t in &mut self.state.tweens {
                    if t.target.equals(&target) {
                        t.active = false;
                    }
                }
                Value::Null
            }
            ("FlxTween", "globalManager") => Value::Null,
            ("FlxAngle", "asRadians") => Value::Float(arg(&a, 0).f().to_radians()),
            ("FlxAngle", "asDegrees") => Value::Float(arg(&a, 0).f().to_degrees()),
            ("FlxMath", "lerp") => Value::Float(arg(&a, 0).f() + (arg(&a, 1).f() - arg(&a, 0).f()) * arg(&a, 2).f()),
            ("FlxMath", "bound") => {
                let v = arg(&a, 0).f();
                let lo = if arg(&a, 1).is_null() { f64::MIN } else { arg(&a, 1).f() };
                let hi = if arg(&a, 2).is_null() { f64::MAX } else { arg(&a, 2).f() };
                Value::Float(v.clamp(lo, hi.max(lo)))
            }
            ("FlxMath", "remapToRange") => {
                let (v, a1, b1, a2, b2) = (arg(&a, 0).f(), arg(&a, 1).f(), arg(&a, 2).f(), arg(&a, 3).f(), arg(&a, 4).f());
                Value::Float(a2 + (v - a1) * (b2 - a2) / (b1 - a1))
            }
            ("FlxColor", f) => match self.flxcolor_fn(f) {
                Some(func) => return interp.call(self, &func, a).map(Some),
                None => return Ok(None),
            },
            _ => return Ok(None),
        }))
    }

    /// `FunkinVideoSprite` methods: `load(path)`, `play()`, `pause()`, `resume()`, `stop()`.
    fn video_call(&mut self, id: u64, method: &str, a: &[Value]) -> Value {
        let audio = self.audio;
        match method {
            "load" => {
                let key = arg(a, 0).to_string();
                let key = key.rsplit(':').next().unwrap_or(&key).to_owned();
                let file = if key.contains('.') { key.clone() } else { format!("{key}.mp4") };
                let file = file.trim_start_matches("assets/").trim_start_matches("videos/").trim_start_matches("videos/").to_owned();
                let path = self.assets.root().join("videos").join("videos").join(&file);
                let opened = funkin_core::vfs::read(&path).map_err(|e| e.to_string()).and_then(crate::video::Video::open);
                let Some(v) = self.state.videos.get_mut(&id) else { return Value::Bool(false) };
                if let Some(old) = &mut v.video {
                    old.stop(audio);
                }
                match opened {
                    Ok(vid) => {
                        v.video = Some(vid);
                        v.formatted = false;
                        Value::Bool(true)
                    }
                    Err(e) => {
                        log::warn!("video {file}: {e}");
                        v.video = None;
                        self.state.video_errors.push(id);
                        Value::Bool(false)
                    }
                }
            }
            _ => {
                let Some(v) = self.state.videos.get_mut(&id) else { return Value::Null };
                match method {
                    "play" => {
                        v.playing = v.video.is_some();
                        v.paused = false;
                        Value::Bool(v.playing)
                    }
                    "pause" | "resume" | "togglePaused" => {
                        v.paused = match method {
                            "pause" => true,
                            "resume" => false,
                            _ => !v.paused,
                        };
                        let p = v.paused;
                        if let Some(vid) = &mut v.video {
                            vid.set_paused(audio, p);
                        }
                        Value::Null
                    }
                    _ => {
                        // stop / destroy
                        if let Some(vid) = &mut v.video {
                            vid.stop(audio);
                        }
                        v.video = None;
                        v.playing = false;
                        if method == "destroy" {
                            self.state.videos.remove(&id);
                        }
                        Value::Null
                    }
                }
            }
        }
    }

    /// A strumline child group's members.
    fn sgroup_members(&self, side: usize, part: usize) -> Vec<Value> {
        let s = &self.state.strum[side];
        match part {
            SG_STRUMS => (0..4).map(|l| Value::Host(HostRef { kind: K_RECEPTOR, id: (side as u64) << 8 | l })).collect(),
            SG_NOTES => s.notes.iter().map(|n| note_ref(side, n.idx)).collect(),
            SG_HOLDS => s.holds.iter().map(|n| Value::Host(HostRef { kind: K_HOLD, id: (side as u64) << 32 | n.idx as u64 })).collect(),
            _ => vec![],
        }
    }

    /// A note's or strumline's `scale`.
    fn nscale(&self, id: u64) -> [f32; 2] {
        if id & SCALE_LINE != 0 {
            return self.state.strum[(id & 1) as usize].scale.unwrap_or([0.7, 0.7]);
        }
        let s = &self.state.strum[(id >> 32) as usize & 1];
        s.note_scale.get(&((id & 0xFFFF_FFFF) as usize)).copied().unwrap_or([0.7, 0.7])
    }

    fn set_nscale(&mut self, id: u64, p: [f32; 2]) {
        if id & SCALE_LINE != 0 {
            self.state.strum[(id & 1) as usize].scale = Some(p);
            return;
        }
        let s = &mut self.state.strum[(id >> 32) as usize & 1];
        s.note_scale.insert((id & 0xFFFF_FFFF) as usize, p);
    }

    /// The fill of `FlxBar` `id` from its value and range.
    fn sync_bar(&mut self, id: u64) {
        let Some(b) = self.state.bars.get(&id) else { return };
        let span = b.max - b.min;
        let frac = if span.abs() < 1e-9 { 0.0 } else { ((b.value - b.min) / span).clamp(0.0, 1.0) };
        if let Some(bar) = self.actor(id).and_then(|a| a.bar_mut()) {
            bar.frac = frac as f32;
        }
    }

    fn sprite_get(&mut self, id: u64, field: &str) -> Option<Value> {
        if field == "bitmap" && self.state.videos.contains_key(&id) {
            return Some(Value::Host(HostRef { kind: K_VBITMAP, id }));
        }
        if let Some(b) = self.state.bars.get(&id) {
            match field {
                "value" => return Some(Value::Float(b.value)),
                "min" => return Some(Value::Float(b.min)),
                "max" => return Some(Value::Float(b.max)),
                "range" => return Some(Value::Float(b.max - b.min)),
                "percent" => {
                    let span = b.max - b.min;
                    return Some(Value::Float(if span.abs() < 1e-9 { 0.0 } else { (b.value - b.min) / span * 100.0 }));
                }
                "parent" => return Some(b.parent.clone()),
                "parentVariable" => return Some(Value::str(&b.variable)),
                "fillDirection" => return Some(Value::str(&self.actor(id).and_then(|a| a.bar_mut()).map(|b| b.dir.clone()).unwrap_or_default())),
                _ => {}
            }
        }
        if let Some(v) = self.state.sprite_extra.get(&(id, field.to_string())) {
            return Some(v.clone());
        }
        if field == "ignoreExclusionPref" {
            // Starts empty; scripts push onto it.
            let v = Value::array(vec![]);
            self.state.sprite_extra.insert((id, field.to_string()), v.clone());
            return Some(v);
        }
        // `anim`: flixel-animate's controller on FunkinSprite (the same animations).
        if field == "animation" || field == "anim" {
            return Some(Value::Host(HostRef { kind: K_ANIM, id }));
        }
        if field == "scale" || field == "scrollFactor" || field == "offset" || field == "velocity" {
            let which = match field {
                "scale" => 0,
                "scrollFactor" => 1,
                "offset" => 2,
                _ => 3,
            };
            return Some(Value::Host(HostRef { kind: K_POINT, id: id | (which << 40) }));
        }
        if field == "superClass" {
            // A scripted class's engine object (`this.superClass`).
            return Some(Self::sprite_ref(id));
        }
        if field == "transformMatrix" {
            return Some(Value::Host(HostRef { kind: K_MATRIX, id }));
        }
        if field == "currentStage" {
            return Some(self.current_stage());
        }
        if field == "cameraFocusPoint" && id & CHAR != 0 {
            self.char_focus(id)?;
            return Some(Value::Host(HostRef { kind: K_CFOCUS, id }));
        }
        if field == "originalPosition" && id & CHAR != 0 {
            return Some(Value::Host(HostRef { kind: K_ORIGPOS, id }));
        }
        if field == "debug" {
            return Some(Value::Bool(false));
        }
        if field == "library" {
            // `library.getSymbol(name).timeline.getLayer(i).forEachFrame(f -> f.forEachElement(e -> e.visible = false))`:
            // the element hides the symbol's art when it's made invisible.
            let get_symbol = Value::native(move |_, _, _, a| {
                let name = arg(&a, 0).to_string();
                let el = Value::object(vec![("visible", Value::Bool(true)), ("__sprite", Value::Int(id as i64)), ("__symbol", Value::str(&name))]);
                let for_each_el = {
                    let el = el.clone();
                    Value::native(move |interp, host, _, a| {
                        interp.call(host, &arg(&a, 0), vec![el.clone()])?;
                        if el.field("visible").is_some_and(|v| !v.truthy()) {
                            host.call(interp, HostRef { kind: K_SPRITE, id }, "__hide_symbol", vec![el.field("__symbol").unwrap_or(Value::Null)])?;
                        }
                        Ok(Value::Null)
                    })
                };
                let frame = Value::object(vec![("forEachElement", for_each_el)]);
                let layer = Value::object(vec![(
                    "forEachFrame",
                    Value::native(move |interp, host, _, a| {
                        interp.call(host, &arg(&a, 0), vec![frame.clone()])?;
                        Ok(Value::Null)
                    }),
                )]);
                let timeline = Value::object(vec![("getLayer", Value::native(move |_, _, _, _| Ok(layer.clone())))]);
                Ok(Value::object(vec![("timeline", timeline)]))
            });
            return Some(Value::object(vec![("getSymbol", get_symbol)]));
        }
        if field == "_data" && id & CHAR != 0 {
            // The character's data file, as scripts read it (`_data.animations`).
            let cid = self.world.as_deref().and_then(|w| w.chars.get((id & 0xFF) as usize)?.as_ref().map(|c| c.id.clone()))?;
            let path = self.assets.root().join("data").join("characters").join(format!("{cid}.json"));
            let text = funkin_core::vfs::read_to_string(&path).ok()?;
            let j: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
            return Some(json_to_value(&j));
        }
        if id & CHAR != 0 {
            let slot = (id & 0xFF) as usize;
            match field {
                "holdTimer" => return Some(Value::Float(self.world.as_deref()?.chars.get(slot)?.as_ref()?.hold_timer)),
                "characterType" => {
                    let kind = self.world.as_deref()?.chars.get(slot)?.as_ref()?.kind;
                    return Some(Value::str(char_type_name(Some(kind))));
                }
                _ => {}
            }
        }
        if field == "characterType" {
            return Some(Value::str(char_type_name(self.world.as_deref().and_then(|w| w.singer_kind(id as usize)))));
        }
        let char_id = self.world.as_deref().and_then(|w| if id & CHAR != 0 { w.chars.get((id & 0xFF) as usize)?.as_ref().map(|c| c.id.clone()) } else { None });
        let a = self.actor(id)?;
        Some(match field {
            "x" => Value::Float(a.x as f64),
            "y" => Value::Float(a.y as f64),
            "alpha" => Value::Float(a.alpha as f64),
            "visible" => Value::Bool(a.visible),
            "angle" => Value::Float(a.angle as f64),
            "flipX" => Value::Bool(a.flip_x),
            "zIndex" => Value::Int(a.z_index as i64),
            "color" => Value::Int(color_to_int([a.color[0], a.color[1], a.color[2], 1.0])),
            "width" => Value::Float(a.size()[0] as f64),
            "height" => Value::Float(a.size()[1] as f64),
            "globalOffsets" => Value::array(vec![Value::Float(a.global_offsets[0] as f64), Value::Float(a.global_offsets[1] as f64)]),
            // BaseCharacter.characterOrigin: the feet, relative to the sprite.
            "characterOrigin" => Value::object(vec![("x", Value::Float(a.size()[0] as f64 / 2.0)), ("y", Value::Float(a.size()[1] as f64))]),
            "frameWidth" => Value::Float(a.frame_size()[0] as f64),
            "frameHeight" => Value::Float(a.frame_size()[1] as f64),
            "name" if id & CHAR != 0 => Value::str(["bf", "dad", "gf"][(id & 0xFF) as usize % 3]),
            "name" => Value::str(&a.name),
            "characterId" => Value::from(char_id.unwrap_or_default()),
            "danceEvery" => Value::Float(a.dance_every),
            "idleSuffix" => Value::str(&a.idle_suffix),
            "antialiasing" | "active" | "exists" | "alive" => Value::Bool(true),
            "pixels" | "graphic" if a.image_texture().is_some() => Value::Host(HostRef { kind: K_BITMAP, id: a.image_texture().unwrap().index() as u64 }),
            "scrollX" if a.tile.is_some() => Value::Float(a.tile.unwrap().scroll[0] as f64),
            "scrollY" if a.tile.is_some() => Value::Float(a.tile.unwrap().scroll[1] as f64),
            "shader" => a.shader.as_ref().map(|s| Value::Host(HostRef { kind: K_SHADER, id: s.borrow().id })).unwrap_or(Value::Null),
            "blend" => match a.blend {
                crate::gfx::Blend::Add => Value::Int(0),
                crate::gfx::Blend::Multiply => Value::Int(9),
                crate::gfx::Blend::Normal => Value::Int(10),
                crate::gfx::Blend::Screen => Value::Int(12),
            },
            _ => return None,
        })
    }

    fn sprite_set(&mut self, id: u64, field: &str, v: &Value) -> bool {
        if let Some(b) = self.state.bars.get_mut(&id) {
            let done = match field {
                "value" => {
                    b.value = v.f().clamp(b.min.min(b.max), b.max.max(b.min));
                    true
                }
                "percent" => {
                    b.value = b.min + (b.max - b.min) * v.f() / 100.0;
                    true
                }
                "parent" => {
                    b.parent = v.clone();
                    true
                }
                "parentVariable" => {
                    b.variable = v.to_string();
                    true
                }
                "fillDirection" => {
                    let d = v.to_string();
                    if let Some(bar) = self.actor(id).and_then(|a| a.bar_mut()) {
                        bar.dir = d;
                    }
                    true
                }
                _ => false,
            };
            if done {
                self.sync_bar(id);
                return true;
            }
        }
        if field == "ignoreExclusionPref" {
            self.state.sprite_extra.insert((id, field.to_string()), v.clone());
            return true;
        }
        if field == "frames" {
            if let Value::Str(key) = v {
                if self.load_into(id, "sparrow", key).is_err() {
                    log::warn!("script: could not load sparrow {key}");
                }
            }
            return true;
        }
        if field == "holdTimer" && id & CHAR != 0 {
            if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((id & 0xFF) as usize)?.as_mut()) {
                c.hold_timer = v.f();
            }
            return true;
        }
        if field == "matrixExposed" {
            if let Some(a) = self.actor(id) {
                a.transform = v.truthy().then_some(a.transform.unwrap_or(crate::gfx::Affine::IDENTITY));
            }
            return true;
        }
        if field == "characterType" {
            // A fetched character (a prop) sings the side it's given.
            let kind = match v.to_string().as_str() {
                "bf" => crate::world::CharType::Bf,
                "dad" => crate::world::CharType::Dad,
                _ => crate::world::CharType::Gf,
            };
            if id & CHAR != 0 {
                if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((id & 0xFF) as usize)?.as_mut()) {
                    c.kind = kind;
                }
            } else {
                let root = self.assets.root.clone();
                let name = self.actor(id).map(|a| a.name.clone()).unwrap_or_default();
                let sing = funkin_core::stage::load_character(&root, &name).map(|d| d.sing_time).unwrap_or(8.0);
                if let Some(w) = self.world.as_deref_mut() {
                    w.set_singer(id as usize, kind, sing);
                }
            }
            return true;
        }
        if field == "shader" {
            let s = self.shader_of(v);
            if let Some(a) = self.actor(id) {
                a.shader = s;
            }
            return true;
        }
        let reorder = field == "zIndex";
        let Some(a) = self.actor(id) else { return false };
        match field {
            "scrollX" | "scrollY" if a.tile.is_some() => {
                if let Some(t) = &mut a.tile {
                    t.scroll[(field == "scrollY") as usize] = v.f() as f32;
                }
            }
            "blend" => {
                // OpenFL BlendMode: ADD = 0, MULTIPLY = 9, NORMAL = 10 (or their names).
                let name = match v {
                    Value::Str(s) => s.to_lowercase(),
                    Value::Null => "normal".into(),
                    v => match v.i() {
                        0 => "add".into(),
                        9 => "multiply".into(),
                        12 => "screen".into(),
                        _ => "normal".into(),
                    },
                };
                a.blend = match name.as_str() {
                    "add" => crate::gfx::Blend::Add,
                    "multiply" => crate::gfx::Blend::Multiply,
                    "screen" => crate::gfx::Blend::Screen,
                    _ => crate::gfx::Blend::Normal,
                };
            }
            "x" => a.x = v.f() as f32,
            "y" => a.y = v.f() as f32,
            "alpha" => a.alpha = v.f().clamp(0.0, 1.0) as f32,
            "visible" => a.visible = v.truthy(),
            "angle" => a.angle = v.f() as f32,
            "flipX" => a.flip_x = v.truthy(),
            "zIndex" => a.z_index = v.i() as i32,
            // The port draws every prop in stage space already.
            "applyStageMatrix" | "shouldBop" => {}
            "color" => {
                let c = int_to_color(v.i());
                a.color = [c[0], c[1], c[2], a.color[3]];
            }
            "danceEvery" => a.dance_every = v.f(),
            "cameras" | "camera" => {
                // The first camera listed draws it (a sprite on several is rare).
                let cam = match v {
                    Value::Array(cams) => cams.borrow().iter().find_map(|c| match c {
                        Value::Host(HostRef { kind: K_CAMERA, id }) => Some(*id),
                        _ => None,
                    }),
                    Value::Host(HostRef { kind: K_CAMERA, id }) => Some(*id),
                    _ => None,
                }
                .unwrap_or(0);
                let i = id as usize;
                if id & CHAR == 0 {
                    if let Some(w) = self.world.as_deref_mut() {
                        w.screen.retain(|&(j, _)| j != i);
                        if cam != 0 {
                            w.screen.push((i, cam));
                        }
                    }
                }
            }
            // openfl ColorTransform: the multipliers tint the sprite.
            "colorTransform" => {
                let m = |n: &str| v.field(n).map(|x| x.f() as f32).unwrap_or(1.0);
                let o = |n: &str| v.field(n).map(|x| x.f() as f32 / 255.0).unwrap_or(0.0);
                a.color = [m("redMultiplier"), m("greenMultiplier"), m("blueMultiplier"), a.color[3]];
                a.color_offset = [o("redOffset"), o("greenOffset"), o("blueOffset"), o("alphaOffset")];
            }
            "useColorTransform" => {}
            "idleSuffix" => a.idle_suffix = if v.is_null() { String::new() } else { v.to_string() },
            "antialiasing" | "active" | "useRenderTexture" | "flipY" | "pixelPerfectRender" | "pixelPerfectPosition" | "moves" | "immovable" | "solid" | "origin" | "clipRect" => {}
            _ => return false,
        }
        if reorder {
            if let Some(w) = self.world.as_deref_mut() {
                w.refresh();
            }
        }
        true
    }

    fn sprite_call(&mut self, interp: &mut Interp, id: u64, method: &str, a: &[Value]) -> Option<Value> {
        match method {
            "load" | "play" | "pause" | "resume" | "stop" | "destroy" | "togglePaused" if self.state.videos.contains_key(&id) => {
                let ok = self.video_call(id, method, a);
                if method != "destroy" {
                    return Some(ok);
                }
                self.actor(id)?.visible = false;
                return Some(Value::Null);
            }
            // setColorTransform(rMul, gMul, bMul, aMul, rOff, gOff, bOff, aOff)
            "setColorTransform" => {
                let g = |i: usize, d: f64| if a.len() > i && !arg(a, i).is_null() { arg(a, i).f() as f32 } else { d as f32 };
                let act = self.actor(id)?;
                act.color = [g(0, 1.0), g(1, 1.0), g(2, 1.0), act.color[3]];
                act.color_offset = [g(4, 0.0) / 255.0, g(5, 0.0) / 255.0, g(6, 0.0) / 255.0, g(7, 0.0) / 255.0];
                return Some(Value::Null);
            }
            // The main timeline (addBySymbol falls back to it for unknown names).
            "getDefaultSymbol" => return Some(Value::str("")),
            "getFramesWithKeyword" => {
                // One stand-in frame: `frame.add(element)` puts the element's sprite in
                // every symbol named with the keyword.
                // `frame.elements` holds a stand-in for the placeholder art: a script that
                // clears it (`splice`) hides the symbol's own art.
                let kw = arg(a, 0).to_string();
                let marker = Value::object(vec![("placeholder", Value::Bool(true))]);
                let elements = Value::array(vec![marker]);
                let els = elements.clone();
                let add = Value::native(move |interp, host, _, args| {
                    let cleared = match &els {
                        Value::Array(v) => !v.borrow().iter().any(|e| e.field("placeholder").is_some()),
                        _ => false,
                    };
                    host.call(interp, HostRef { kind: K_SPRITE, id }, "__inject", vec![Value::str(&kw), args.first().cloned().unwrap_or(Value::Null), Value::Bool(cleared)])?;
                    Ok(Value::Null)
                });
                return Some(Value::array(vec![Value::object(vec![("add", add), ("elements", elements)])]));
            }
            "__hide_symbol" => {
                let name = arg(a, 0).to_string();
                if let Some(act) = self.actor(id) {
                    act.hide_symbols(&name, true);
                }
                return Some(Value::Null);
            }
            "__inject" => {
                let kw = arg(a, 0).to_string();
                let inner = arg(a, 1).field("sprite").and_then(|v| Self::sprite_id(&v));
                let cleared = arg(a, 2).truthy();
                if let (Some(inner), Some(act)) = (inner, self.actor(id)) {
                    if inner & CHAR == 0 {
                        if cleared {
                            act.hide_symbols(&kw, false);
                        }
                        act.mouth.retain(|(k, _)| *k != kw);
                        act.mouth.push((kw, inner as usize));
                        if let Some(a) = self.actor(inner) {
                            a.nested = true;
                        }
                    }
                }
                return Some(Value::Null);
            }
            // `super.set_alpha(val)` from a script's setter override.
            m if m.starts_with("set_") && a.len() == 1 => {
                self.sprite_set(id, &m[4..], &a[0]);
                return Some(a[0].clone());
            }
            "loadGraphic" | "loadTexture" | "loadSparrow" | "loadTextureAtlas" | "loadFrames" => {
                let kind = match method {
                    "loadSparrow" | "loadFrames" => "sparrow",
                    "loadTextureAtlas" => "animateatlas",
                    _ => "image",
                };
                let key = arg(a, 0);
                if !key.is_null() {
                    let mut key = key.to_string();
                    if let Value::Str(lib) = arg(a, 1) {
                        if !key.contains(':') {
                            key = format!("{lib}:{key}");
                        }
                    }
                    self.load_logged(interp, id, kind, &key);
                    if kind == "animateatlas" && arg(a, 2).field("swfMode").is_some_and(|v| v.truthy()) {
                        self.actor(id)?.set_swf_mode(true);
                    }
                }
                return Some(Self::sprite_ref(id));
            }
            // FlxBar
            "createFilledBar" | "createColoredEmptyBar" | "createColoredFilledBar" | "createGradientBar" | "createGradientEmptyBar"
            | "createGradientFilledBar"
                if self.state.bars.contains_key(&id) =>
            {
                let stops = |v: Value| -> Vec<crate::gfx::Color> {
                    match v {
                        Value::Array(a) => a.borrow().iter().map(|c| int_to_color(c.i())).collect(),
                        Value::Null => vec![],
                        c => vec![int_to_color(c.i())],
                    }
                };
                let (empty, fill, rot) = match method {
                    "createFilledBar" => (stops(arg(a, 0)), stops(arg(a, 1)), None),
                    "createColoredEmptyBar" => (stops(arg(a, 0)), vec![], None),
                    "createColoredFilledBar" => (vec![], stops(arg(a, 0)), None),
                    // (empty, fill, chunkSize, rotation)
                    "createGradientBar" => (stops(arg(a, 0)), stops(arg(a, 1)), Some(arg(a, 3))),
                    // (colors, chunkSize, rotation)
                    "createGradientEmptyBar" => (stops(arg(a, 0)), vec![], Some(arg(a, 2))),
                    _ => (vec![], stops(arg(a, 0)), Some(arg(a, 2))),
                };
                let bar = self.actor(id)?.bar_mut()?;
                if !empty.is_empty() {
                    bar.empty = empty;
                }
                if !fill.is_empty() {
                    bar.fill = fill;
                }
                bar.rotation = match rot {
                    Some(Value::Null) | None => 180.0,
                    Some(r) => r.f() as f32,
                };
                self.sync_bar(id);
                return Some(Self::sprite_ref(id));
            }
            "setRange" if self.state.bars.contains_key(&id) => {
                let b = self.state.bars.get_mut(&id)?;
                (b.min, b.max) = (arg(a, 0).f(), arg(a, 1).f());
                b.value = b.value.clamp(b.min.min(b.max), b.max.max(b.min));
                self.sync_bar(id);
                return Some(Value::Null);
            }
            "setParent" if self.state.bars.contains_key(&id) => {
                let b = self.state.bars.get_mut(&id)?;
                b.parent = arg(a, 0);
                b.variable = arg(a, 1).to_string();
                return Some(Value::Null);
            }
            "updateBar" | "updateEmptyBar" | "updateFilledBar" if self.state.bars.contains_key(&id) => {
                self.sync_bar(id);
                return Some(Value::Null);
            }
            "makeGraphic" | "makeSolidColor" => {
                let (w, h) = (arg(a, 0).f() as f32, arg(a, 1).f() as f32);
                let c = if a.len() > 2 { int_to_color(arg(a, 2).i()) } else { [1.0; 4] };
                self.actor(id)?.take_graphics(Actor::solid("", c, w.max(1.0), h.max(1.0)));
                return Some(Self::sprite_ref(id));
            }
            "screenCenter" => {
                let act = self.actor(id)?;
                let [w, h] = act.size();
                let axes = arg(a, 0).to_string();
                if !axes.ends_with('Y') || axes.ends_with("XY") || axes == "null" {
                    act.x = (1280.0 - w) / 2.0;
                }
                if !axes.ends_with('X') || axes.ends_with("XY") || axes == "null" {
                    act.y = (720.0 - h) / 2.0;
                }
                return Some(Self::sprite_ref(id));
            }
            // `super.onNoteHit(event)` from a character script: the engine's sing.
            "onNoteHit" | "onNoteMiss" | "onNoteGhostMiss" if id & CHAR != 0 => {
                let ev = arg(a, 0);
                if !ev.field("eventCanceled").is_some_and(|v| v.truthy()) {
                    let must = ev.field("mustHit").is_none_or(|v| v.truthy());
                    let dir = funkin_core::play::NoteDirection::from_index(ev.field("dir").map_or(0, |v| v.i()).clamp(0, 3) as usize).unwrap_or(funkin_core::play::NoteDirection::Left);
                    let kind = match ev.field("kind") {
                        Some(Value::Str(s)) => s.to_string(),
                        _ => String::new(),
                    };
                    if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((id & 0xFF) as usize)?.as_mut()) {
                        c.on_note(method == "onNoteHit", must, dir, &kind);
                    }
                }
                return Some(Value::Null);
            }
            "playSingAnimation" if id & CHAR != 0 => {
                let dir = funkin_core::play::NoteDirection::from_index(arg(a, 0).i().clamp(0, 3) as usize).unwrap_or(funkin_core::play::NoteDirection::Left);
                let suffix = match arg(a, 2) {
                    Value::Str(s) => s.to_string(),
                    _ => String::new(),
                };
                if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((id & 0xFF) as usize)?.as_mut()) {
                    c.sing_suffix(dir, arg(a, 1).truthy(), &suffix);
                }
                return Some(Value::Null);
            }
            m if m.starts_with("on") && m.len() > 2 && m.as_bytes()[2].is_ascii_uppercase() => return Some(Value::Null),
            "update" | "draw" | "centerOffsets" | "centerOrigin" | "resetFrameSize" | "setFrameSize" => return Some(Value::Null),
            "addOffset" | "setAnimationOffsets" => {
                let name = arg(a, 0).to_string();
                let (x, y) = match arg(a, 1) {
                    Value::Array(v) => {
                        let v = v.borrow();
                        (v.first().map(|v| v.f()).unwrap_or(0.0), v.get(1).map(|v| v.f()).unwrap_or(0.0))
                    }
                    x => (x.f(), arg(a, 2).f()),
                };
                self.actor(id)?.set_anim_offsets(&name, x as f32, y as f32);
                return Some(Value::Null);
            }
            "getGraphicMidpoint" | "getMidpoint" => {
                let act = self.actor(id)?;
                let [w, h] = act.size();
                let (x, y) = (act.x + w / 2.0, act.y + h / 2.0);
                return Some(Value::object(vec![("x", Value::Float(x as f64)), ("y", Value::Float(y as f64))]));
            }
            "setPosition" => {
                let act = self.actor(id)?;
                act.x = arg(a, 0).f() as f32;
                act.y = arg(a, 1).f() as f32;
                return Some(Value::Null);
            }
            "setGraphicSize" => {
                let act = self.actor(id)?;
                let [w, h] = act.frame_size();
                let tw = arg(a, 0).f() as f32;
                let th = arg(a, 1).f() as f32;
                let sx = if tw > 0.0 { tw / w } else { 0.0 };
                let sy = if th > 0.0 { th / h } else { sx };
                act.scale = [if tw > 0.0 { sx } else { sy }, sy];
                return Some(Value::Null);
            }
            "updateHitbox" | "kill" | "revive" | "destroy" => {
                match method {
                    // A destroyed sprite no longer draws (it stays in its group, inert).
                    "kill" | "destroy" => self.actor(id)?.visible = false,
                    // Flixel recentres `offset` for the current scale.
                    "updateHitbox" => self.actor(id)?.offset = None,
                    _ => {}
                }
                return Some(Value::Null);
            }
            _ => {}
        }
        let a0 = arg(a, 0).to_string();
        let act = self.actor(id)?;
        Some(match method {
            "hasAnimation" => Value::Bool(act.has_anim(&a0)),
            "playAnimation" => {
                act.play(&a0, arg(a, 1).truthy(), arg(a, 2).truthy());
                Value::Null
            }
            "getCurrentAnimation" => Value::str(act.current()),
            // Elements are handed to scripts as their symbol's name.
            "getFirstElement" => match act.atlas_mut() {
                Some(at) if at.has_first_element(&a0) => Value::str(&a0),
                _ => Value::Null,
            },
            "scaleElement" => {
                let (scale, off, all) = (arg(a, 1).f() as f32, arg(a, 2).f() as f32, arg(a, 3).truthy());
                if let Some(at) = act.atlas_mut() {
                    at.scale_first_element(&a0, scale, off, all);
                }
                Value::Null
            }
            "isAnimationFinished" => Value::Bool(act.finished()),
            "resetPosition" if id & CHAR != 0 => {
                // Bopper.resetPosition; BaseCharacter's x/y setters move the camera focus along.
                let slot = (id & 0xFF) as usize;
                if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars[slot].as_mut()) {
                    let [dx, dy] = [c.original[0] - c.actor.x, c.original[1] - c.actor.y];
                    c.actor.x = c.original[0];
                    c.actor.y = c.original[1];
                    c.focus[0] += dx as f64;
                    c.focus[1] += dy as f64;
                }
                Value::Null
            }
            "dance" => {
                let force = arg(a, 0).truthy();
                if id & CHAR != 0 {
                    let slot = (id & 0xFF) as usize;
                    if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars[slot].as_mut()) {
                        c.dance(force);
                    }
                } else {
                    act.bop(force);
                }
                Value::Null
            }
            _ => {
                let _ = interp;
                return None;
            }
        })
    }

    fn anim_get(&mut self, id: u64, field: &str) -> Option<Value> {
        let a = self.actor(id)?;
        Some(match field {
            "name" => {
                let c = a.current();
                if c.is_empty() {
                    Value::Null
                } else {
                    Value::str(c)
                }
            }
            "finished" => Value::Bool(a.finished()),
            "curAnim" => {
                if a.current().is_empty() {
                    Value::Null
                } else {
                    Value::Host(HostRef { kind: K_CURANIM, id })
                }
            }
            "frameIndex" | "curFrame" => Value::Int(a.frame_number() as i64),
            "paused" => Value::Bool(false),
            "isPlaying" => Value::Bool(!a.current().is_empty() && !a.finished()),
            "onFinish" => Value::Host(HostRef { kind: K_SIGNAL, id }),
            "onFrameChange" => Value::Host(HostRef { kind: K_SIGNAL, id: id | (1 << 40) }),
            "onFrameLabel" => Value::Host(HostRef { kind: K_SIGNAL, id: id | (2 << 40) }),
            "finishCallback" | "callback" => Value::Null,
            _ => return None,
        })
    }

    fn anim_call(&mut self, id: u64, method: &str, a: &[Value]) -> Option<Value> {
        let act = self.actor(id)?;
        Some(match method {
            "play" => {
                let name = arg(a, 0).to_string();
                if (name.is_empty() || arg(a, 0).is_null()) && act.is_animate() {
                    // FlxAnimate with no symbol: the main timeline.
                    act.play_timeline(0, None);
                } else {
                    act.play(&name, arg(a, 1).truthy(), false);
                    // play(name, force, reversed, frame)
                    if a.len() > 3 && arg(a, 3).i() > 0 {
                        act.set_frame_number(arg(a, 3).i() as usize);
                    }
                }
                Value::Null
            }
            "exists" => Value::Bool(act.has_anim(&arg(a, 0).to_string())),
            "getByName" => {
                if act.has_anim(&arg(a, 0).to_string()) {
                    Value::object(vec![("name", arg(a, 0))])
                } else {
                    Value::Null
                }
            }
            "stop" | "pause" | "resume" | "finish" | "reset" => Value::Null,
            "addByPrefix" => {
                let fps = if a.len() > 2 { arg(a, 2).f() } else { 30.0 } as f32;
                let looped = if a.len() > 3 { arg(a, 3).truthy() } else { true };
                if !act.add_sparrow_anim(&arg(a, 0).to_string(), &arg(a, 1).to_string(), &[], fps, looped, arg(a, 4).truthy()) {
                    // A texture atlas: the prefix names a frame label, else a symbol.
                    for ty in ["framelabel", "symbol"] {
                        let d = serde_json::json!({"name": arg(a, 0).to_string(), "prefix": arg(a, 1).to_string(), "frameRate": fps, "looped": looped, "animType": ty});
                        if let Ok(d) = serde_json::from_value::<funkin_core::stage::AnimationData>(d) {
                            act.add_atlas_anims(&[d]);
                        }
                        if act.has_anim(&arg(a, 0).to_string()) {
                            break;
                        }
                    }
                }
                Value::Null
            }
            "addByFrameLabel" | "addByFrameLabelIndices" | "addBySymbolIndices" => {
                // (name, label, [indices], fps, looped) — indices only for the *Indices forms.
                let with_idx = method != "addByFrameLabel";
                let o = if with_idx { 1 } else { 0 };
                let idx: Vec<serde_json::Value> = match arg(a, 2) {
                    Value::Array(v) if with_idx => v.borrow().iter().map(|v| serde_json::Value::from(v.i())).collect(),
                    _ => vec![],
                };
                let fps = if a.len() > 2 + o { arg(a, 2 + o).f() } else { 24.0 };
                let looped = a.len() > 3 + o && arg(a, 3 + o).truthy();
                let d = serde_json::json!({
                    "name": arg(a, 0).to_string(), "prefix": arg(a, 1).to_string(), "frameIndices": idx,
                    "frameRate": fps, "looped": looped, "animType": if method == "addBySymbolIndices" { "symbol" } else { "framelabel" },
                });
                if let Ok(d) = serde_json::from_value::<funkin_core::stage::AnimationData>(d) {
                    act.add_atlas_anims(&[d]);
                }
                Value::Null
            }
            "addBySymbol" => {
                let fps = if a.len() > 2 { arg(a, 2).f() } else { 24.0 } as f32;
                let looped = a.len() > 3 && arg(a, 3).truthy();
                act.add_symbol_anim(&arg(a, 0).to_string(), &arg(a, 1).to_string(), fps, looped);
                Value::Null
            }
            "addByIndices" => {
                let idx: Vec<i32> = match arg(a, 2) {
                    Value::Array(v) => v.borrow().iter().map(|v| v.i() as i32).collect(),
                    _ => vec![],
                };
                let fps = if a.len() > 4 { arg(a, 4).f() } else { 30.0 } as f32;
                let looped = if a.len() > 5 { arg(a, 5).truthy() } else { true };
                act.add_sparrow_anim(&arg(a, 0).to_string(), &arg(a, 1).to_string(), &idx, fps, looped, arg(a, 6).truthy());
                Value::Null
            }
            _ => return None,
        })
    }

    fn stage_call(&mut self, method: &str, a: &[Value]) -> Option<Value> {
        match method {
            "add" | "addProp" => {
                self.add_obj(&arg(a, 0), true);
                return Some(arg(a, 0));
            }
            "insert" => {
                self.add_obj(&arg(a, 1), true);
                return Some(arg(a, 1));
            }
            "remove" | "removeProp" => {
                self.add_obj(&arg(a, 0), false);
                return Some(arg(a, 0));
            }
            _ => {}
        }
        let w = self.world.as_deref_mut();
        Some(match method {
            "getNamedProp" => {
                let name = arg(a, 0).to_string();
                match w.and_then(|w| w.props.iter().position(|p| p.name == name)) {
                    Some(i) => Self::sprite_ref(i as u64),
                    None => Value::Null,
                }
            }
            "getBoyfriendPosition" | "getDadPosition" | "getGirlfriendPosition" => {
                // The stage data's position for that slot (the feet), as a new point.
                let p = w.and_then(|w| {
                    let c = &w.stage.characters;
                    match method {
                        "getBoyfriendPosition" => c.bf.as_ref(),
                        "getDadPosition" => c.dad.as_ref(),
                        _ => c.gf.as_ref(),
                    }
                    .map(|s| s.position)
                });
                let [x, y] = p.unwrap_or([0.0; 2]);
                Value::object(vec![("x", Value::Float(x)), ("y", Value::Float(y))])
            }
            "getBoyfriend" | "getDad" | "getGirlfriend" => {
                let slot = match method {
                    "getBoyfriend" => 0,
                    "getDad" => 1,
                    _ => 2,
                };
                match w.and_then(|w| w.chars[slot].as_mut()) {
                    Some(c) => {
                        // `getGirlfriend(pop = true)`: the character comes off the stage.
                        if arg(a, 0).truthy() {
                            c.actor.visible = false;
                        }
                        Self::sprite_ref(CHAR | slot as u64)
                    }
                    None => Value::Null,
                }
            }
            "refresh" => {
                if let Some(w) = w {
                    w.refresh();
                }
                Value::Null
            }
            "fetchAssetPaths" => Value::array(vec![]),
            "pause" | "resume" => Value::Null,
            // Engine-side stage hooks the scripts chain up to (`super.onBeatHit(event)`).
            "addCharacter" => {
                // A fetched character (a prop); the stage's own are already placed.
                self.add_obj(&arg(a, 0), true);
                Value::Null
            }
            m if m.starts_with("on") || m == "buildStage" || m == "resetStage" || m == "dispatchToCharacters" => Value::Null,
            _ => return None,
        })
    }

    /// A camera's scroll (`which` 0) or target offset (1).
    fn cam_point(&self, cam: u64, which: u64) -> Option<[f64; 2]> {
        let w = self.world.as_deref()?;
        let c = match cam {
            0 => &w.camera,
            c if c >= 3 => &w.extra_cams.get(c as usize - 3)?.cam,
            _ => return Some([0.0; 2]),
        };
        Some(if which == 0 { c.scroll } else { c.target_offset })
    }

    fn set_cam_point(&mut self, cam: u64, which: u64, p: [f64; 2]) {
        let Some(w) = self.world.as_deref_mut() else { return };
        let c = match cam {
            0 => &mut w.camera,
            c if c >= 3 => match w.extra_cams.get_mut(c as usize - 3) {
                Some(e) => &mut e.cam,
                None => return,
            },
            _ => return,
        };
        if which == 0 {
            c.scroll = p;
        } else {
            c.target_offset = p;
        }
    }

    fn playstate_get(&mut self, field: &str) -> Option<Value> {
        Some(match field {
            "currentStage" => self.current_stage(),
            "cameraFollowPoint" => Value::Host(HostRef { kind: K_FOLLOW, id: 0 }),
            "comboPopUps" => Value::Host(HostRef { kind: K_POPUPS, id: 0 }),
            "currentSong" => Value::Host(HostRef { kind: K_SONG, id: 0 }),
            "camGame" | "camHUD" | "camCutscene" => Value::Host(HostRef { kind: K_CAMERA, id: match field {
                "camGame" => 0,
                "camHUD" => 1,
                _ => 2,
            } }),
            "currentDifficulty" => Value::str(&self.state.difficulty),
            "stageZoom" | "defaultCameraZoom" => Value::Float(self.world.as_deref().map(|w| w.camera.stage_zoom).unwrap_or(1.0)),
            "currentCameraZoom" => Value::Float(self.world.as_deref().map(|w| w.camera.current_zoom).unwrap_or(1.0)),
            "currentConversation" => {
                if self.state.dialogue.is_some() {
                    Value::Host(HostRef { kind: K_CONV, id: 0 })
                } else {
                    Value::Null
                }
            }
            "currentVariation" => Value::str(&self.state.variation),
            "health" => Value::Float(self.state.health),
            "healthBarBG" => Value::Host(HostRef { kind: K_HUD, id: HUD_BAR_BG as u64 }),
            "healthBar" => Value::Host(HostRef { kind: K_HUD, id: HUD_BAR as u64 }),
            "iconP1" => Value::Host(HostRef { kind: K_HUD, id: HUD_ICON_P1 as u64 }),
            "iconP2" => Value::Host(HostRef { kind: K_HUD, id: HUD_ICON_P2 as u64 }),
            "scoreText" => Value::Host(HostRef { kind: K_HUD, id: HUD_SCORE as u64 }),
            "playerStrumline" | "opponentStrumline" => Value::Host(HostRef { kind: K_STRUM, id: (field == "opponentStrumline") as u64 }),
            "controls" => Value::Host(HostRef { kind: K_CONTROLS, id: 0 }),
            "isInCutscene" => Value::Bool(self.state.in_cutscene),
            "mayPauseGame" => Value::Bool(!self.state.no_pause),
            "isBotPlayMode" => Value::Bool(self.state.botplay),
            "isPracticeMode" => Value::Bool(self.state.practice),
            "isChartingMode" | "isMinimalMode" | "disableKeys" | "isGamePaused" | "justUnpaused" | "startingSong" => Value::Bool(false),
            _ => return None,
        })
    }
}

fn obj_random() -> Value {
    let r = funkin_script::random;
    Value::object(vec![
        (
            "int",
            Value::native(move |_, _, _, a| {
                let (lo, hi) = (arg(&a, 0).i(), if a.len() > 1 { arg(&a, 1).i() } else { i32::MAX as i64 });
                Ok(Value::Int(lo + ((r() * (hi - lo + 1) as f64) as i64).min(hi - lo)))
            }),
        ),
        (
            "float",
            Value::native(move |_, _, _, a| {
                let (lo, hi) = (arg(&a, 0).f(), if a.len() > 1 { arg(&a, 1).f() } else { 1.0 });
                Ok(Value::Float(lo + r() * (hi - lo)))
            }),
        ),
        ("bool", Value::native(move |_, _, _, a| Ok(Value::Bool(r() * 100.0 < if a.is_empty() { 50.0 } else { arg(&a, 0).f() })))),
        (
            "getObject",
            Value::native(move |_, _, _, a| {
                Ok(match arg(&a, 0) {
                    Value::Array(v) => {
                        let v = v.borrow();
                        if v.is_empty() {
                            Value::Null
                        } else {
                            v[((r() * v.len() as f64) as usize).min(v.len() - 1)].clone()
                        }
                    }
                    _ => Value::Null,
                })
            }),
        ),
        ("sign", Value::native(move |_, _, _, _| Ok(Value::Int(if r() < 0.5 { -1 } else { 1 })))),
    ])
}

impl Host for Ctx<'_> {
    fn global(&mut self, _interp: &mut Interp, name: &str, _path: Option<&str>) -> Option<Value> {
        if let Some(i) = CLASSES.iter().position(|c| *c == name) {
            return Some(Value::Host(HostRef { kind: K_CLASS, id: i as u64 }));
        }
        Some(self.stub(name))
    }

    fn get(&mut self, interp: &mut Interp, obj: HostRef, field: &str) -> Result<Option<Value>, String> {
        Ok(match obj.kind {
            K_CLASS => {
                let class = CLASSES[obj.id as usize];
                match self.class_get(interp, class, field) {
                    Some(v) => Some(v),
                    None if class == "FlxColor" || class == "FlxEase" => None,
                    None => Some(self.stub(&format!("{class}.{field}"))),
                }
            }
            K_CONV | K_DBOX | K_SPEAKER | K_TEXT => crate::dialogue::get(self, obj.kind, field),
            K_SPRITE => match self.sprite_get(obj.id, field) {
                Some(v) => Some(v),
                None => Some(self.stub(&format!("FlxSprite.{field}"))),
            },
            K_SOUND => {
                let s = self.state.sounds.get(obj.id as usize).ok_or("no such sound")?;
                Some(match field {
                    "time" => Value::Float(s.playing.map(|(_, t)| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0)),
                    "playing" => Value::Bool(s.playing.is_some_and(|(_, t)| s.pcm.as_ref().is_some_and(|p| s.looped || t.elapsed().as_secs_f64() * 1000.0 < p.duration_ms()))),
                    "volume" => Value::Float(s.volume as f64),
                    "length" => Value::Float(s.pcm.as_ref().map(|p| p.duration_ms()).unwrap_or(0.0)),
                    _ => self.stub(&format!("FunkinSound.{field}")),
                })
            }
            K_ANIM => self.anim_get(obj.id, field),
            K_POINT => {
                let (id, which) = (obj.id & ((1 << 40) - 1), obj.id >> 40);
                let a = self.actor(id).ok_or("sprite is gone")?;
                let p = match which {
                    0 => a.scale,
                    1 => a.scroll,
                    2 => a.flx_offset(),
                    _ => a.velocity,
                };
                match field {
                    "x" => Some(Value::Float(p[0] as f64)),
                    "y" => Some(Value::Float(p[1] as f64)),
                    _ => None,
                }
            }
            K_PLAYSTATE => self.playstate_get(field).or_else(|| Some(self.stub(&format!("PlayState.instance.{field}")))),
            K_SONG => match field {
                "id" => Some(Value::str(&self.state.song_id)),
                _ => Some(self.stub(&format!("currentSong.{field}"))),
            },
            K_STAGE => match field {
                "id" | "stageName" => Some(Value::str(&self.state.stage_id)),
                "members" => {
                    let n = self.world.as_deref().map(|w| w.props.len()).unwrap_or(0);
                    let mut v: Vec<Value> = (0..n as u64).map(Self::sprite_ref).collect();
                    for slot in 0..3u64 {
                        if self.world.as_deref().is_some_and(|w| w.chars[slot as usize].is_some()) {
                            v.push(Self::sprite_ref(CHAR | slot));
                        }
                    }
                    Some(Value::array(v))
                }
                _ => None,
            },
            K_KEYS => {
                let set = match obj.id {
                    1 => &self.state.keys_down,
                    2 => &self.state.keys_pressed,
                    3 => &self.state.keys_released,
                    _ => {
                        return Ok(match field {
                            "pressed" => Some(Value::Host(HostRef { kind: K_KEYS, id: 1 })),
                            "justPressed" => Some(Value::Host(HostRef { kind: K_KEYS, id: 2 })),
                            "justReleased" => Some(Value::Host(HostRef { kind: K_KEYS, id: 3 })),
                            "enabled" => Some(Value::Bool(true)),
                            _ => None,
                        })
                    }
                };
                match field {
                    "ANY" => Some(Value::Bool(!set.is_empty())),
                    _ => FLX_KEYS.iter().find(|k| k.0 == field).map(|k| Value::Bool(set.contains(&k.1))),
                }
            }
            K_CONDUCTOR => Some(match field {
                "songPosition" => Value::Float(self.state.song_pos),
                "beatLengthMs" | "crochet" => Value::Float(self.state.beat_ms),
                "stepLengthMs" | "stepCrochet" => Value::Float(self.state.step_ms),
                "currentBeat" => Value::Int(self.state.beat),
                "currentStep" => Value::Int(self.state.step),
                "bpm" => Value::Float(self.state.bpm),
                "currentBeatTime" => Value::Float(self.state.song_pos / self.state.beat_ms.max(1e-6)),
                "currentStepTime" => Value::Float(self.state.song_pos / self.state.step_ms.max(1e-6)),
                // `songPosition` already has the offsets applied.
                "combinedOffset" | "instrumentalOffset" | "audioVisualOffset" | "inputOffset" | "globalOffset" => Value::Float(0.0),
                _ => return Ok(None),
            }),
            K_TWEEN => Some(match field {
                "value" => Value::Float(self.state.tween_values.get(&obj.id).copied().unwrap_or(0.0)),
                "active" | "running" => Value::Bool(self.state.tweens.iter().any(|t| t.id == obj.id && t.active)),
                "finished" => Value::Bool(!self.state.tweens.iter().any(|t| t.id == obj.id)),
                _ => return Ok(None),
            }),
            K_TIMER => Some(match field {
                "active" => Value::Bool(self.state.timers.iter().any(|t| t.id == obj.id && t.active)),
                "finished" => Value::Bool(!self.state.timers.iter().any(|t| t.id == obj.id)),
                "loopsLeft" => Value::Int(self.state.timers.iter().find(|t| t.id == obj.id).map(|t| (t.loops - t.done).max(0)).unwrap_or(0)),
                "elapsedLoops" => Value::Int(self.state.timers.iter().find(|t| t.id == obj.id).map(|t| t.done).unwrap_or(0)),
                _ => return Ok(None),
            }),
            K_STRUM => {
                let side = (obj.id & 1) as usize;
                let s = &self.state.strum[side];
                let group = |p: usize| Value::Host(HostRef { kind: K_SGROUP, id: (side as u64) << 8 | p as u64 });
                Some(match field {
                    "members" => Value::array((0..6).map(group).collect()),
                    "strumlineNotes" => group(SG_STRUMS),
                    "notes" => group(SG_NOTES),
                    "holdNotes" => group(SG_HOLDS),
                    "noteSplashes" => group(SG_SPLASHES),
                    "noteHoldCovers" => group(SG_COVERS),
                    "background" => group(SG_BG),
                    "x" => Value::Float((s.base[0] + s.off[0]) as f64),
                    "y" => Value::Float((s.base[1] + s.off[1]) as f64),
                    "width" => Value::Float(s.width as f64),
                    "height" => Value::Float(s.height as f64),
                    "visible" => Value::Bool(s.visible),
                    "alpha" => Value::Float(s.alpha as f64),
                    "scale" => Value::Host(HostRef { kind: K_NSCALE, id: SCALE_LINE | side as u64 }),
                    "isPlayer" => Value::Bool(side == 0),
                    "cameras" => Value::array(vec![Value::Host(HostRef { kind: K_CAMERA, id: s.cam })]),
                    "camera" => Value::Host(HostRef { kind: K_CAMERA, id: s.cam }),
                    "length" => Value::Int(6),
                    _ => return Ok(None),
                })
            }
            K_SGROUP => {
                let (side, part) = ((obj.id >> 8) as usize & 1, (obj.id & 0xFF) as usize);
                Some(match field {
                    "visible" => Value::Bool(self.state.strum[side].part_visible[part]),
                    "alpha" => Value::Float(self.state.strum[side].part_alpha[part] as f64),
                    "members" => Value::array(self.sgroup_members(side, part)),
                    "length" => Value::Int(self.sgroup_members(side, part).len() as i64),
                    _ => return Ok(None),
                })
            }
            K_RECEPTOR => {
                let (side, lane) = ((obj.id >> 8) as usize & 1, (obj.id & 3) as usize);
                let s = &self.state.strum[side];
                let r = s.receptors[lane];
                Some(match field {
                    "x" => Value::Float((r[0] + s.off[0] + s.lane_off[lane][0]) as f64),
                    "y" => Value::Float((r[1] + s.off[1] + s.lane_off[lane][1]) as f64),
                    "width" => Value::Float(r[2] as f64),
                    "height" => Value::Float(r[3] as f64),
                    "direction" | "ID" => Value::Int(lane as i64),
                    "alpha" => Value::Float(s.lane_alpha[lane] as f64),
                    "visible" => Value::Bool(s.lane_visible[lane]),
                    _ => return Ok(None),
                })
            }
            K_NOTE | K_HOLD => {
                let (side, idx) = ((obj.id >> 32) as usize & 1, (obj.id & 0xFFFF_FFFF) as usize);
                let s = &self.state.strum[side];
                let v = s.view(idx).cloned().unwrap_or_default();
                let hold = obj.kind == K_HOLD;
                let r = match field {
                    "x" if hold => Some(Value::Float(s.hold_x.get(&idx).copied().unwrap_or(v.hold_x) as f64)),
                    "y" if hold => Some(Value::Float((v.hold_y + s.hold_dy.get(&idx).copied().unwrap_or(0.0)) as f64)),
                    "alpha" if hold => Some(Value::Float(s.hold_alpha.get(&idx).copied().unwrap_or(1.0) as f64)),
                    "x" => Some(Value::Float(s.note_x.get(&idx).copied().unwrap_or(v.x) as f64)),
                    "y" => Some(Value::Float((v.y + s.dy.get(&idx).copied().unwrap_or(0.0)) as f64)),
                    "alpha" => Some(Value::Float(s.note_alpha.get(&idx).copied().unwrap_or(1.0) as f64)),
                    "width" => Some(Value::Float(v.w as f64)),
                    "height" => Some(Value::Float(v.h as f64)),
                    "visible" | "alive" | "exists" => Some(Value::Bool(true)),
                    "direction" | "noteDirection" => Some(Value::Int(v.dir as i64)),
                    "strumTime" => Some(Value::Float(v.strum_time)),
                    "hasBeenHit" | "hitNote" => Some(Value::Bool(v.hit)),
                    "missedNote" | "hasMissed" => Some(Value::Bool(v.missed)),
                    "sustainLength" => Some(Value::Float(v.sustain)),
                    "fullSustainLength" => Some(Value::Float(v.full_sustain)),
                    "scale" if !hold => Some(Value::Host(HostRef { kind: K_NSCALE, id: obj.id })),
                    "holdNoteSprite" if !hold => Some(if v.has_hold { Value::Host(HostRef { kind: K_HOLD, id: obj.id }) } else { Value::Null }),
                    "zoom" => Some(Value::Float(1.0)),
                    "graphic" => Some(Value::object(vec![("width", Value::Float(0.0)), ("height", Value::Float(0.0))])),
                    _ => None,
                };
                match r {
                    Some(r) => Some(r),
                    None => match self.state.note_info.get(&obj.id).and_then(|o| o.field(field)) {
                        Some(f) => Some(f),
                        None => return Ok(None),
                    },
                }
            }
            K_VBITMAP => {
                let Some(v) = self.state.videos.get(&obj.id) else { return Ok(Some(Value::Null)) };
                let size = match (&v.video, self.gfx.as_deref()) {
                    (Some(vid), Some(g)) => vid.texture().map(|t| g.texture_size(t)),
                    _ => None,
                };
                Some(match field {
                    "onEndReached" => v.signals[0].obj.clone(),
                    "onFormatSetup" => v.signals[1].obj.clone(),
                    "onEncounteredError" => v.signals[2].obj.clone(),
                    "onOpening" | "onPlaying" => v.signals[3].obj.clone(),
                    "bitmapData" => match size {
                        Some((w, h)) => Value::object(vec![("width", Value::Float(w as f64)), ("height", Value::Float(h as f64))]),
                        None => Value::Null,
                    },
                    "time" => Value::Float(v.video.as_ref().map(|x| x.time() * 1000.0).unwrap_or(0.0)),
                    "isPlaying" => Value::Bool(v.playing && !v.paused && v.video.is_some()),
                    "volume" | "volumeAdjust" => Value::Float(v.volume as f64),
                    _ => Value::Null,
                })
            }
            K_NSCALE => {
                let p = self.nscale(obj.id);
                Some(match field {
                    "x" => Value::Float(p[0] as f64),
                    "y" => Value::Float(p[1] as f64),
                    _ => return Ok(None),
                })
            }
            K_FOLLOW => {
                let p = self.world.as_deref().map(|w| w.camera.follow_point).unwrap_or([0.0; 2]);
                Some(match field {
                    "x" => Value::Float(p[0]),
                    "y" => Value::Float(p[1]),
                    _ => Value::Null,
                })
            }
            K_HUD => {
                let e = &self.state.hud[(obj.id & 0xFF) as usize];
                let r = e.rect.get();
                Some(match field {
                    "visible" => Value::Bool(e.visible),
                    "alpha" => Value::Float(e.alpha as f64),
                    "x" => Value::Float(e.pos[0].unwrap_or(r[0]) as f64),
                    "y" => Value::Float(e.pos[1].unwrap_or(r[1]) as f64),
                    "width" | "frameWidth" => Value::Float(r[2] as f64),
                    "height" | "frameHeight" => Value::Float(r[3] as f64),
                    "flipX" => Value::Bool(e.flip.unwrap_or(false)),
                    "autoUpdate" => Value::Bool(e.auto_update),
                    "bopEvery" => Value::Int(e.bop_every),
                    "bopAngle" => Value::Float(e.bop_angle as f64),
                    "zIndex" => Value::Int(e.z.unwrap_or(match obj.id as usize {
                        HUD_BAR_BG => 800,
                        HUD_BAR => 801,
                        HUD_SCORE => 802,
                        _ => 850,
                    }) as i64),
                    "value" => Value::Float(self.state.hud_health),
                    "text" => Value::str(""),
                    "animation" => Value::Host(HostRef { kind: K_HUD, id: obj.id | 1 << 8 }),
                    "name" | "curAnim" if obj.id >> 8 == 1 => Value::str(e.anim.as_deref().unwrap_or("idle")),
                    "scale" => Value::object(vec![("x", Value::Float(1.0)), ("y", Value::Float(1.0))]),
                    _ => Value::Null,
                })
            }
            K_CAMLIST => Some(match field {
                "list" => Value::array(self.world.as_deref().map(|w| w.cam_order.clone()).unwrap_or_else(|| vec![0, 1, 2]).into_iter().map(|id| Value::Host(HostRef { kind: K_CAMERA, id })).collect()),
                // `FlxG.cameras.defaults`: PlayState draws to camGame by default.
                "defaults" => Value::array(vec![Value::Host(HostRef { kind: K_CAMERA, id: 0 })]),
                _ => Value::Null,
            }),
            K_ORIGPOS => {
                let p = self.world.as_deref().and_then(|w| w.chars.get((obj.id & 0xFF) as usize)?.as_ref().map(|c| c.original)).unwrap_or([0.0; 2]);
                Some(match field {
                    "x" => Value::Float(p[0] as f64),
                    "y" => Value::Float(p[1] as f64),
                    _ => Value::Null,
                })
            }
            K_CFOCUS => {
                let p = self.char_focus(obj.id).unwrap_or([0.0; 2]);
                Some(match field {
                    "x" => Value::Float(p[0]),
                    "y" => Value::Float(p[1]),
                    _ => Value::Null,
                })
            }
            K_CAMPT => {
                let (cam, which) = (obj.id & 0xFF, obj.id >> 8);
                let p = self.cam_point(cam, which).unwrap_or([0.0; 2]);
                Some(match field {
                    "x" => Value::Float(p[0]),
                    "y" => Value::Float(p[1]),
                    _ => Value::Null,
                })
            }
            K_CAMERA if obj.id >= 3 => {
                let Some(c) = self.world.as_deref().and_then(|w| w.extra_cams.get(obj.id as usize - 3)) else { return Ok(Some(Value::Null)) };
                Some(match field {
                    "zoom" => Value::Float(c.cam.current_zoom),
                    "alpha" => Value::Float(c.alpha as f64),
                    "visible" => Value::Bool(c.visible),
                    "width" => Value::Float(c.cam.view_w),
                    "height" => Value::Float(c.cam.view_h),
                    "angle" => Value::Float(0.0),
                    "scroll" => Value::Host(HostRef { kind: K_CAMPT, id: obj.id }),
                    "targetOffset" => Value::Host(HostRef { kind: K_CAMPT, id: obj.id | 1 << 8 }),
                    "filters" => Value::array(vec![]),
                    _ => Value::Null,
                })
            }
            K_CAMERA => match field {
                "targetOffset" => Some(Value::Host(HostRef { kind: K_CAMPT, id: obj.id | 1 << 8 })),
                "angle" => Some(Value::Float(0.0)),
                "width" => Some(Value::Int(1280)),
                "height" => Some(Value::Int(720)),
                "zoom" => Some(Value::Float(self.world.as_deref().map(|w| w.camera.zoom()).unwrap_or(1.0))),
                "alpha" if obj.id == 1 => Some(Value::Float(1.0 - self.state.hud_fade as f64)),
                "visible" if obj.id == 1 => Some(Value::Bool(!self.state.hud_hidden)),
                "scroll" => Some(Value::Host(HostRef { kind: K_CAMPT, id: obj.id })),
                _ => Some(self.stub(&format!("camera.{field}"))),
            },
            K_SHADER => self.shader_get(obj.id, field),
            K_CURANIM => {
                let Some(a) = self.actor(obj.id) else { return Ok(Some(Value::Null)) };
                Some(match field {
                    "name" => Value::str(a.current()),
                    "finished" => Value::Bool(a.finished()),
                    "curFrame" => Value::Int(a.frame_number() as i64),
                    "numFrames" => Value::Int(a.frame_count() as i64),
                    "frameRate" => Value::Float(a.frame_rate() as f64),
                    "paused" | "reversed" => Value::Bool(false),
                    _ => Value::Null,
                })
            }
            K_MATRIX => {
                let m = self.actor(obj.id).and_then(|a| a.transform).unwrap_or(crate::gfx::Affine::IDENTITY).0;
                Some(match field {
                    "a" => Value::Float(m[0] as f64),
                    "b" => Value::Float(m[1] as f64),
                    "c" => Value::Float(m[2] as f64),
                    "d" => Value::Float(m[3] as f64),
                    "tx" => Value::Float(m[4] as f64),
                    "ty" => Value::Float(m[5] as f64),
                    _ => Value::Null,
                })
            }
            K_GROUP => {
                let g = self.state.groups.get(obj.id as usize).ok_or("no such group")?;
                Some(match field {
                    "zIndex" => Value::Int(g.z as i64),
                    "members" => Value::array(g.members.clone()),
                    "length" => Value::Int(g.members.len() as i64),
                    "x" | "y" => Value::Float(0.0),
                    "visible" | "exists" | "alive" | "active" => Value::Bool(true),
                    "alpha" => Value::Float(1.0),
                    _ => return Ok(None),
                })
            }
            K_CONTROLS => Some(match field {
                "CUTSCENE_ADVANCE" => Value::Bool(self.state.cutscene_advance),
                _ => Value::Bool(false),
            }),
            K_STEXT => Some(self.text_get(obj.id, field)),
            K_TMGR => Some(Value::Null),
            K_TRAIL => Some(match field {
                "zIndex" => Value::Int(self.world.as_deref().and_then(|w| w.trail_z(obj.id as usize)).unwrap_or(0) as i64),
                _ => Value::Null,
            }),
            K_SPARAM => {
                let (sid, name) = ((obj.id >> 20) as usize, self.state.param_names.get((obj.id & 0xFFFFF) as usize).cloned().unwrap_or_default());
                let s = self.state.shaders.get(sid).cloned();
                Some(match (field, s) {
                    ("value", Some(s)) => s.borrow().uniform(&name).map(|v| Value::array(v.iter().map(|x| Value::Float(*x)).collect())).unwrap_or(Value::Null),
                    _ => Value::Null,
                })
            }
            K_POPUPS => Some(match field {
                "offsets" => Value::array(self.state.popup_offsets.iter().map(|v| Value::Int(*v as i64)).collect()),
                _ => self.stub(&format!("comboPopUps.{field}")),
            }),
            K_FILTER => Some(match field {
                "shader" if obj.id != NO_FILTER => Value::Host(HostRef { kind: K_SHADER, id: obj.id }),
                _ => Value::Null,
            }),
            K_STUB => Some(self.stub(&format!("{}.{field}", self.state.stubs[obj.id as usize].clone()))),
            K_SCAN => None,
            _ => None,
        })
    }

    fn set(&mut self, interp: &mut Interp, obj: HostRef, field: &str, v: Value) -> Result<bool, String> {
        if obj.kind == K_SPRITE && obj.id & CHAR != 0 {
            // A character script's property setter (`override function set_alpha(val)`).
            let m = format!("set_{field}");
            if let Some(inst) = self.char_script(obj.id).filter(|i| matches!(i, Value::Instance(c) if c.class.has_method(&m))) {
                let key = (obj.id, field.to_owned());
                if !self.state.in_setter.contains(&key) {
                    self.state.in_setter.push(key.clone());
                    let r = interp.call_method(self, &inst, &m, vec![v]);
                    self.state.in_setter.retain(|k| *k != key);
                    r?;
                    return Ok(true);
                }
            }
        }
        Ok(match obj.kind {
            K_CLASS => {
                // A static field a script sets (`Constants.STRUMLINE_X_OFFSET = 95`);
                // the engine reads the ones it knows.
                let class = CLASSES[obj.id as usize];
                if (class, field) == ("HealthIcon", "BOP_SCALE") {
                    self.state.icon_bop_scale = Some(v.f() as f32);
                }
                self.state.class_statics.insert((class.to_owned(), field.to_owned()), v);
                true
            }
            K_SHADER => {
                self.shader_set(interp, obj.id, field, &v);
                true
            }
            K_FILTER | K_BITMAP | K_CONTROLS | K_TMGR => true,
            K_CURANIM => {
                if field == "curFrame" {
                    if let Some(a) = self.actor(obj.id) {
                        a.frame_override = None;
                        a.set_frame_number(v.i().max(0) as usize);
                    }
                }
                true
            }
            K_MATRIX => {
                let i = match field {
                    "a" => 0,
                    "b" => 1,
                    "c" => 2,
                    "d" => 3,
                    "tx" => 4,
                    "ty" => 5,
                    _ => return Ok(false),
                };
                if let Some(a) = self.actor(obj.id) {
                    a.transform.get_or_insert(crate::gfx::Affine::IDENTITY).0[i] = v.f() as f32;
                }
                true
            }
            K_GROUP => {
                if field == "zIndex" {
                    let z = v.i() as i32;
                    let Some(g) = self.state.groups.get_mut(obj.id as usize) else { return Ok(false) };
                    g.z = z;
                    let (members, added) = (g.members.clone(), g.added);
                    for m in &members {
                        if let Some(a) = Self::sprite_id(m).and_then(|id| self.actor(id)) {
                            a.z_index = z;
                        }
                    }
                    if added {
                        if let Some(w) = self.world.as_deref_mut() {
                            w.refresh();
                        }
                    }
                }
                true
            }
            K_STEXT => {
                self.text_set(obj.id, field, &v);
                true
            }
            K_PLAYSTATE if field == "isInCutscene" => {
                self.state.in_cutscene = v.truthy();
                true
            }
            K_PLAYSTATE if field == "health" => {
                self.state.health = v.f();
                self.state.health_set = true;
                true
            }
            K_PLAYSTATE if field == "mayPauseGame" => {
                self.state.no_pause = !v.truthy();
                true
            }
            K_TRAIL => {
                if field == "zIndex" {
                    if let Some(w) = self.world.as_deref_mut() {
                        w.set_trail_z(obj.id as usize, v.i() as i32);
                    }
                }
                true
            }
            K_SPARAM => {
                let (sid, name) = ((obj.id >> 20) as usize, self.state.param_names.get((obj.id & 0xFFFFF) as usize).cloned().unwrap_or_default());
                if let Some(s) = self.state.shaders.get(sid).cloned() {
                    let mut s = s.borrow_mut();
                    match (field, &v) {
                        ("input", Value::Host(HostRef { kind: K_BITMAP, id })) => s.set_texture(&name, crate::gfx::TexId::from_index(*id as usize)),
                        ("input", Value::Str(k)) => s.set_image(&name, self.image_file(k)),
                        ("value", Value::Array(a)) => {
                            let vals = a.borrow().iter().map(|x| if let Value::Bool(b) = x { *b as i32 as f64 } else { x.f() }).collect();
                            s.set_uniform(&name, vals);
                        }
                        _ => {}
                    }
                }
                true
            }
            K_POPUPS => {
                if let (Value::Array(a), "offsets") = (&v, field) {
                    let a = a.borrow();
                    self.state.popup_offsets = [a.first().map_or(0.0, |v| v.f()) as f32, a.get(1).map_or(0.0, |v| v.f()) as f32];
                }
                true
            }
            K_CAMERA if field == "filters" => {
                if obj.id == 0 {
                    self.set_filters(&v);
                } else if !matches!(&v, Value::Array(a) if a.borrow().is_empty()) {
                    interp.warn_once("filters on cameras other than camGame are not available to scripts yet".into());
                }
                true
            }
            K_SPRITE => self.sprite_set(obj.id, field, &v),
            K_CONV | K_DBOX | K_SPEAKER | K_TEXT => {
                crate::dialogue::set(self, obj.kind, obj.id, field, &v);
                true
            }
            K_ANIM => match field {
                "finishCallback" | "callback" if field == "finishCallback" => {
                    self.state.signals.entry(obj.id).or_default().finish_callback = v;
                    true
                }
                "curFrame" | "frameIndex" => {
                    if let Some(a) = self.actor(obj.id) {
                        a.frame_override = Some(v.i().max(0) as usize);
                    }
                    true
                }
                _ => false,
            },
            K_SOUND => {
                if field == "volume" {
                    if let Some(s) = self.state.sounds.get_mut(obj.id as usize) {
                        s.volume = v.f() as f32;
                        if let Some((id, _)) = s.playing {
                            self.audio.set_sfx_volume(id, s.volume);
                        }
                    }
                }
                true
            }
            K_POINT => {
                let (id, which) = (obj.id & ((1 << 40) - 1), obj.id >> 40);
                let Some(a) = self.actor(id) else { return Ok(false) };
                if which == 2 {
                    let cur = a.flx_offset();
                    let p = a.offset.insert(cur);
                    match field {
                        "x" => p[0] = v.f() as f32,
                        "y" => p[1] = v.f() as f32,
                        _ => return Ok(false),
                    }
                    return Ok(true);
                }
                let p = match which {
                    0 => &mut a.scale,
                    1 => &mut a.scroll,
                    _ => &mut a.velocity,
                };
                match field {
                    "x" => p[0] = v.f() as f32,
                    "y" => p[1] = v.f() as f32,
                    _ => return Ok(false),
                }
                true
            }
            K_STRUM => {
                let s = &mut self.state.strum[(obj.id & 1) as usize];
                match field {
                    "visible" => s.visible = v.truthy(),
                    "alpha" => s.alpha = v.f() as f32,
                    "x" => s.off[0] = v.f() as f32 - s.base[0],
                    "y" => s.off[1] = v.f() as f32 - s.base[1],
                    "cameras" | "camera" => {
                        let cam = match v {
                            Value::Array(a) => a.borrow().iter().find_map(|c| match c {
                                Value::Host(HostRef { kind: K_CAMERA, id }) => Some(*id),
                                _ => None,
                            }),
                            Value::Host(HostRef { kind: K_CAMERA, id }) => Some(id),
                            _ => None,
                        };
                        if let Some(c) = cam {
                            s.cam = c;
                        }
                    }
                    _ => {}
                }
                true
            }
            K_SGROUP => {
                let (side, part) = ((obj.id >> 8) as usize & 1, (obj.id & 0xFF) as usize);
                let s = &mut self.state.strum[side];
                match field {
                    "visible" => s.part_visible[part] = v.truthy(),
                    "alpha" => s.part_alpha[part] = v.f() as f32,
                    _ => {}
                }
                true
            }
            K_RECEPTOR => {
                let (side, lane) = ((obj.id >> 8) as usize & 1, (obj.id & 3) as usize);
                let s = &mut self.state.strum[side];
                let r = s.receptors[lane];
                match field {
                    "x" => s.lane_off[lane][0] = v.f() as f32 - r[0] - s.off[0],
                    "y" => s.lane_off[lane][1] = v.f() as f32 - r[1] - s.off[1],
                    "alpha" => s.lane_alpha[lane] = v.f() as f32,
                    "visible" => s.lane_visible[lane] = v.truthy(),
                    _ => {}
                }
                true
            }
            K_NOTE | K_HOLD => {
                let (side, idx) = ((obj.id >> 32) as usize & 1, (obj.id & 0xFFFF_FFFF) as usize);
                let s = &mut self.state.strum[side];
                let view = s.view(idx).cloned().unwrap_or_default();
                let hold = obj.kind == K_HOLD;
                match field {
                    "x" if hold => {
                        s.hold_x.insert(idx, v.f() as f32);
                    }
                    "y" if hold => {
                        s.hold_dy.insert(idx, v.f() as f32 - view.hold_y);
                    }
                    "alpha" if hold => {
                        s.hold_alpha.insert(idx, v.f() as f32);
                    }
                    "x" => {
                        s.note_x.insert(idx, v.f() as f32);
                    }
                    "y" => {
                        s.dy.insert(idx, v.f() as f32 - view.y);
                    }
                    "alpha" => {
                        s.note_alpha.insert(idx, v.f() as f32);
                    }
                    _ => {}
                }
                true
            }
            K_VBITMAP => {
                if let ("volume" | "volumeAdjust", Some(vs)) = (field, self.state.videos.get_mut(&obj.id)) {
                    vs.volume = v.f() as f32;
                }
                if let Some(v) = self.state.videos.get_mut(&obj.id) {
                    let vol = v.volume;
                    if let Some(vid) = &mut v.video {
                        vid.set_volume(self.audio, vol);
                    }
                }
                // `time = x` (ms, a resync): skip ahead.
                if field == "time" {
                    if let Some(vid) = self.state.videos.get_mut(&obj.id).and_then(|v| v.video.as_mut()) {
                        vid.seek(self.audio, v.f() / 1000.0);
                    }
                }
                true
            }
            K_NSCALE => {
                let mut p = self.nscale(obj.id);
                match field {
                    "x" => p[0] = v.f() as f32,
                    "y" => p[1] = v.f() as f32,
                    _ => return Ok(false),
                }
                self.set_nscale(obj.id, p);
                true
            }
            K_FOLLOW => {
                if let Some(w) = self.world.as_deref_mut() {
                    match field {
                        "x" => w.camera.follow_point[0] = v.f(),
                        "y" => w.camera.follow_point[1] = v.f(),
                        _ => {}
                    }
                }
                true
            }
            K_HUD => {
                let e = &mut self.state.hud[(obj.id & 0xFF) as usize];
                match field {
                    "visible" => e.visible = v.truthy(),
                    "alpha" => e.alpha = v.f() as f32,
                    "x" => e.pos[0] = Some(v.f() as f32),
                    "y" => e.pos[1] = Some(v.f() as f32),
                    "flipX" => e.flip = Some(v.truthy()),
                    "autoUpdate" => e.auto_update = v.truthy(),
                    "bopEvery" => e.bop_every = v.i(),
                    "bopAngle" => e.bop_angle = v.f() as f32,
                    "zIndex" => e.z = Some(v.i() as i32),
                    _ => {}
                }
                true
            }
            K_ORIGPOS => {
                let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((obj.id & 0xFF) as usize)?.as_mut()) else { return Ok(true) };
                match field {
                    "x" => c.original[0] = v.f() as f32,
                    "y" => c.original[1] = v.f() as f32,
                    _ => return Ok(false),
                }
                true
            }
            K_CFOCUS => {
                let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((obj.id & 0xFF) as usize)?.as_mut()) else { return Ok(true) };
                match field {
                    "x" => c.focus[0] = v.f(),
                    "y" => c.focus[1] = v.f(),
                    _ => return Ok(false),
                }
                true
            }
            K_CAMPT => {
                let (cam, which) = (obj.id & 0xFF, obj.id >> 8);
                if let Some(mut p) = self.cam_point(cam, which) {
                    match field {
                        "x" => p[0] = v.f(),
                        "y" => p[1] = v.f(),
                        _ => return Ok(false),
                    }
                    self.set_cam_point(cam, which, p);
                }
                true
            }
            K_CAMERA if obj.id >= 3 => {
                let pt = |v: &Value| [v.field("x").map(|x| x.f()).unwrap_or(0.0), v.field("y").map(|x| x.f()).unwrap_or(0.0)];
                let point = match &v {
                    Value::Host(h) if h.kind == K_CAMPT => self.cam_point(h.id & 0xFF, h.id >> 8),
                    Value::Host(_) | Value::Null => None,
                    other => Some(pt(other)),
                };
                let Some(c) = self.world.as_deref_mut().and_then(|w| w.extra_cams.get_mut(obj.id as usize - 3)) else { return Ok(true) };
                match field {
                    "zoom" => c.cam.current_zoom = v.f(),
                    "alpha" => c.alpha = v.f() as f32,
                    "visible" => c.visible = v.truthy(),
                    "targetOffset" => c.cam.target_offset = point.unwrap_or([0.0; 2]),
                    "scroll" => c.cam.scroll = point.unwrap_or(c.cam.scroll),
                    _ => {}
                }
                true
            }
            K_CAMERA => {
                match (obj.id, field) {
                    (0, "targetOffset") => {
                        let p = match &v {
                            Value::Host(h) if h.kind == K_CAMPT => self.cam_point(h.id & 0xFF, h.id >> 8),
                            other => Some([other.field("x").map(|x| x.f()).unwrap_or(0.0), other.field("y").map(|x| x.f()).unwrap_or(0.0)]),
                        };
                        if let (Some(p), Some(w)) = (p, self.world.as_deref_mut()) {
                            w.camera.target_offset = p;
                        }
                    }
                    (1, "visible") => self.state.hud_hidden = !v.truthy(),
                    (1, "alpha") => self.state.hud_fade = 1.0 - (v.f() as f32).clamp(0.0, 1.0),
                    // PlayState writes `currentCameraZoom * cameraBopMultiplier` to
                    // FlxG.camera.zoom every frame, so a direct write never shows.
                    _ => {}
                }
                true
            }
            K_STUB | K_PLAYSTATE => true,
            _ => false,
        })
    }

    fn call(&mut self, interp: &mut Interp, obj: HostRef, method: &str, args: Vec<Value>) -> Result<Option<Value>, String> {
        match obj.kind {
            K_CLASS => {
                let class = CLASSES[obj.id as usize];
                match self.class_call(interp, class, method, args)? {
                    Some(v) => Ok(Some(v)),
                    None => {
                        let msg = format!("{class}.{method}() is not available to scripts yet");
                        interp.warn_once(msg);
                        Ok(Some(Value::Null))
                    }
                }
            }
            K_STAGE => Ok(self.stage_call(method, &args)),
            K_KEYS => {
                let set = match method {
                    "anyPressed" => &self.state.keys_down,
                    "anyJustPressed" => &self.state.keys_pressed,
                    "anyJustReleased" => &self.state.keys_released,
                    "checkStatus" => {
                        let k = flx_key(&arg(&args, 0));
                        let st = arg(&args, 1).i();
                        // FlxInputState: JUST_RELEASED -1, RELEASED 0, PRESSED 1, JUST_PRESSED 2.
                        let s = &self.state;
                        let v = k.is_some_and(|k| match st {
                            2 => s.keys_pressed.contains(&k),
                            1 => s.keys_down.contains(&k),
                            -1 => s.keys_released.contains(&k),
                            _ => !s.keys_down.contains(&k),
                        });
                        return Ok(Some(Value::Bool(v)));
                    }
                    _ => return Ok(Some(Value::Bool(false))),
                };
                let keys: Vec<i64> = match arg(&args, 0) {
                    Value::Array(v) => v.borrow().iter().filter_map(flx_key).collect(),
                    v => flx_key(&v).into_iter().collect(),
                };
                Ok(Some(Value::Bool(keys.iter().any(|k| set.contains(k)))))
            }
            K_BEATSIG => {
                let f = arg(&args, 0);
                match method {
                    "add" | "addOnce" => self.state.beat_listeners.push(f),
                    "remove" => self.state.beat_listeners.retain(|x| !same_value(x, &f)),
                    "removeAll" => self.state.beat_listeners.clear(),
                    "has" => return Ok(Some(Value::Bool(self.state.beat_listeners.iter().any(|x| same_value(x, &f))))),
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_FOLLOW => {
                if let Some(w) = self.world.as_deref_mut() {
                    let p = &mut w.camera.follow_point;
                    match method {
                        "setPosition" | "set" => *p = [arg(&args, 0).f(), arg(&args, 1).f()],
                        "add" => *p = [p[0] + arg(&args, 0).f(), p[1] + arg(&args, 1).f()],
                        "subtract" => *p = [p[0] - arg(&args, 0).f(), p[1] - arg(&args, 1).f()],
                        _ => {}
                    }
                }
                Ok(Some(Value::Host(obj)))
            }
            K_CONV if obj.id == 1 => {
                crate::dialogue::music_call(self, method);
                Ok(Some(Value::Null))
            }
            K_TEXT => Ok(Some(Value::Null)),
            K_CONV | K_DBOX | K_SPEAKER => {
                use crate::dialogue::Who;
                let who = match obj.kind {
                    K_CONV => Who::Conv,
                    K_DBOX => Who::Box,
                    _ => Who::Speaker,
                };
                match crate::dialogue::native(interp, self, who, method, args)? {
                    Some(v) => Ok(Some(v)),
                    None => {
                        interp.warn_once(format!("{who:?}.{method}() is not available to scripts yet"));
                        Ok(Some(Value::Null))
                    }
                }
            }
            K_VSIGNAL => {
                let cb = arg(&args, 0);
                let list = &mut self.state.video_signals[(obj.id as usize).min(4)];
                match method {
                    "add" | "addOnce" => {
                        if !list.iter().any(|(c, _)| c.equals(&cb)) {
                            list.push((cb, method == "addOnce"));
                        }
                    }
                    "remove" => list.retain(|(c, _)| !c.equals(&cb)),
                    "removeAll" => list.clear(),
                    "has" => return Ok(Some(Value::Bool(list.iter().any(|(c, _)| c.equals(&cb))))),
                    "dispatch" => self.state.video_signal_fire = Some(obj.id as usize),
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_SIGNAL => {
                let (sprite, which) = (obj.id & ((1 << 40) - 1), obj.id >> 40);
                let cb = arg(&args, 0);
                let sig = self.state.signals.entry(sprite).or_default();
                let list = match which {
                    1 => &mut sig.frame,
                    2 => &mut sig.label,
                    _ => &mut sig.finish,
                };
                match method {
                    "add" | "addOnce" => {
                        if !list.iter().any(|(c, _)| c.equals(&cb)) {
                            list.push((cb, method == "addOnce"));
                        }
                    }
                    "remove" => list.retain(|(c, _)| !c.equals(&cb)),
                    "removeAll" => list.clear(),
                    "has" => return Ok(Some(Value::Bool(list.iter().any(|(c, _)| c.equals(&cb))))),
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_SOUND => {
                let audio = self.audio;
                let Some(s) = self.state.sounds.get_mut(obj.id as usize) else { return Ok(Some(Value::Null)) };
                match method {
                    "play" | "resume" => {
                        let force = arg(&args, 0).truthy();
                        if s.playing.is_none() || force || method == "resume" {
                            if let Some((id, _)) = s.playing.take() {
                                audio.stop_sfx(id);
                            }
                            if let Some(p) = &s.pcm {
                                log::debug!("script sound {} plays at volume {} ({:.2}s, peak {:.3})", obj.id, s.volume, p.duration_ms() / 1000.0, p.samples.iter().fold(0f32, |m, x| m.max(x.abs())));
                                s.playing = Some((audio.play_sfx_tracked(p.clone(), s.volume), std::time::Instant::now()));
                            }
                        }
                    }
                    "stop" | "pause" | "destroy" => {
                        if let Some((id, _)) = s.playing.take() {
                            audio.stop_sfx(id);
                        }
                    }
                    _ => {}
                }
                Ok(Some(Value::Host(obj)))
            }
            K_SPRITE => Ok(Some(match self.sprite_call(interp, obj.id, method, &args) {
                Some(v) => v,
                None => {
                    interp.warn_once(format!("FlxSprite.{method}() is not available to scripts yet"));
                    Value::Null
                }
            })),
            K_ANIM => Ok(Some(match self.anim_call(obj.id, method, &args) {
                Some(v) => v,
                None => {
                    interp.warn_once(format!("animation.{method}() is not available to scripts yet"));
                    Value::Null
                }
            })),
            K_POINT => {
                if method == "set" {
                    let (x, y) = (arg(&args, 0), if args.len() > 1 { arg(&args, 1) } else { arg(&args, 0) });
                    self.set(interp, obj, "x", x)?;
                    self.set(interp, obj, "y", y)?;
                    return Ok(Some(Value::Null));
                }
                Ok(None)
            }
            K_TWEEN => {
                if method == "cancel" {
                    for t in &mut self.state.tweens {
                        if t.id == obj.id {
                            t.active = false;
                        }
                    }
                }
                Ok(Some(Value::Null))
            }
            K_TIMER => {
                match method {
                    "start" => {
                        let (time, cb, loops) = (arg(&args, 0).f(), arg(&args, 1), if args.len() > 2 { arg(&args, 2).i() } else { 1 });
                        self.state.timers.retain(|t| t.id != obj.id);
                        let mgr = self.state.timer_mgr.get(&obj.id).copied();
                        self.state.timers.push(Timer { id: obj.id, mgr, time: time.max(0.0), t: 0.0, loops, done: 0, cb, active: true });
                        return Ok(Some(Value::Host(obj)));
                    }
                    "cancel" => self.state.timers.retain(|t| t.id != obj.id),
                    "reset" => {
                        if let Some(t) = self.state.timers.iter_mut().find(|t| t.id == obj.id) {
                            // FlxTimer.reset: start again with the same callback and loops.
                            t.t = 0.0;
                            t.done = 0;
                            t.active = true;
                            if !args.is_empty() && arg(&args, 0).f() >= 0.0 {
                                t.time = arg(&args, 0).f();
                            }
                        }
                    }
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_SHADER => Ok(Some(self.shader_call(interp, obj.id, method, &args))),
            K_FILTER => Ok(Some(Value::Null)),
            K_ORIGPOS => {
                if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((obj.id & 0xFF) as usize)?.as_mut()) {
                    match method {
                        "set" => {
                            let x = arg(&args, 0).f() as f32;
                            c.original = [x, if args.len() > 1 { arg(&args, 1).f() as f32 } else { x }];
                        }
                        "add" => {
                            c.original[0] += arg(&args, 0).f() as f32;
                            c.original[1] += arg(&args, 1).f() as f32;
                        }
                        _ => {}
                    }
                }
                Ok(Some(Value::Host(obj)))
            }
            K_CFOCUS => {
                if let Some(c) = self.world.as_deref_mut().and_then(|w| w.chars.get_mut((obj.id & 0xFF) as usize)?.as_mut()) {
                    match method {
                        "set" => {
                            let x = arg(&args, 0).f();
                            c.focus = [x, if args.len() > 1 { arg(&args, 1).f() } else { x }];
                        }
                        "add" => {
                            c.focus[0] += arg(&args, 0).f();
                            c.focus[1] += arg(&args, 1).f();
                        }
                        _ => {}
                    }
                }
                Ok(Some(Value::Host(obj)))
            }
            K_CAMPT => {
                if method == "set" {
                    let (cam, which) = (obj.id & 0xFF, obj.id >> 8);
                    let x = arg(&args, 0).f();
                    let y = if args.len() > 1 { arg(&args, 1).f() } else { x };
                    self.set_cam_point(cam, which, [x, y]);
                }
                Ok(Some(Value::Host(obj)))
            }
            K_STRUM => {
                let side = (obj.id & 1) as usize;
                Ok(Some(match method {
                    "isKeyHeld" => Value::Bool(self.state.strum[side].key_held.get(arg(&args, 0).i().clamp(0, 3) as usize).copied().unwrap_or(false)),
                    "forEach" | "forEachAlive" => {
                        for m in (0..6).map(|p| Value::Host(HostRef { kind: K_SGROUP, id: (side as u64) << 8 | p })) {
                            interp.call(self, &arg(&args, 0), vec![m])?;
                        }
                        Value::Null
                    }
                    "getNotesOnScreen" => Value::array(self.sgroup_members(side, SG_NOTES)),
                    _ => return Ok(None),
                }))
            }
            K_SGROUP => {
                let (side, part) = ((obj.id >> 8) as usize & 1, (obj.id & 0xFF) as usize);
                Ok(Some(match method {
                    "forEach" | "forEachAlive" | "forEachExists" => {
                        for m in self.sgroup_members(side, part) {
                            interp.call(self, &arg(&args, 0), vec![m])?;
                        }
                        Value::Null
                    }
                    _ => return Ok(None),
                }))
            }
            K_RECEPTOR => {
                let (side, lane) = ((obj.id >> 8) as usize & 1, (obj.id & 3) as usize);
                Ok(Some(match method {
                    "getCurrentAnimation" => Value::str(self.state.strum[side].anims[lane]),
                    "setPosition" => {
                        let s = &mut self.state.strum[side];
                        let r = s.receptors[lane];
                        s.lane_off[lane] = [arg(&args, 0).f() as f32 - r[0] - s.off[0], arg(&args, 1).f() as f32 - r[1] - s.off[1]];
                        Value::Null
                    }
                    _ => Value::Null,
                }))
            }
            K_NOTE | K_HOLD => {
                let (side, idx) = ((obj.id >> 32) as usize & 1, (obj.id & 0xFFFF_FFFF) as usize);
                match method {
                    "set_alpha" => {
                        let s = &mut self.state.strum[side];
                        let a = arg(&args, 0).f() as f32;
                        if obj.kind == K_HOLD {
                            s.hold_alpha.insert(idx, a);
                        } else {
                            s.note_alpha.insert(idx, a);
                        }
                        Ok(Some(Value::Null))
                    }
                    "getBaseScrollSpeed" => Ok(Some(Value::Float(1.0))),
                    "updateColorTransform" | "updateClipping" | "updateHitbox" | "setGraphicSize" | "kill" | "revive" => Ok(Some(Value::Null)),
                    _ => match self.state.note_info.get(&obj.id).and_then(|o| o.field(method)) {
                        Some(f) => Ok(Some(interp.call(self, &f, args)?)),
                        None => Ok(None),
                    },
                }
            }
            K_NSCALE => {
                match method {
                    "set" => {
                        let x = arg(&args, 0).f() as f32;
                        let y = if args.len() > 1 { arg(&args, 1).f() as f32 } else { x };
                        self.set_nscale(obj.id, [x, y]);
                    }
                    _ => return Ok(None),
                }
                Ok(Some(Value::Null))
            }
            K_HUD => {
                let e = &mut self.state.hud[(obj.id & 0xFF) as usize];
                match method {
                    "setPosition" => e.pos = [Some(arg(&args, 0).f() as f32), Some(arg(&args, 1).f() as f32)],
                    "play" if obj.id >> 8 == 1 => e.anim = Some(arg(&args, 0).to_string()),
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_CAMLIST => {
                let cam = match arg(&args, 0) {
                    Value::Host(HostRef { kind: K_CAMERA, id }) => Some(id),
                    _ => None,
                };
                if let (Some(cam), Some(w)) = (cam, self.world.as_deref_mut()) {
                    match method {
                        "add" => {
                            w.cam_order.retain(|&c| c != cam);
                            w.cam_order.push(cam);
                        }
                        "insert" => {
                            w.cam_order.retain(|&c| c != cam);
                            let at = (arg(&args, 1).i().max(0) as usize).min(w.cam_order.len());
                            w.cam_order.insert(at, cam);
                        }
                        "remove" => w.cam_order.retain(|&c| c != cam),
                        _ => {}
                    }
                    return Ok(Some(arg(&args, 0)));
                }
                Ok(Some(Value::Null))
            }
            K_CAMERA if obj.id >= 3 => {
                let Some(c) = self.world.as_deref_mut().and_then(|w| w.extra_cams.get_mut(obj.id as usize - 3)) else { return Ok(Some(Value::Null)) };
                match method {
                    // follow(target, style, lerp): only camGame's follow point is followed.
                    "follow" => c.follow = Some(if args.len() > 2 && !arg(&args, 2).is_null() { arg(&args, 2).f() } else { 1.0 }),
                    "unfollow" => c.follow = None,
                    "shake" => c.cam.shake(if args.is_empty() { 0.05 } else { arg(&args, 0).f() }, if args.len() > 1 { arg(&args, 1).f() } else { 0.5 }),
                    "snapToTarget" => c.cam.snap(),
                    _ => {}
                }
                Ok(Some(Value::Null))
            }
            K_CAMERA => {
                if method == "fade" {
                    // fade(color = BLACK, duration = 1, fadeIn = false, onComplete, force = false)
                    let i = (obj.id as usize).min(2);
                    let busy = self.state.flx_fades[i].as_ref().is_some_and(|f| f.t < f.duration);
                    if !busy || arg(&args, 4).truthy() {
                        let color = if args.is_empty() { 0xFF000000 } else { arg(&args, 0).i() };
                        let duration = if args.len() > 1 { arg(&args, 1).f() } else { 1.0 };
                        self.state.flx_fades[i] = Some(FlxFade { color: int_to_color(color), duration, fade_in: arg(&args, 2).truthy(), t: 0.0, on_complete: arg(&args, 3) });
                    }
                } else if method == "stopFade" {
                    self.state.flx_fades[(obj.id as usize).min(2)] = None;
                } else if method == "setFilters" && obj.id == 0 {
                    self.set_filters(&arg(&args, 0));
                } else if method == "flash" && obj.id == 0 {
                    let c = if args.is_empty() { 0xFFFFFFFF } else { arg(&args, 0).i() };
                    let dur = if args.len() > 1 { arg(&args, 1).f() as f32 } else { 1.0 };
                    self.state.flash = Some((int_to_color(c), dur));
                } else if method == "shake" && obj.id == 0 {
                    let i = if args.is_empty() { 0.05 } else { arg(&args, 0).f() };
                    let d = if args.len() > 1 { arg(&args, 1).f() } else { 0.5 };
                    if let Some(w) = self.world.as_deref_mut() {
                        w.camera.shake(i, d);
                    }
                } else {
                    interp.warn_once(format!("camera.{method}() is not available to scripts yet"));
                }
                Ok(Some(Value::Null))
            }
            K_PLAYSTATE => {
                match method {
                    "startCountdown" => self.state.start_countdown = true,
                    "endSong" => self.state.end_song = true,
                    "startConversation" => crate::dialogue::start(interp, self, &arg(&args, 0).to_string()),
                    "tweenCameraToPosition" | "tweenCameraZoom" | "resetCamera" | "cancelCameraFollowTween" | "cancelCameraZoomTween" => {
                        if let Some(w) = self.world.as_deref_mut() {
                            let ease = |v: Value| if v.is_null() { "linear".to_owned() } else { v.to_string() };
                            match method {
                                "tweenCameraToPosition" => {
                                    let dur = arg(&args, 2).f();
                                    w.camera.focus(arg(&args, 0).f(), arg(&args, 1).f(), Some(dur), &ease(arg(&args, 3)));
                                    if dur <= 0.0 {
                                        w.camera.snap();
                                    }
                                }
                                "resetCamera" => {
                                    // resetCamera(resetZoom = true, cancelTweens = true, snap = true)
                                    let flag = |i: usize| args.len() <= i || arg(&args, i).truthy();
                                    if flag(0) {
                                        w.camera.zoom_to(1.0, false, 0.0, "linear");
                                        w.camera.bop_multiplier = 1.0;
                                    }
                                    if flag(2) {
                                        w.camera.snap();
                                    }
                                }
                                "tweenCameraZoom" => {
                                    let direct = args.len() < 3 || arg(&args, 2).truthy();
                                    let z = arg(&args, 0).f();
                                    if !(z.is_finite() && z > 0.0) {
                                        interp.warn_once(format!("tweenCameraZoom({z}) ignored"));
                                    } else {
                                    w.camera.zoom_to(arg(&args, 0).f(), direct, arg(&args, 1).f(), &ease(arg(&args, 3)));
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    "refresh" => {
                        if let Some(w) = self.world.as_deref_mut() {
                            w.refresh();
                        }
                    }
                    "add" => self.add_obj(&arg(&args, 0), true),
                    "insert" => self.add_obj(&arg(&args, 1), true),
                    "remove" => self.add_obj(&arg(&args, 0), false),
                    // Mobile-only pause button.
                    "togglePauseButton" => {}
                    _ => interp.warn_once(format!("PlayState.instance.{method}() is not available to scripts yet")),
                }
                Ok(Some(Value::Null))
            }
            K_STUB => {
                let name = format!("{}.{method}()", self.state.stubs[obj.id as usize]);
                interp.warn_once(format!("{name} is not available to scripts yet"));
                Ok(Some(Value::Null))
            }
            // `Conductor.instance.update(t)` with t at or past the start during
            // the countdown: the song starts there (what skipping does).
            K_CONDUCTOR if method == "update" => {
                if args.first().is_some_and(|a| matches!(a, Value::Int(_) | Value::Float(_)) && a.f() >= 0.0) {
                    self.state.skip_countdown = true;
                }
                Ok(Some(Value::Null))
            }
            K_SONG | K_SCAN | K_CONDUCTOR | K_CURANIM | K_MATRIX => Ok(Some(Value::Null)),
            K_TMGR => {
                if matches!(method, "clear" | "destroy") {
                    let id = obj.id;
                    self.state.timers.retain(|t| t.mgr != Some(id));
                }
                Ok(Some(Value::Null))
            }
            K_CONTROLS => Ok(Some(match method {
                "getDialogueNameFromToken" => Value::str(&self.state.advance_key),
                _ => Value::Bool(false),
            })),
            K_STEXT => {
                match method {
                    "setFormat" => self.text_format(obj.id, &args),
                    "setPosition" => {
                        self.text_set(obj.id, "x", &arg(&args, 0));
                        self.text_set(obj.id, "y", &arg(&args, 1));
                    }
                    "screenCenter" => {
                        // FlxAxes: X = 0x01, Y = 0x10, XY = 0x11 (the default).
                        let axes = if args.is_empty() { 0x11 } else { arg(&args, 0).i() };
                        let (w, h) = (self.text_get(obj.id, "width").f(), self.text_get(obj.id, "height").f());
                        if axes & 0x01 != 0 {
                            self.text_set(obj.id, "x", &Value::Float((1280.0 - w) / 2.0));
                        }
                        if axes & 0x10 != 0 {
                            self.text_set(obj.id, "y", &Value::Float((720.0 - h) / 2.0));
                        }
                    }
                    _ => {}
                }
                Ok(Some(Value::Host(obj)))
            }
            K_GROUP => {
                let g = obj.id as usize;
                match method {
                    "add" | "insert" | "remove" => {
                        let m = arg(&args, if method == "insert" { 1 } else { 0 });
                        let add = method != "remove";
                        let Some(grp) = self.state.groups.get_mut(g) else { return Ok(Some(Value::Null)) };
                        if add {
                            grp.members.push(m.clone());
                        } else {
                            grp.members.retain(|x| !x.equals(&m));
                        }
                        let (added, z) = (grp.added, grp.z);
                        if added {
                            if add {
                                if let Some(a) = Self::sprite_id(&m).and_then(|id| self.actor(id)) {
                                    a.z_index = z;
                                }
                            }
                            self.add_obj(&m, add);
                        }
                        Ok(Some(m))
                    }
                    _ => Ok(Some(Value::Null)),
                }
            }
            _ => Ok(None),
        }
    }

    fn construct(&mut self, interp: &mut Interp, class: &str, args: Vec<Value>) -> Result<Value, String> {
        match class {
            "FlxTimer" => {
                let id = self.state.next_id;
                self.state.next_id += 1;
                if let Value::Host(HostRef { kind: K_TMGR, id: m }) = arg(&args, 0) {
                    self.state.timer_mgr.insert(id, m);
                }
                Ok(Value::Host(HostRef { kind: K_TIMER, id }))
            }
            "Conductor" => Ok(script_conductor()),
            "FunkinVideoSprite" | "FlxVideoSprite" => {
                let Some(id) = self.new_sprite(arg(&args, 0).f(), arg(&args, 1).f()) else { return Ok(self.stub(class)) };
                self.state.videos.insert(
                    id,
                    VideoSprite { video: None, playing: false, paused: false, volume: 1.0, signals: std::array::from_fn(|_| Signal::new()), formatted: false },
                );
                Ok(Self::sprite_ref(id))
            }
            "FlxBar" => {
                // new FlxBar(x, y, direction = LEFT_TO_RIGHT, width = 100, height = 10,
                //            parentRef, variable = "", min = 0, max = 100, showBorder)
                let or = |v: Value, d: f64| if v.is_null() { d } else { v.f() };
                let dir = match arg(&args, 2) {
                    Value::Null => "LEFT_TO_RIGHT".to_owned(),
                    v => v.to_string(),
                };
                let (w, h) = (or(arg(&args, 3), 100.0) as f32, or(arg(&args, 4), 10.0) as f32);
                let Some(id) = self.new_sprite(arg(&args, 0).f(), arg(&args, 1).f()) else { return Ok(self.stub("FlxBar")) };
                if let Some(a) = self.actor(id) {
                    a.take_graphics(Actor::bar("", w, h, &dir));
                }
                let (min, max) = (or(arg(&args, 7), 0.0), or(arg(&args, 8), 100.0));
                let variable = if arg(&args, 6).is_null() { String::new() } else { arg(&args, 6).to_string() };
                self.state.bars.insert(id, BarBind { parent: arg(&args, 5), variable, min, max, value: min });
                Ok(Self::sprite_ref(id))
            }
            "FlxCamera" => {
                // new FlxCamera(x, y, width = 0, height = 0, zoom = 0): 0 means the game's.
                let Some(w) = self.world.as_deref_mut() else { return Ok(Value::Null) };
                let or = |v: f64, d: f64| if v > 0.0 { v } else { d };
                let (cw, ch) = (or(arg(&args, 2).f(), 1280.0), or(arg(&args, 3).f(), 720.0));
                w.extra_cams.push(crate::world::ExtraCam::new(cw, ch, or(arg(&args, 4).f(), 1.0)));
                Ok(Value::Host(HostRef { kind: K_CAMERA, id: 2 + w.extra_cams.len() as u64 }))
            }
            "ColorTransform" => {
                let n = ["redMultiplier", "greenMultiplier", "blueMultiplier", "alphaMultiplier", "redOffset", "greenOffset", "blueOffset", "alphaOffset"];
                Ok(Value::object(n.iter().enumerate().map(|(i, k)| (*k, if args.len() > i { Value::Float(arg(&args, i).f()) } else { Value::Float(if i < 4 { 1.0 } else { 0.0 }) })).collect()))
            }
            // The engine draws Nene's A-Bot speaker and its visualizer itself; a
            // script's own `ABotVis` is an inert stand-in.
            "ABotVis" => {
                let noop = || Value::native(|_, _, _, _| Ok(Value::Null));
                Ok(Value::object(vec![
                    ("members", Value::array(vec![])),
                    ("x", Value::Float(0.0)),
                    ("y", Value::Float(0.0)),
                    ("alpha", Value::Float(1.0)),
                    ("visible", Value::Bool(false)),
                    ("initAnalyzer", noop()),
                    ("dumpSound", noop()),
                ]))
            }
            "FlxSpriteElement" => Ok(Value::object(vec![("sprite", arg(&args, 0)), ("active", Value::Bool(true))])),
            "FlxTimerManager" => {
                let id = self.state.next_id;
                self.state.next_id += 1;
                Ok(Value::Host(HostRef { kind: K_TMGR, id }))
            }
            "FlxText" | "FunkinText" => {
                // new FlxText(x, y, fieldWidth, text, size = 8)
                let size = if args.len() > 4 { arg(&args, 4).f() } else { 8.0 };
                let text = if arg(&args, 3).is_null() { String::new() } else { arg(&args, 3).to_string() };
                Ok(self.new_text(arg(&args, 0).f(), arg(&args, 1).f(), arg(&args, 2).f(), text, size))
            }
            c if GROUP_CLASSES.contains(&c) => {
                self.state.groups.push(Group::default());
                Ok(Value::Host(HostRef { kind: K_GROUP, id: self.state.groups.len() as u64 - 1 }))
            }
            "FlxPoint" => Ok(flx_point(arg(&args, 0).f(), arg(&args, 1).f())),
            "Sequence" => {
                // `new Sequence([{time, callback}, ...])`: one-shot timers, started now.
                let mut ids = Vec::new();
                if let Value::Array(v) = arg(&args, 0) {
                    for e in v.borrow().iter() {
                        let id = self.state.next_id;
                        self.state.next_id += 1;
                        let cb = e.field("callback").unwrap_or(Value::Null);
                        let time = e.field("time").map(|t| t.f()).unwrap_or(0.0);
                        self.state.timers.push(Timer { id, mgr: None, time, t: 0.0, loops: 1, done: 0, cb, active: true });
                        ids.push(id);
                    }
                }
                let stop = Value::native(move |interp, host, _, _| {
                    for &id in &ids {
                        host.call(interp, HostRef { kind: K_TIMER, id }, "cancel", vec![])?;
                    }
                    Ok(Value::Null)
                });
                Ok(Value::object(vec![("destroy", stop.clone()), ("stop", stop), ("running", Value::Bool(true))]))
            }
            c if crate::shaders::class(c).is_some() => {
                let src = match arg(&args, 0) {
                    Value::Str(s) => Some(s.to_string()),
                    _ => None,
                };
                let class = crate::shaders::class(c).unwrap();
                let v = self.make_shader(class, src);
                if class == crate::shaders::Class::Wiggle {
                    // new WiggleEffectRuntime(speed, freq, amplitude, ?effect = DREAMY)
                    if let Some(sh) = self.shader_of(&v) {
                        let mut sh = sh.borrow_mut();
                        for (i, p) in ["waveSpeed", "waveFrequency", "waveAmplitude", "effectType"].iter().enumerate() {
                            sh.set_prop(p, if args.len() > i { arg(&args, i).f() } else { 0.0 });
                        }
                    }
                }
                Ok(v)
            }
            "FunkTrail" | "FlxTrail" => {
                // new FunkTrail(target, ?graphic, length = 10, delay = 0.1, alpha = 0.4, diff = 0.05)
                let n = |i: usize, d: f64| if args.len() > i && !arg(&args, i).is_null() { arg(&args, i).f() } else { d };
                let owner = Self::sprite_id(&arg(&args, 0)).filter(|id| id & CHAR != 0).map(|id| (id & 0xFF) as usize);
                match (owner, self.world.as_deref_mut()) {
                    (Some(o), Some(w)) => {
                        let i = w.new_trail(o, n(2, 10.0) as usize, n(3, 0.1) as f32, n(4, 0.4) as f32, n(5, 0.05) as f32);
                        Ok(Value::Host(HostRef { kind: K_TRAIL, id: i as u64 }))
                    }
                    _ => {
                        interp.warn_once(format!("new {class}() only follows characters for now"));
                        Ok(self.stub(class))
                    }
                }
            }
            "ShaderFilter" => Ok(match self.shader_of(&arg(&args, 0)) {
                Some(s) => Value::Host(HostRef { kind: K_FILTER, id: s.borrow().id }),
                None => Value::Host(HostRef { kind: K_FILTER, id: NO_FILTER }),
            }),
            // Blur filters need an offscreen blur pass; they're left out for now.
            "BlurFilter" | "GlowFilter" | "DropShadowFilter" => Ok(Value::Host(HostRef { kind: K_FILTER, id: NO_FILTER })),
            "FlxTiledSprite" => {
                // new FlxTiledSprite(graphic, width, height, repeatX = true, repeatY = true)
                let key = match arg(&args, 0) {
                    Value::Str(s) => s.to_string(),
                    _ => String::new(),
                };
                let v = self.make_sprite(interp, 0.0, 0.0, "image", &key);
                if let Value::Host(HostRef { kind: K_SPRITE, id }) = v {
                    let flag = |i: usize| args.len() <= i || arg(&args, i).truthy();
                    let tile = crate::actor::Tile { w: arg(&args, 1).f() as f32, h: arg(&args, 2).f() as f32, repeat: [flag(3), flag(4)], scroll: [0.0; 2] };
                    if let Some(a) = self.actor(id) {
                        a.tile = Some(tile);
                    }
                }
                Ok(v)
            }
            "FlxBackdrop" => {
                // new FlxBackdrop(graphic, repeatAxes = XY (0x11), spacingX, spacingY)
                let key = match arg(&args, 0) {
                    Value::Str(s) => s.to_string(),
                    _ => String::new(),
                };
                let axes = if args.len() > 1 { arg(&args, 1).i() } else { 0x11 };
                let v = self.make_sprite(interp, 0.0, 0.0, "image", &key);
                if let Some(a) = Self::sprite_id(&v).and_then(|id| self.actor(id)) {
                    a.backdrop = [axes & 0x01 != 0, axes & 0x10 != 0];
                }
                Ok(v)
            }
            c if SPRITE_CLASSES.contains(&c) => {
                let key = match arg(&args, 2) {
                    Value::Str(s) => s.to_string(),
                    _ => String::new(),
                };
                // FlxAtlasSprite(x, y, Paths.animateAtlas(key)): an Animate texture atlas.
                let kind = if c == "FlxAtlasSprite" { "animateatlas" } else { "image" };
                Ok(self.make_sprite(interp, arg(&args, 0).f(), arg(&args, 1).f(), kind, &key))
            }
            _ => {
                interp.warn_once(format!("new {class}() is not available to scripts yet"));
                Ok(self.stub(class))
            }
        }
    }

    fn construct_base(&mut self, interp: &mut Interp, base: &str, _this: &Rc<Instance>, args: Vec<Value>) -> Result<Value, String> {
        let bindable = base == "Stage" || base == "Song" || CHAR_BASES.contains(&base) || DIALOGUE_BASES.contains(&base);
        if base == "NoteKind" {
            // super(noteKind, description, noteStyleId, params, noAnim)
            let mut f = _this.fields.borrow_mut();
            for (i, n) in ["noteKind", "description", "noteStyleId", "params", "noAnim"].iter().enumerate() {
                f.insert((*n).into(), arg(&args, i));
            }
            return Ok(Value::Null);
        }
        if base == "Module" || base == "SongEvent" {
            // `super(id, priority, {state: PlayState})` / `super(id)`: plain script objects.
            let mut f = _this.fields.borrow_mut();
            if base == "Module" {
                f.insert("moduleId".into(), arg(&args, 0));
                f.insert("priority".into(), if args.len() > 1 { arg(&args, 1) } else { Value::Int(1000) });
                f.insert("state".into(), Value::object(vec![("state", arg(&args, 2).field("state").unwrap_or(Value::Null))]));
                f.insert("active".into(), Value::Bool(true));
            } else {
                f.insert("id".into(), arg(&args, 0));
            }
            f.insert("superClass".into(), Value::Instance(_this.clone()));
            return Ok(Value::Null);
        }
        if !bindable {
            // A script class extending an engine class (a sprite, a shader, ...).
            if GROUP_CLASSES.contains(&base) {
                self.state.groups.push(Group::default());
                return Ok(Value::Host(HostRef { kind: K_GROUP, id: self.state.groups.len() as u64 - 1 }));
            }
            if SPRITE_CLASSES.contains(&base) {
                let key = match arg(&args, 2) {
                    Value::Str(s) => s.to_string(),
                    _ => String::new(),
                };
                let kind = if base == "FlxAtlasSprite" { "animateatlas" } else { "image" };
                return Ok(self.make_sprite(interp, arg(&args, 0).f(), arg(&args, 1).f(), kind, &key));
            }
            if let Some(c) = crate::shaders::class(base) {
                // `class BuildingEffectShader extends FlxRuntimeShader { ... super(fragText); }`
                let src = match arg(&args, 0) {
                    Value::Str(s) => Some(s.to_string()),
                    _ => None,
                };
                return Ok(self.make_shader(c, src));
            }
            return Ok(self.stub(base));
        }
        match self.state.binding {
            Some(r @ HostRef { kind: K_SCAN, id }) => {
                // `super('senpai', {variation: 'pico'})`: a variation's own song script.
                let mut key = arg(&args, 0).to_string();
                if let Some(v) = arg(&args, 1).field("variation").filter(|v| !v.is_null()) {
                    key = format!("{key}:{v}");
                }
                self.state.scanned.insert(id, key);
                Ok(Value::Host(r))
            }
            Some(r) => Ok(Value::Host(r)),
            None => Ok(Value::Null),
        }
    }

    fn iterate(&mut self, _interp: &mut Interp, obj: HostRef) -> Option<Vec<Value>> {
        match obj.kind {
            K_SGROUP => Some(self.sgroup_members((obj.id >> 8) as usize & 1, (obj.id & 0xFF) as usize)),
            K_STRUM => Some((0..6).map(|p| Value::Host(HostRef { kind: K_SGROUP, id: (obj.id & 1) << 8 | p })).collect()),
            _ => None,
        }
    }

    fn is_type(&mut self, _interp: &mut Interp, v: &Value, ty: &str) -> bool {
        match v {
            Value::Host(HostRef { kind: K_SPRITE, id }) => {
                matches!(ty, "FlxSprite" | "FunkinSprite" | "FlxBasic" | "FlxObject" | "Bopper" | "StageProp") || (id & CHAR != 0 && matches!(ty, "BaseCharacter"))
            }
            Value::Host(HostRef { kind: K_STAGE, .. }) => ty == "Stage",
            Value::Host(HostRef { kind: K_SHADER, id }) => {
                ty == "FlxShader" || ty == "FlxRuntimeShader" || self.state.shaders.get(*id as usize).is_some_and(|s| s.borrow().name() == ty)
            }
            _ => false,
        }
    }

    fn call_self(&mut self, interp: &mut Interp, obj: HostRef, _args: Vec<Value>) -> Result<Option<Value>, String> {
        if obj.kind == K_STUB {
            let name = self.state.stubs[obj.id as usize].clone();
            interp.warn_once(format!("{name}() is not available to scripts yet"));
            return Ok(Some(Value::Null));
        }
        Ok(None)
    }

    fn is_stub(&self, obj: HostRef) -> bool {
        obj.kind == K_STUB
    }

    fn describe(&self, obj: HostRef) -> String {
        match obj.kind {
            K_CLASS => CLASSES[obj.id as usize].to_owned(),
            K_STUB => self.state.stubs.get(obj.id as usize).cloned().unwrap_or_default(),
            K_SPRITE => format!("sprite {}", obj.id),
            K_STAGE => "the stage".into(),
            K_SHADER => self.state.shaders.get(obj.id as usize).map(|s| s.borrow().name().to_owned()).unwrap_or_default(),
            _ => format!("engine object {}:{}", obj.kind, obj.id),
        }
    }
}
