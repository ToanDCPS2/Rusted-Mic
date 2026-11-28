//! Port of `funkin.play.cutscene.dialogue` (`Conversation`, `DialogueBox`,
//! `Speaker`, `FunkinTypeText`): the Week 6 style dialogue cutscenes.
//!
//! These are engine classes, so they're native; scripts extend them the way
//! Polymod allows (`class RosesDialogueBox extends DialogueBox`): every
//! overridable method goes through the bound script instance first, and its
//! `super.method()` lands back here (see [`native`]).

use std::sync::Arc;

use funkin_core::camera::GameCamera;
use funkin_core::stage::AnimationData;
use funkin_script::{HostRef, Interp, Value};
use serde::Deserialize;

use crate::actor::Actor;
use crate::audio::{Pcm, Track};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Affine, Gfx, VIEW_H, VIEW_W};
use crate::scripting::{event, Ctx, K_CONV, K_DBOX, K_SPEAKER, K_TEXT};

/// Sprite ids (`K_SPRITE`) of the dialogue's sprites.
pub const DLG: u64 = 1 << 33;
pub const DLG_BOX: u64 = DLG;
pub const DLG_SPEAKER: u64 = DLG | 1;

#[derive(Deserialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct ConversationData {
    #[serde(default)]
    backdrop: Option<BackdropData>,
    #[serde(default)]
    music: Option<MusicData>,
    #[serde(default)]
    outro: Option<OutroData>,
    #[serde(default)]
    dialogue: Vec<EntryData>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BackdropData {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    fade_time: f64,
    #[serde(default)]
    color: String,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MusicData {
    #[serde(default)]
    asset: String,
    #[serde(default)]
    fade_time: f64,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct OutroData {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    fade_time: f64,
}

#[derive(Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
struct EntryData {
    #[serde(default)]
    speaker: String,
    #[serde(default)]
    speaker_animation: Option<String>,
    #[serde(default)]
    r#box: String,
    #[serde(default)]
    box_animation: Option<String>,
    #[serde(default)]
    text: Vec<String>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BoxData {
    asset_path: String,
    #[serde(default)]
    is_pixel: bool,
    #[serde(default)]
    offsets: [f64; 2],
    #[serde(default = "one")]
    scale: f64,
    text: BoxText,
    #[serde(default)]
    animations: Vec<AnimationData>,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct BoxText {
    #[serde(default)]
    offsets: [f64; 2],
    #[serde(default = "w300")]
    width: f64,
    #[serde(default = "s32")]
    size: f64,
    #[serde(default)]
    color: Option<String>,
    #[serde(default)]
    shadow_color: Option<String>,
    #[serde(default = "two")]
    shadow_width: f64,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SpeakerData {
    asset_path: String,
    #[serde(default)]
    is_pixel: bool,
    #[serde(default)]
    flip_x: bool,
    #[serde(default)]
    offsets: [f64; 2],
    #[serde(default = "one")]
    scale: f64,
    #[serde(default)]
    animations: Vec<AnimationData>,
}

fn one() -> f64 {
    1.0
}
fn two() -> f64 {
    2.0
}
fn w300() -> f64 {
    300.0
}
fn s32() -> f64 {
    32.0
}

/// `#AARRGGBB` / `#RRGGBB`.
fn color(s: &str) -> [f32; 4] {
    let h = s.trim_start_matches('#').trim_start_matches("0x");
    let v = u32::from_str_radix(h, 16).unwrap_or(0);
    let (a, rgb) = if h.len() > 6 { ((v >> 24) & 255, v & 0xFFFFFF) } else { (255, v) };
    [((rgb >> 16) & 255) as f32 / 255.0, ((rgb >> 8) & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0, a as f32 / 255.0]
}

fn stepped(t: f64, steps: f64) -> f64 {
    (t * steps).floor() / steps
}

/// `FunkinTypeText`: text revealed a character at a time.
struct TypeText {
    prefix: String,
    /// `prefix + text`, wrapped to the field width.
    full: Vec<char>,
    shown: usize,
    t: f64,
    typing: bool,
    delay: f64,
    x: f32,
    y: f32,
    width: f32,
    size: f32,
    color: [f32; 4],
    shadow: [f32; 4],
    shadow_w: f32,
    visible: bool,
    sound: Option<Arc<Pcm>>,
    sound_id: Option<u64>,
}

pub struct DialogueBox {
    pub id: String,
    pub sprite: Actor,
    text: TypeText,
    pub script: Value,
    pub alpha: f32,
    /// (animation, finished) last frame, for `onAnimationFinished`.
    last: (String, bool),
}

pub struct Speaker {
    pub id: String,
    pub sprite: Actor,
    pub script: Value,
}

struct Music {
    track: Arc<Track>,
    /// (from, to, duration, elapsed)
    fade: Option<(f32, f32, f64, f64)>,
}

pub struct Conversation {
    pub id: String,
    data: ConversationData,
    pub state: String,
    entry: usize,
    line: usize,
    pub alpha: f32,
    backdrop: Option<([f32; 4], f64, f64)>,
    music: Option<Music>,
    pub speaker: Option<Speaker>,
    pub dbox: Option<DialogueBox>,
    /// (duration, elapsed)
    outro: Option<(f64, f64)>,
    pub script: Value,
    /// The outro ended: PlayState drops the conversation and starts the countdown.
    pub finished: bool,
    font: Option<TtfFont>,
    cam: GameCamera,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Who {
    Conv,
    Box,
    Speaker,
}

fn who_ref(w: Who) -> HostRef {
    HostRef { kind: match w { Who::Conv => K_CONV, Who::Box => K_DBOX, Who::Speaker => K_SPEAKER }, id: 0 }
}

fn read_json<T: for<'de> Deserialize<'de>>(ctx: &Ctx, kind: &str, id: &str) -> Result<T, String> {
    let path = ctx.assets.root().join("data").join("dialogue").join(kind).join(format!("{id}.json"));
    let text = funkin_core::vfs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn arg(a: &[Value], i: usize) -> Value {
    a.get(i).cloned().unwrap_or(Value::Null)
}

fn conv<'a>(ctx: &'a mut Ctx) -> Option<&'a mut Conversation> {
    ctx.state.dialogue.as_mut()
}

/// Load a sparrow sprite for the dialogue (in the song's texture scope).
fn load_sprite(ctx: &mut Ctx, path: &str, anims: &[AnimationData], pixel: bool) -> Result<Actor, String> {
    let level = ctx.world.as_deref().and_then(|w| w.stage.directory.clone());
    let assets = ctx.assets;
    let scope = ctx.tex_scope;
    let gfx = ctx.gfx.as_deref_mut().ok_or("no renderer")?;
    let mut a = gfx.with_scope(scope, |g| Actor::animated(g, assets, path, "sparrow", path, level.as_deref(), anims, !pixel))?;
    a.scroll = [0.0, 0.0];
    a.should_bop = false;
    Ok(a)
}

/// The bound script for a dialogue class (`class X extends DialogueBox { super('id') }`).
fn bind(interp: &mut Interp, ctx: &mut Ctx, base: &str, id: &str, target: HostRef) -> Value {
    let Some(name) = ctx.state.dialogue_classes.get(&(base.to_owned(), id.to_owned())).cloned() else { return Value::Null };
    let Some(cls) = interp.class(&name) else { return Value::Null };
    ctx.state.binding = Some(target);
    let r = interp.instantiate(ctx, &cls, vec![]);
    ctx.state.binding = None;
    match r {
        Ok(i) => Value::Instance(i),
        Err(e) => {
            log::warn!("script {name}: {e}");
            Value::Null
        }
    }
}

/// An overridable method: the script's version if it has one (whose `super`
/// comes back to [`native`]), else the engine's.
pub fn vcall(interp: &mut Interp, ctx: &mut Ctx, who: Who, name: &str, args: Vec<Value>) -> Value {
    let script = match (who, conv(ctx)) {
        (_, None) => return Value::Null,
        (Who::Conv, Some(c)) => c.script.clone(),
        (Who::Box, Some(c)) => match &c.dbox {
            Some(b) => b.script.clone(),
            None => return Value::Null,
        },
        (Who::Speaker, Some(c)) => match &c.speaker {
            Some(s) => s.script.clone(),
            None => return Value::Null,
        },
    };
    let r = if let Value::Instance(_) = &script {
        interp.call_method(ctx, &script, name, args)
    } else {
        native(interp, ctx, who, name, args).map(|v| v.unwrap_or(Value::Null))
    };
    match r {
        Ok(v) => v,
        Err(e) => {
            interp.warn_once(format!("dialogue {name}: {e}"));
            Value::Null
        }
    }
}

/// `PlayState.dispatchEvent`: the song/stage/character scripts, then the conversation.
fn dispatch_all(interp: &mut Interp, ctx: &mut Ctx, name: &str) {
    let ev = dialogue_event();
    for b in ctx.state.bound.clone() {
        let Value::Instance(inst) = &b else { continue };
        if inst.class.has_method(name) {
            if let Err(e) = interp.call_method(ctx, &b, name, vec![ev.clone()]) {
                interp.warn_once(format!("script {}.{name}: {e}", inst.class.decl.name));
            }
        }
    }
    vcall(interp, ctx, Who::Conv, name, vec![ev]);
}

fn dialogue_event() -> Value {
    event(vec![("conversation", Value::Host(HostRef { kind: K_CONV, id: 0 }))])
}

fn canceled(ev: &Value) -> bool {
    ev.field("eventCanceled").is_some_and(|v| v.truthy())
}

/// `Conversation.propagateEvent`: to the box, then the speaker.
fn propagate(interp: &mut Interp, ctx: &mut Ctx, name: &str, ev: &Value) {
    let (has_box, has_speaker) = match conv(ctx) {
        Some(c) => (c.dbox.is_some(), c.speaker.is_some()),
        None => return,
    };
    if has_box {
        vcall(interp, ctx, Who::Box, name, vec![ev.clone()]);
    }
    if has_speaker {
        vcall(interp, ctx, Who::Speaker, name, vec![ev.clone()]);
    }
}

/// `PlayState.startConversation(id)`.
pub fn start(interp: &mut Interp, ctx: &mut Ctx, id: &str) {
    let data: ConversationData = match read_json(ctx, "conversations", id) {
        Ok(d) => d,
        Err(e) => {
            log::warn!("conversation {id}: {e}");
            ctx.state.start_countdown = true;
            return;
        }
    };
    let font = ctx.assets.ttf_path("5by7_Bold.ttf").and_then(|p| TtfFont::load(&p).ok());
    ctx.state.dialogue = Some(Conversation {
        id: id.to_owned(),
        data,
        state: "Start".into(),
        entry: 0,
        line: 0,
        alpha: 1.0,
        backdrop: None,
        music: None,
        speaker: None,
        dbox: None,
        outro: None,
        script: Value::Null,
        finished: false,
        font,
        cam: GameCamera::new(1.0, VIEW_W as f64, VIEW_H as f64),
    });
    let script = bind(interp, ctx, "Conversation", id, who_ref(Who::Conv));
    if let Some(c) = conv(ctx) {
        c.script = script;
    }
    vcall(interp, ctx, Who::Conv, "onCreate", vec![event(vec![])]);
}

/// `advanceConversation` (the accept key).
pub fn advance(interp: &mut Interp, ctx: &mut Ctx) {
    let Some(state) = conv(ctx).map(|c| c.state.clone()) else { return };
    match state.as_str() {
        "Start" => dispatch_all(interp, ctx, "onDialogueStart"),
        "Opening" | "Speaking" => dispatch_all(interp, ctx, "onDialogueCompleteLine"),
        "Idle" => dispatch_all(interp, ctx, "onDialogueLine"),
        "Ending" => end_outro(interp, ctx),
        _ => {}
    }
}

/// Per frame: typing, animation callbacks, `onUpdate`, the outro and music fades.
pub fn update(interp: &mut Interp, ctx: &mut Ctx, dt: f64) {
    let audio = ctx.audio;
    let mut typed_done = false;
    let mut anim_done: Option<String> = None;
    {
        let Some(c) = conv(ctx) else { return };
        if let Some(b) = &mut c.dbox {
            b.sprite.update(dt as f32);
            let t = &mut b.text;
            if t.typing {
                t.t += dt;
                while t.typing && t.t >= t.delay {
                    t.t -= t.delay;
                    t.shown += 1;
                    if let Some(s) = &t.sound {
                        if let Some(id) = t.sound_id.take() {
                            audio.stop_sfx(id);
                        }
                        t.sound_id = Some(audio.play_sfx_tracked(s.clone(), 0.6));
                    }
                    if t.shown >= t.full.len() {
                        t.shown = t.full.len();
                        t.typing = false;
                        typed_done = true;
                    }
                }
            }
            let now = (b.sprite.current().to_owned(), b.sprite.finished());
            if now.1 && (!b.last.1 || b.last.0 != now.0) && !now.0.is_empty() {
                anim_done = Some(now.0.clone());
            }
            b.last = now;
        }
        if let Some(s) = &mut c.speaker {
            s.sprite.update(dt as f32);
        }
        if let Some((_, fade, t)) = &mut c.backdrop {
            *t += dt;
            let _ = fade;
        }
        if let Some(m) = &mut c.music {
            if let Some((from, to, dur, t)) = &mut m.fade {
                *t += dt;
                let k = if *dur > 0.0 { (*t / *dur).min(1.0) } else { 1.0 } as f32;
                m.track.set_volume(*from + (*to - *from) * k);
                if k >= 1.0 {
                    m.fade = None;
                }
            }
        }
    }
    if typed_done {
        vcall(interp, ctx, Who::Box, "onTypingComplete", vec![]);
    }
    if let Some(name) = anim_done {
        vcall(interp, ctx, Who::Box, "onAnimationFinished", vec![Value::str(&name)]);
    }
    vcall(interp, ctx, Who::Conv, "onUpdate", vec![event(vec![("elapsed", Value::Float(dt))])]);
    // The outro fade.
    let mut ended = false;
    if let Some(c) = conv(ctx) {
        if let Some((dur, t)) = &mut c.outro {
            *t += dt;
            let k = if *dur > 0.0 { (*t / *dur).min(1.0) } else { 1.0 };
            c.alpha = 1.0 - stepped(k, 8.0) as f32;
            ended = k >= 1.0;
        }
    }
    if ended {
        end_outro(interp, ctx);
    }
}

fn end_outro(interp: &mut Interp, ctx: &mut Ctx) {
    if conv(ctx).is_some_and(|c| c.finished) {
        return;
    }
    vcall(interp, ctx, Who::Conv, "onDestroy", vec![event(vec![])]);
}

fn entry(c: &Conversation) -> Option<&EntryData> {
    c.data.dialogue.get(c.entry)
}

fn line_string(c: &Conversation) -> String {
    entry(c).and_then(|e| e.text.get(c.line).cloned()).unwrap_or_default()
}

/// The engine's implementation of a dialogue method (also `super.x()` from scripts).
pub fn native(interp: &mut Interp, ctx: &mut Ctx, who: Who, name: &str, a: Vec<Value>) -> Result<Option<Value>, String> {
    match who {
        Who::Conv => conv_native(interp, ctx, name, a),
        Who::Box => box_native(interp, ctx, name, a),
        Who::Speaker => speaker_native(ctx, name, a),
    }
}

fn conv_native(interp: &mut Interp, ctx: &mut Ctx, name: &str, a: Vec<Value>) -> Result<Option<Value>, String> {
    let ev = arg(&a, 0);
    match name {
        "onCreate" => {
            if let Some(c) = conv(ctx) {
                c.entry = 0;
                c.line = 0;
                c.state = "Start".into();
            }
            dispatch_all(interp, ctx, "onDialogueStart");
        }
        "onDialogueStart" => {
            propagate(interp, ctx, name, &ev);
            vcall(interp, ctx, Who::Conv, "setupMusic", vec![]);
            vcall(interp, ctx, Who::Conv, "setupBackdrop", vec![]);
            if let Some(c) = conv(ctx) {
                c.state = "Opening".into();
            }
            vcall(interp, ctx, Who::Conv, "showCurrentDialogueBox", vec![]);
            vcall(interp, ctx, Who::Conv, "playDialogueBoxAnimation", vec![]);
        }
        "onDialogueLine" => {
            propagate(interp, ctx, name, &ev);
            if canceled(&ev) {
                return Ok(Some(Value::Null));
            }
            let Some(c) = conv(ctx) else { return Ok(None) };
            c.line += 1;
            let lines = entry(c).map(|e| e.text.len()).unwrap_or(0);
            if c.line >= lines {
                c.line = 0;
                c.entry += 1;
                if c.entry >= c.data.dialogue.len() {
                    dispatch_all(interp, ctx, "onDialogueEnd");
                } else if c.state == "Idle" {
                    vcall(interp, ctx, Who::Conv, "showCurrentDialogueBox", vec![]);
                    vcall(interp, ctx, Who::Conv, "playDialogueBoxAnimation", vec![]);
                    if let Some(c) = conv(ctx) {
                        c.state = "Opening".into();
                    }
                }
            } else {
                c.state = "Speaking".into();
                let s = line_string(c);
                vcall(interp, ctx, Who::Box, "appendText", vec![Value::from(s)]);
            }
        }
        "onDialogueCompleteLine" => {
            propagate(interp, ctx, name, &ev);
            if !canceled(&ev) {
                vcall(interp, ctx, Who::Box, "skip", vec![]);
            }
        }
        "onDialogueSkip" => {
            propagate(interp, ctx, name, &ev);
            if !canceled(&ev) {
                dispatch_all(interp, ctx, "onDialogueEnd");
            }
        }
        "onDialogueEnd" => {
            propagate(interp, ctx, name, &ev);
            if let Some(c) = conv(ctx) {
                c.state = "Ending".into();
            }
        }
        "onUpdate" => {
            propagate(interp, ctx, name, &ev);
            if canceled(&ev) {
                return Ok(Some(Value::Null));
            }
            let Some(c) = conv(ctx) else { return Ok(None) };
            match c.state.as_str() {
                "Opening" => {
                    let want = entry(c).and_then(|e| e.box_animation.clone()).unwrap_or_default();
                    // `getCurrentAnimation()` is always "" on a DialogueBox, so a box
                    // with a line animation is ready at once.
                    let ready = c.dbox.as_ref().map(|b| b.sprite.finished() || !want.is_empty());
                    if ready == Some(true) {
                        c.state = "Speaking".into();
                        let s = line_string(c);
                        vcall(interp, ctx, Who::Conv, "showCurrentSpeaker", vec![]);
                        vcall(interp, ctx, Who::Conv, "playSpeakerAnimation", vec![]);
                        vcall(interp, ctx, Who::Box, "setText", vec![Value::from(s)]);
                    }
                }
                "Ending" => {
                    if c.outro.is_none() {
                        vcall(interp, ctx, Who::Conv, "startOutro", vec![]);
                    }
                }
                _ => {}
            }
        }
        "onDestroy" => {
            propagate(interp, ctx, name, &ev);
            let audio = ctx.audio;
            if let Some(c) = conv(ctx) {
                if c.music.take().is_some() {
                    audio.set_overlay(None);
                }
                c.speaker = None;
                c.dbox = None;
                c.backdrop = None;
                c.finished = true;
            }
            // `PlayState.onConversationComplete`.
            ctx.state.start_countdown = true;
        }
        "onScriptEvent" | "onSongRetry" | "onPause" | "onResume" | "onNoteHit" | "onNoteMiss" => propagate(interp, ctx, name, &ev),
        "setupMusic" => {
            let Some(asset) = conv(ctx).and_then(|c| c.data.music.clone()).filter(|m| !m.asset.is_empty()) else { return Ok(Some(Value::Null)) };
            let level = ctx.world.as_deref().and_then(|w| w.stage.directory.clone()).unwrap_or_default();
            let root = ctx.assets.root().to_owned();
            let path = [root.join(&level).join("music").join(format!("{}.ogg", asset.asset)), ctx.assets.music_path(&asset.asset)]
                .into_iter()
                .find(|p| funkin_core::vfs::exists(p));
            match path.map(|p| Pcm::load_ogg(&p)) {
                Some(Ok(pcm)) => {
                    let track = Track::new(pcm);
                    let fade = (asset.fade_time > 0.0).then_some((0.0, 1.0, asset.fade_time, 0.0));
                    track.set_volume(if fade.is_some() { 0.0 } else { 1.0 });
                    ctx.audio.set_overlay(Some((track.clone(), 0.0)));
                    if let Some(c) = conv(ctx) {
                        c.music = Some(Music { track, fade });
                    }
                }
                Some(Err(e)) => log::warn!("dialogue music {}: {e}", asset.asset),
                None => log::warn!("dialogue music {} not found", asset.asset),
            }
        }
        "pauseMusic" | "resumeMusic" | "pause" | "resume" => {}
        "setupBackdrop" => {
            if let Some(c) = conv(ctx) {
                c.backdrop = c.data.backdrop.as_ref().filter(|b| b.kind == "solid").map(|b| (color(&b.color), b.fade_time, 0.0));
            }
        }
        "showCurrentSpeaker" => {
            let Some(c) = conv(ctx) else { return Ok(None) };
            let next = entry(c).map(|e| e.speaker.clone()).unwrap_or_default();
            if c.speaker.as_ref().is_some_and(|s| s.id == next) {
                return Ok(Some(Value::Null));
            }
            c.speaker = None;
            let data: SpeakerData = match read_json(ctx, "speakers", &next) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("speaker {next}: {e}");
                    return Ok(Some(Value::Null));
                }
            };
            let mut sprite = match load_sprite(ctx, &data.asset_path, &data.animations, data.is_pixel) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("speaker {next}: {e}");
                    return Ok(Some(Value::Null));
                }
            };
            // `screenCenter` on the unscaled frame, then the offsets and scale (`updateHitbox`).
            let [fw, fh] = sprite.size();
            sprite.x = (VIEW_W - fw) / 2.0 + data.offsets[0] as f32;
            sprite.y = (VIEW_H - fh) / 2.0 + data.offsets[1] as f32;
            sprite.scale = [data.scale as f32; 2];
            sprite.flip_x = data.flip_x;
            if let Some(c) = conv(ctx) {
                c.speaker = Some(Speaker { id: next.clone(), sprite, script: Value::Null });
            }
            let script = bind(interp, ctx, "Speaker", &next, who_ref(Who::Speaker));
            if let Some(s) = conv(ctx).and_then(|c| c.speaker.as_mut()) {
                s.script = script;
            }
            vcall(interp, ctx, Who::Speaker, "onCreate", vec![event(vec![])]);
        }
        "playSpeakerAnimation" => {
            let anim = conv(ctx).and_then(|c| entry(c).and_then(|e| e.speaker_animation.clone()));
            if let Some(anim) = anim {
                vcall(interp, ctx, Who::Speaker, "playAnimation", vec![Value::from(anim)]);
            }
        }
        "showCurrentDialogueBox" => {
            let Some(c) = conv(ctx) else { return Ok(None) };
            let next = entry(c).map(|e| e.r#box.clone()).unwrap_or_default();
            if c.dbox.as_ref().is_some_and(|b| b.id == next) {
                return Ok(Some(Value::Null));
            }
            c.dbox = None;
            let data: BoxData = match read_json(ctx, "boxes", &next) {
                Ok(d) => d,
                Err(e) => {
                    log::warn!("dialogue box {next}: {e}");
                    return Ok(Some(Value::Null));
                }
            };
            let mut sprite = match load_sprite(ctx, &data.asset_path, &data.animations, data.is_pixel) {
                Ok(s) => s,
                Err(e) => {
                    log::warn!("dialogue box {next}: {e}");
                    return Ok(Some(Value::Null));
                }
            };
            // The (empty) group is screen-centred, then offset; the box sits at its origin.
            let (gx, gy) = (VIEW_W / 2.0 + data.offsets[0] as f32, VIEW_H / 2.0 + data.offsets[1] as f32);
            sprite.x = gx;
            sprite.y = gy;
            sprite.scale = [data.scale as f32; 2];
            let t = &data.text;
            let text = TypeText {
                prefix: String::new(),
                full: Vec::new(),
                shown: 0,
                t: 0.0,
                typing: false,
                delay: 0.05,
                x: gx + t.offsets[0] as f32,
                y: gy + t.offsets[1] as f32,
                width: t.width as f32,
                size: t.size as f32,
                color: color(t.color.as_deref().unwrap_or("#000000")),
                shadow: color(t.shadow_color.as_deref().unwrap_or("#00000000")),
                shadow_w: t.shadow_width as f32,
                visible: true,
                sound: ctx.assets.sound(&ctx.sound_key("pixelText")),
                sound_id: None,
            };
            if let Some(c) = conv(ctx) {
                c.dbox = Some(DialogueBox { id: next.clone(), sprite, text, script: Value::Null, alpha: 1.0, last: (String::new(), false) });
            }
            let script = bind(interp, ctx, "DialogueBox", &next, who_ref(Who::Box));
            if let Some(b) = conv(ctx).and_then(|c| c.dbox.as_mut()) {
                b.script = script;
            }
            vcall(interp, ctx, Who::Box, "onCreate", vec![event(vec![])]);
        }
        "playDialogueBoxAnimation" => {
            let anim = conv(ctx).and_then(|c| entry(c).and_then(|e| e.box_animation.clone()));
            if let Some(anim) = anim {
                vcall(interp, ctx, Who::Box, "playAnimation", vec![Value::from(anim)]);
            }
        }
        "onTypingComplete" => {
            if let Some(c) = conv(ctx) {
                c.state = "Idle".into();
            }
        }
        "startOutro" => {
            let Some(c) = conv(ctx) else { return Ok(None) };
            match c.data.outro.clone() {
                Some(o) if o.kind == "fade" => {
                    c.outro = Some((o.fade_time, 0.0));
                    if let Some(m) = &mut c.music {
                        m.fade = Some((1.0, 0.0, o.fade_time, 0.0));
                    }
                }
                _ => end_outro(interp, ctx),
            }
        }
        "endOutro" => end_outro(interp, ctx),
        "advanceConversation" => advance(interp, ctx),
        "skipConversation" => dispatch_all(interp, ctx, "onDialogueSkip"),
        "refresh" | "kill" | "revive" | "resetConversation" => {}
        m if m.starts_with("on") => {}
        _ => return Ok(None),
    }
    Ok(Some(Value::Null))
}

fn wrap(font: Option<&TtfFont>, gfx: Option<&mut Gfx>, text: &str, size: f32, width: f32) -> Vec<char> {
    let (Some(font), Some(gfx)) = (font, gfx) else { return text.chars().collect() };
    let mut out = String::new();
    for (pi, para) in text.split('\n').enumerate() {
        if pi > 0 {
            out.push('\n');
        }
        let mut line = String::new();
        for word in para.split(' ') {
            let cand = if line.is_empty() { word.to_owned() } else { format!("{line} {word}") };
            if !line.is_empty() && font.width(gfx, &cand, size) > width {
                out.push_str(&line);
                out.push('\n');
                line = word.to_owned();
            } else {
                line = cand;
            }
        }
        out.push_str(&line);
    }
    out.chars().collect()
}

fn box_native(interp: &mut Interp, ctx: &mut Ctx, name: &str, a: Vec<Value>) -> Result<Option<Value>, String> {
    let a0 = arg(&a, 0);
    match name {
        "playAnimation" => {
            let Some(b) = conv(ctx).and_then(|c| c.dbox.as_mut()) else { return Ok(Some(Value::Null)) };
            let mut n = a0.to_string();
            // `correctAnimationName`: strip `-suffix`es, then fall back to idle.
            while !b.sprite.has_anim(&n) {
                match n.rfind('-') {
                    Some(i) => n.truncate(i),
                    None if n != "idle" => n = "idle".into(),
                    None => return Ok(Some(Value::Null)),
                }
            }
            b.sprite.play(&n, arg(&a, 1).truthy(), false);
            b.last = (n, false);
        }
        // DialogueBox reads the group's own (empty) animation, not the box sprite's.
        "getCurrentAnimation" => return Ok(Some(Value::str(""))),
        "hasAnimation" => return Ok(Some(Value::Bool(conv(ctx).and_then(|c| c.dbox.as_ref()).is_some_and(|b| b.sprite.has_anim(&a0.to_string()))))),
        "isAnimationFinished" => return Ok(Some(Value::Bool(conv(ctx).and_then(|c| c.dbox.as_ref()).is_some_and(|b| b.sprite.finished())))),
        "setText" | "appendText" => {
            let append = name == "appendText";
            let s = a0.to_string();
            let Some(c) = ctx.state.dialogue.as_mut() else { return Ok(None) };
            let font = c.font.as_ref();
            let Some(b) = c.dbox.as_mut() else { return Ok(Some(Value::Null)) };
            let t = &mut b.text;
            t.prefix = if append { t.full.iter().collect() } else { String::new() };
            let base = t.prefix.chars().count();
            let joined = if append && !t.prefix.is_empty() { format!("{} {s}", t.prefix) } else { s.clone() };
            t.full = wrap(font, ctx.gfx.as_deref_mut(), &joined, t.size, t.width);
            t.shown = base.min(t.full.len());
            t.t = 0.0;
            t.typing = true;
            if t.shown >= t.full.len() {
                t.typing = false;
                vcall(interp, ctx, Who::Box, "onTypingComplete", vec![]);
            }
        }
        "skip" => {
            let mut was = false;
            if let Some(b) = conv(ctx).and_then(|c| c.dbox.as_mut()) {
                was = b.text.typing;
                b.text.shown = b.text.full.len();
                b.text.typing = false;
            }
            if was {
                vcall(interp, ctx, Who::Box, "onTypingComplete", vec![]);
            }
        }
        // `typingCompleteCallback` is the conversation's `onTypingComplete`.
        "onTypingComplete" => {
            vcall(interp, ctx, Who::Conv, "onTypingComplete", vec![]);
        }
        "setScale" => {
            if let Some(b) = conv(ctx).and_then(|c| c.dbox.as_mut()) {
                b.sprite.scale = [a0.f() as f32; 2];
            }
        }
        "setAnimationOffsets" => {
            if let Some(b) = conv(ctx).and_then(|c| c.dbox.as_mut()) {
                b.sprite.set_anim_offsets(&a0.to_string(), arg(&a, 1).f() as f32, arg(&a, 2).f() as f32);
            }
        }
        "kill" | "revive" | "loadSpritesheet" | "loadAnimations" | "loadText" => {}
        m if m.starts_with("on") => {}
        _ => return Ok(None),
    }
    Ok(Some(Value::Null))
}

fn speaker_native(ctx: &mut Ctx, name: &str, a: Vec<Value>) -> Result<Option<Value>, String> {
    let Some(s) = conv(ctx).and_then(|c| c.speaker.as_mut()) else { return Ok(Some(Value::Null)) };
    match name {
        "playAnimation" => {
            let mut n = arg(&a, 0).to_string();
            while !s.sprite.has_anim(&n) {
                match n.rfind('-') {
                    Some(i) => n.truncate(i),
                    None if n != "idle" => n = "idle".into(),
                    None => return Ok(Some(Value::Null)),
                }
            }
            s.sprite.play(&n, arg(&a, 1).truthy(), false);
        }
        "getCurrentAnimation" => return Ok(Some(Value::str(s.sprite.current()))),
        "hasAnimation" => return Ok(Some(Value::Bool(s.sprite.has_anim(&arg(&a, 0).to_string())))),
        "isAnimationFinished" => return Ok(Some(Value::Bool(s.sprite.finished()))),
        "setScale" => s.sprite.scale = [arg(&a, 0).f() as f32; 2],
        "kill" | "revive" => {}
        m if m.starts_with("on") => {}
        _ => return Ok(None),
    }
    Ok(Some(Value::Null))
}

/// Script-visible fields.
pub fn get(ctx: &mut Ctx, kind: u32, field: &str) -> Option<Value> {
    let c = ctx.state.dialogue.as_ref()?;
    Some(match (kind, field) {
        (K_CONV, "state") => Value::str(&c.state),
        (K_CONV, "id") => Value::str(&c.id),
        (K_CONV, "alpha") => Value::Float(c.alpha as f64),
        (K_CONV, "music") => {
            if c.music.is_some() {
                Value::Host(HostRef { kind: K_CONV, id: 1 })
            } else {
                Value::Null
            }
        }
        (K_CONV, "currentSpeaker") => {
            if c.speaker.is_some() {
                Value::Host(HostRef { kind: K_SPEAKER, id: 0 })
            } else {
                Value::Null
            }
        }
        (K_CONV, "currentDialogueBox") => {
            if c.dbox.is_some() {
                Value::Host(HostRef { kind: K_DBOX, id: 0 })
            } else {
                Value::Null
            }
        }
        (K_CONV, "currentDialogueEntry") => Value::Int(c.entry as i64),
        (K_CONV, "currentDialogueLine") => Value::Int(c.line as i64),
        (K_CONV, "currentDialogueEntryCount") => Value::Int(c.data.dialogue.len() as i64),
        (K_SPEAKER, "id") => Value::str(&c.speaker.as_ref()?.id),
        (K_DBOX, "id") => Value::str(&c.dbox.as_ref()?.id),
        (K_DBOX, "textDisplay") => Value::Host(HostRef { kind: K_TEXT, id: 0 }),
        (K_DBOX, "alpha") => Value::Float(c.dbox.as_ref()?.alpha as f64),
        (K_DBOX, "x") => Value::Float(c.dbox.as_ref()?.sprite.x as f64),
        (K_DBOX, "y") => Value::Float(c.dbox.as_ref()?.sprite.y as f64),
        (K_SPEAKER, "x") => Value::Float(c.speaker.as_ref()?.sprite.x as f64),
        (K_SPEAKER, "y") => Value::Float(c.speaker.as_ref()?.sprite.y as f64),
        (K_DBOX, "boxSprite") => Value::Host(HostRef { kind: crate::scripting::K_SPRITE_PUB, id: DLG_BOX }),
        (K_TEXT, "visible") => Value::Bool(c.dbox.as_ref()?.text.visible),
        (K_TEXT, "text") => Value::from(c.dbox.as_ref()?.text.full[..c.dbox.as_ref()?.text.shown].iter().collect::<String>()),
        _ => return None,
    })
}

pub fn set(ctx: &mut Ctx, kind: u32, id: u64, field: &str, v: &Value) -> bool {
    let audio = ctx.audio;
    let Some(c) = ctx.state.dialogue.as_mut() else { return true };
    match (kind, field) {
        (K_CONV, "state") => c.state = v.to_string(),
        (K_CONV, "alpha") => c.alpha = v.f() as f32,
        (K_CONV, "music") if v.is_null() => {
            if c.music.take().is_some() {
                audio.set_overlay(None);
            }
        }
        (K_CONV, "volume") if id == 1 => {
            if let Some(m) = &c.music {
                m.track.set_volume(v.f() as f32);
            }
        }
        (K_DBOX, "alpha") => {
            if let Some(b) = &mut c.dbox {
                b.alpha = v.f() as f32;
            }
        }
        (K_DBOX, "x") | (K_DBOX, "y") => {
            if let Some(b) = &mut c.dbox {
                let d = v.f() as f32 - if field == "x" { b.sprite.x } else { b.sprite.y };
                if field == "x" {
                    b.sprite.x += d;
                    b.text.x += d;
                } else {
                    b.sprite.y += d;
                    b.text.y += d;
                }
            }
        }
        (K_TEXT, "visible") => {
            if let Some(b) = &mut c.dbox {
                b.text.visible = v.truthy();
            }
        }
        _ => return false,
    }
    true
}

/// `conversation.music.stop()` / `.destroy()` etc.
pub fn music_call(ctx: &mut Ctx, method: &str) {
    let audio = ctx.audio;
    if let Some(c) = ctx.state.dialogue.as_mut() {
        if matches!(method, "stop" | "destroy" | "pause") && c.music.take().is_some() {
            audio.set_overlay(None);
        }
    }
}

impl Conversation {
    pub fn actor(&mut self, id: u64) -> Option<&mut Actor> {
        match id {
            DLG_BOX => self.dbox.as_mut().map(|b| &mut b.sprite),
            DLG_SPEAKER => self.speaker.as_mut().map(|s| &mut s.sprite),
            _ => None,
        }
    }

    /// Draw over everything (`camCutscene`).
    pub fn draw(&self, gfx: &mut Gfx) {
        gfx.set_transform(Affine::IDENTITY);
        if let Some((col, fade, t)) = self.backdrop {
            let k = if fade > 0.0 { stepped((t / fade).min(1.0), 10.0) } else { 1.0 } as f32;
            gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [col[0], col[1], col[2], col[3] * k * self.alpha]);
        }
        if let Some(s) = &self.speaker {
            s.sprite.draw_alpha(gfx, &self.cam, self.alpha);
        }
        if let Some(b) = &self.dbox {
            let alpha = self.alpha * b.alpha;
            b.sprite.draw_alpha(gfx, &self.cam, alpha);
            let t = &b.text;
            if t.visible {
                if let Some(font) = &self.font {
                    gfx.set_transform(Affine::IDENTITY);
                    let shown: String = t.full[..t.shown.min(t.full.len())].iter().collect();
                    let lh = font.line_height(t.size);
                    for (i, line) in shown.split('\n').enumerate() {
                        let y = t.y + lh * i as f32;
                        if t.shadow[3] > 0.0 {
                            font.draw(gfx, line, t.x + t.shadow_w, y + t.shadow_w, t.size, [t.shadow[0], t.shadow[1], t.shadow[2], t.shadow[3] * alpha]);
                        }
                        font.draw(gfx, line, t.x, y, t.size, [t.color[0], t.color[1], t.color[2], t.color[3] * alpha]);
                    }
                }
            }
        }
    }
}
