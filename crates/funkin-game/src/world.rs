//! The game world behind the HUD: a stage (`Stage.hx`), its three characters
//! (`BaseCharacter.hx`), and the game camera driven by song events.

use funkin_core::camera::GameCamera;
use funkin_core::play::NoteDirection;
use funkin_core::song::SongEventData;
use funkin_core::stage::{load_character, load_stage, StageCharacter, StageData};

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::Pcm;
use crate::gfx::Gfx;
use crate::spectrum::Analyzer;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CharType {
    Bf,
    Dad,
    Gf,
}

pub struct Character {
    pub actor: Actor,
    pub kind: CharType,
    sing_time_steps: f64,
    hold_timer: f64,
    pub focus: [f64; 2],
    /// The character id (`characterId`).
    pub id: String,
    /// `getDeathCameraOffsets`, `getDeathCameraZoom`, `getDeathPreTransitionDelay`.
    pub death: funkin_core::stage::DeathData,
    /// Where the game over camera looks (`GameOverSubState.setCameraTarget`).
    pub death_focus: [f64; 2],
}

fn dir_upper(d: NoteDirection) -> &'static str {
    match d {
        NoteDirection::Left => "LEFT",
        NoteDirection::Down => "DOWN",
        NoteDirection::Up => "UP",
        NoteDirection::Right => "RIGHT",
    }
}

impl Character {
    fn load(gfx: &mut Gfx, assets: &Assets, id: &str, kind: CharType, slot: &StageCharacter) -> Result<Self, String> {
        let data = load_character(&assets.root, id).map_err(|e| e.to_string())?;
        let mut actor = Actor::animated(
            gfx,
            assets,
            id,
            &data.render_type,
            &data.asset_path,
            None,
            &data.animations,
            !data.is_pixel,
        )?;
        actor.should_bop = false;
        actor.set_swf_mode(true);
        actor.dance_every = data.dance_every;
        actor.global_offsets = [data.offsets[0] as f32, data.offsets[1] as f32];
        actor.flip_x = if kind == CharType::Bf { !data.flip_x } else { data.flip_x };

        // resetCharacter: dance into the idle, then updateHitbox against it.
        actor.bop(true);
        actor.update_hitbox();

        // Stage.addCharacter + BaseCharacter.setScale: feet on the stage position.
        let s = (data.scale * slot.scale) as f32;
        actor.scale = [s, s];
        let [w, h] = actor.size();
        actor.x = slot.position[0] as f32 - w / 2.0 + actor.global_offsets[0];
        actor.y = slot.position[1] as f32 - h + actor.global_offsets[1];
        actor.scroll = [slot.scroll[0] as f32, slot.scroll[1] as f32];
        actor.z_index = slot.z_index;
        if let Some(a) = &data.starting_animation {
            actor.play(a, true, false);
        }

        let focus = [
            (actor.x + w / 2.0) as f64 + data.camera_offsets[0] + slot.camera_offsets[0],
            (actor.y + h / 2.0) as f64 + data.camera_offsets[1] + slot.camera_offsets[1],
        ];
        log::debug!("character {id}: size {:?} pos ({}, {}) focus {:?}", actor.size(), actor.x, actor.y, focus);
        let death = data.death.clone().unwrap_or_default();
        let death_focus = [
            focus[0] - data.camera_offsets[0].trunc() - slot.camera_offsets[0].trunc() + death.camera_offsets[0],
            focus[1] - data.camera_offsets[1].trunc() - slot.camera_offsets[1].trunc() + death.camera_offsets[1],
        ];
        Ok(Self { actor, kind, sing_time_steps: data.sing_time, hold_timer: 0.0, focus, id: id.to_owned(), death, death_focus })
    }

    fn is_singing(&self) -> bool {
        let c = self.actor.current();
        c.starts_with("sing") && !c.ends_with("-end")
    }

    /// `BaseCharacter.dance`.
    pub fn dance(&mut self, force: bool) {
        if !force {
            if self.is_singing() {
                return;
            }
            let c = self.actor.current();
            if !c.starts_with("dance") && !c.starts_with("idle") && !self.actor.finished() {
                return;
            }
        }
        self.actor.bop(false);
    }

    pub fn sing(&mut self, dir: NoteDirection, miss: bool) {
        let name = format!("sing{}{}", dir_upper(dir), if miss { "miss" } else { "" });
        self.actor.play(&name, true, false);
        self.hold_timer = 0.0;
    }

    pub fn on_step(&mut self, step: i64) {
        let de = self.actor.dance_every;
        if de > 0.0 && (step as f64).rem_euclid(de * 4.0) == 0.0 {
            self.dance(false);
        }
    }

    /// `BaseCharacter.onUpdate`. `sustaining`: this side is holding a sustain note;
    /// `keys_held`: the player is holding any note key (`isHoldingNote`).
    pub fn update(&mut self, elapsed: f64, step_ms: f64, sustaining: bool, keys_held: bool) {
        self.actor.update(elapsed as f32);
        let cur = self.actor.current().to_owned();
        if self.actor.finished() && !cur.ends_with("-hold") && self.actor.has_anim(&format!("{cur}-hold")) {
            self.actor.play(&format!("{cur}-hold"), false, false);
        }
        if sustaining {
            // PlayState keeps the hold timer at zero while a sustain is held.
            self.hold_timer = 0.0;
        }
        if self.is_singing() {
            self.hold_timer += elapsed;
            let mut sing_sec = self.sing_time_steps * step_ms / 1000.0;
            if self.actor.current().ends_with("miss") {
                sing_sec *= 2.0;
            }
            let should_stop = if self.kind == CharType::Bf { !(keys_held || sustaining) } else { true };
            if self.hold_timer > sing_sec && should_stop {
                self.hold_timer = 0.0;
                let cur = self.actor.current().to_owned();
                let base = cur.strip_suffix("-hold").unwrap_or(&cur);
                let end = format!("{base}-end");
                if self.actor.has_anim(&end) {
                    self.actor.play(&end, false, false);
                } else {
                    self.dance(true);
                }
            }
        } else {
            self.hold_timer = 0.0;
        }
    }

    /// GF reacting to the player's combo (`playComboAnimation`).
    pub fn combo(&mut self, combo: u32) {
        let n = format!("combo{combo}");
        if self.actor.has_anim(&n) {
            self.actor.play(&n, true, true);
        }
    }
}

/// Nene's A-Bot speaker (port of `NeneBaseCharacter.setupAbot` and `ABotVis`):
/// the speaker body bumps when its owner dances, the eyes follow `FocusCamera`,
/// and seven bars show the instrumental's spectrum. Indices point into `props`.
struct ABot {
    owner: usize,
    owner_bops: u32,
    body: usize,
    pupil: usize,
    bars: Vec<usize>,
    analyzer: Analyzer<7>,
}

impl ABot {
    /// Characters whose scripts add an A-Bot, with its offset from the character.
    fn offset_for(id: &str) -> Option<[f32; 2]> {
        match id {
            "otis-speaker" => Some([170.0, 455.0]),
            _ if id.starts_with("nene") && !id.contains("pixel") => Some([-95.0, 384.0]),
            _ => None,
        }
    }

    fn build(gfx: &mut Gfx, assets: &Assets, owner: usize, c: &Character, id: &str, props: &mut Vec<Actor>) -> Result<Self, String> {
        let [ox, oy] = Self::offset_for(id).ok_or("no a-bot")?;
        let a = &c.actor;
        let x = a.x + ox + a.global_offsets[0] * a.scale[0];
        let y = a.y + oy + a.global_offsets[1] * a.scale[1];
        let z = a.z_index - 10;
        let dark = id.ends_with("-dark");
        let system = if dark { "shared:characters/abot/dark/abotSystem" } else { "shared:characters/abot/abotSystem" };

        let place = |mut p: Actor, px: f32, py: f32, pz: i32| {
            p.x = px;
            p.y = py;
            p.z_index = pz;
            p
        };
        let body = Actor::animated(gfx, assets, "abot", "animateatlas", system, None, &[], true)?;
        let mut stereo = Actor::image(gfx, assets, "stereoBG", "shared:characters/abot/stereoBG", None, true)?;
        if dark {
            stereo.color = [0x61 as f32 / 255.0, 0x67 as f32 / 255.0, 0x85 as f32 / 255.0, 1.0];
        }
        let eyes = Actor::solid("eyeWhites", [1.0; 4], 160.0, 60.0);
        let pupil = Actor::animated(gfx, assets, "abotPupil", "animateatlas", "shared:characters/abot/systemEyes", None, &[], true)?;

        let base = props.len();
        props.push(place(body, x, y, z));
        props.push(place(stereo, x + 150.0, y + 30.0, z - 8));
        props.push(place(eyes, x + 40.0, y + 250.0, z - 10));
        props.push(place(pupil, x + 50.0, y + 238.0, z - 10 + 5));

        // ABotVis: bars at cumulative offsets from (abot + 207, abot + 84).
        const POS_X: [f32; 7] = [0.0, 59.0, 56.0, 66.0, 54.0, 52.0, 51.0];
        const POS_Y: [f32; 7] = [0.0, -8.0, -3.5, -0.4, 0.5, 4.7, 7.0];
        let mut bars = Vec::new();
        for i in 1..=7usize {
            let data: funkin_core::stage::AnimationData = serde_json::from_value(serde_json::json!({
                "name": "VIZ", "prefix": format!("viz{i}0"), "frameRate": 0
            }))
            .map_err(|e| e.to_string())?;
            let mut bar = Actor::animated(gfx, assets, "abotViz", "sparrow", "shared:characters/abot/aBotViz", None, &[data], true)?;
            bar.play("VIZ", true, false);
            bar.visible = false;
            if dark {
                bar.color = [0.62, 0.62, 0.72, 1.0];
            }
            let bx = x + 207.0 + POS_X[..i].iter().sum::<f32>();
            let by = y + 84.0 + POS_Y[..i].iter().sum::<f32>();
            bars.push(props.len());
            props.push(place(bar, bx, by, z - 1));
        }
        Ok(Self { owner, owner_bops: a.bops, body: base, pupil: base + 3, bars, analyzer: Analyzer::default() })
    }
}

enum Slot {
    Prop(usize),
    Char(usize),
}

pub struct World {
    #[allow(dead_code)]
    pub stage: StageData,
    props: Vec<Actor>,
    /// Indexed by `CharType as usize` order: bf, dad, gf.
    pub chars: [Option<Character>; 3],
    order: Vec<Slot>,
    pub camera: GameCamera,
    abot: Option<ABot>,
    /// Camera bops on the beat (`Preferences.zoomCamera`).
    pub zoom_camera: bool,
}

fn parse_color(s: &str) -> Option<[f32; 4]> {
    let h = s.trim().trim_start_matches('#').trim_start_matches("0x");
    let v = u32::from_str_radix(h, 16).ok()?;
    let (a, rgb) = if h.len() > 6 { ((v >> 24) as f32 / 255.0, v & 0xFF_FFFF) } else { (1.0, v) };
    Some([((rgb >> 16) & 255) as f32 / 255.0, ((rgb >> 8) & 255) as f32 / 255.0, (rgb & 255) as f32 / 255.0, a])
}

impl World {
    pub fn load(gfx: &mut Gfx, assets: &Assets, stage_id: &str, bf: &str, dad: &str, gf: &str) -> Result<Self, String> {
        let stage = load_stage(&assets.root, stage_id).map_err(|e| e.to_string())?;
        let level = stage.directory.clone();
        let level = level.as_deref();
        let mut props = Vec::new();
        for p in &stage.props {
            let smooth = !p.is_pixel;
            let actor = if p.asset_path.starts_with('#') {
                let c = parse_color(&p.asset_path).unwrap_or([0.0, 0.0, 0.0, 1.0]);
                Ok(Actor::solid(&p.name, c, p.scale[0].trunc() as f32, p.scale[1].trunc() as f32))
            } else if !p.animations.is_empty() {
                if p.anim_type == "packer" {
                    Err("packer atlases are not supported yet".to_owned())
                } else {
                    Actor::animated(gfx, assets, &p.name, &p.anim_type, &p.asset_path, level, &p.animations, smooth)
                }
            } else {
                Actor::image(gfx, assets, &p.name, &p.asset_path, level, smooth)
            };
            let mut a = match actor {
                Ok(a) => a,
                Err(e) => {
                    log::warn!("stage {stage_id}: prop {}: {e}", p.name);
                    continue;
                }
            };
            if !p.asset_path.starts_with('#') {
                a.scale = [p.scale[0] as f32, p.scale[1] as f32];
                if let Some(c) = p.color.as_deref().and_then(parse_color) {
                    a.color = c;
                }
            }
            a.x = p.position[0] as f32;
            a.y = p.position[1] as f32;
            a.alpha = p.alpha as f32;
            a.scroll = [p.scroll[0] as f32, p.scroll[1] as f32];
            a.angle = p.angle as f32;
            a.z_index = p.z_index;
            a.flip_x = p.flip_x;
            a.dance_every = p.dance_every;
            if let Some(s) = &p.starting_animation {
                a.play(s, false, false);
            }
            props.push(a);
        }

        let mut chars: [Option<Character>; 3] = [None, None, None];
        let slots = [
            (CharType::Bf, bf, stage.characters.bf.as_ref()),
            (CharType::Dad, dad, stage.characters.dad.as_ref()),
            (CharType::Gf, gf, stage.characters.gf.as_ref()),
        ];
        for (i, (kind, id, slot)) in slots.into_iter().enumerate() {
            let (Some(slot), false) = (slot, id.is_empty()) else { continue };
            match Character::load(gfx, assets, id, kind, slot) {
                Ok(c) => chars[i] = Some(c),
                Err(e) => log::warn!("character {id}: {e}"),
            }
        }

        let mut abot = None;
        for (i, id) in [bf, dad, gf].into_iter().enumerate() {
            if let (Some(c), Some(_)) = (&chars[i], ABot::offset_for(id)) {
                match ABot::build(gfx, assets, i, c, id, &mut props) {
                    Ok(a) => abot = Some(a),
                    Err(e) => log::warn!("a-bot: {e}"),
                }
                break;
            }
        }

        let mut order: Vec<(i32, Slot)> = props.iter().enumerate().map(|(i, p)| (p.z_index, Slot::Prop(i))).collect();
        for (i, c) in chars.iter().enumerate() {
            if let Some(c) = c {
                order.push((c.actor.z_index, Slot::Char(i)));
            }
        }
        order.sort_by_key(|(z, _)| *z);

        let mut camera = GameCamera::new(stage.camera_zoom, 1280.0, 720.0);
        // PlayState starts on the opponent, then snaps.
        if let Some(c) = chars[1].as_ref().or(chars[2].as_ref()).or(chars[0].as_ref()) {
            camera.follow_point = c.focus;
        }
        camera.snap();

        Ok(Self { stage, props, chars, order: order.into_iter().map(|(_, s)| s).collect(), camera, abot, zoom_camera: true })
    }

    pub fn char_mut(&mut self, kind: CharType) -> Option<&mut Character> {
        self.chars[kind as usize].as_mut()
    }

    pub fn on_step(&mut self, step: i64) {
        // `Preferences.zoomCamera` gates the beat bops.
        if self.zoom_camera {
            self.camera.on_step(step);
        }
        for p in &mut self.props {
            p.on_step(step);
        }
        for c in self.chars.iter_mut().flatten() {
            c.on_step(step);
        }
        if let Some(ab) = &mut self.abot {
            if let Some(c) = &self.chars[ab.owner] {
                if c.actor.bops != ab.owner_bops {
                    ab.owner_bops = c.actor.bops;
                    // "we start on frame 1, since from Flash the symbol has a non-bumpin frame on frame 0"
                    self.props[ab.body].play_timeline(1, None);
                }
            }
        }
    }

    /// Feed the A-Bot visualizer (`ABotVis.drawFFT`).
    pub fn update_visualizer(&mut self, pcm: &Pcm, pos_ms: f64) {
        let Some(ab) = &mut self.abot else { return };
        ab.analyzer.update(pcm, pos_ms);
        for (i, &bar) in ab.bars.iter().enumerate() {
            let level = (ab.analyzer.levels[i] * 6.0).round() as i32;
            let b = &mut self.props[bar];
            b.visible = level > 0;
            b.frame_override = Some(((level - 1).clamp(0, 5) - 5).unsigned_abs() as usize);
        }
    }

    pub fn update(&mut self, elapsed: f64, step_ms: f64, holding: [bool; 2], keys_held: bool) {
        self.camera.update(elapsed);
        for p in &mut self.props {
            p.update(elapsed as f32);
        }
        for c in self.chars.iter_mut().flatten() {
            let h = match c.kind {
                CharType::Bf => holding[0],
                CharType::Dad => holding[1],
                CharType::Gf => false,
            };
            c.update(elapsed, step_ms, h, keys_held && c.kind == CharType::Bf);
        }
    }

    /// Song events that affect the world (`FocusCamera`, `ZoomCamera`,
    /// `SetCameraBop`, `PlayAnimation`).
    pub fn handle_event(&mut self, ev: &SongEventData, step_ms: f64) {
        let num = |m: &serde_json::Map<String, serde_json::Value>, k: &str| {
            m.get(k).and_then(|v| v.as_f64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        };
        let text = |m: &serde_json::Map<String, serde_json::Value>, k: &str| m.get(k).and_then(|v| v.as_str()).map(str::to_owned);
        let ease_name = |m: &serde_json::Map<String, serde_json::Value>, default: &str| {
            let ease = text(m, "ease").unwrap_or_else(|| default.to_owned());
            let has_dir = ["In", "Out", "InOut"].iter().any(|d| ease.to_lowercase().ends_with(&d.to_lowercase()));
            let dir = if has_dir || ease == "linear" { String::new() } else { text(m, "easeDir").unwrap_or_else(|| "In".to_owned()) };
            (ease.clone(), format!("{ease}{dir}"))
        };
        match ev.event_kind.as_str() {
            "FocusCamera" => {
                let m = ev.value_as_struct("char");
                let (x, y) = (num(&m, "x").unwrap_or(0.0), num(&m, "y").unwrap_or(0.0));
                let target = num(&m, "char").unwrap_or(0.0) as i64;
                if let Some(ab) = &self.abot {
                    match target {
                        0 => self.props[ab.pupil].play_timeline(17, Some(30)),
                        1 => self.props[ab.pupil].play_timeline(0, Some(17)),
                        _ => {}
                    }
                }
                let base = match target {
                    0 => self.chars[0].as_ref().map(|c| c.focus),
                    1 => self.chars[1].as_ref().map(|c| c.focus),
                    2 => self.chars[2].as_ref().map(|c| c.focus),
                    _ => Some([0.0, 0.0]),
                };
                let Some(base) = base else { return };
                let (tx, ty) = (base[0] + x, base[1] + y);
                let (ease, full) = ease_name(&m, "CLASSIC");
                match ease.as_str() {
                    "CLASSIC" => self.camera.focus(tx, ty, None, ""),
                    "INSTANT" => self.camera.focus(tx, ty, Some(0.0), ""),
                    _ => {
                        let dur = step_ms * num(&m, "duration").unwrap_or(4.0) / 1000.0;
                        self.camera.focus(tx, ty, Some(dur), &full);
                    }
                }
            }
            "ZoomCamera" => {
                let m = ev.value_as_struct("zoom");
                let zoom = num(&m, "zoom").unwrap_or(1.0);
                let direct = text(&m, "mode").as_deref().unwrap_or("direct") == "direct";
                let (ease, full) = ease_name(&m, "linear");
                let dur = if ease == "INSTANT" { 0.0 } else { step_ms * num(&m, "duration").unwrap_or(4.0) / 1000.0 };
                self.camera.zoom_to(zoom, direct, dur, &full);
            }
            "SetCameraBop" => {
                let m = ev.value_as_struct("rate");
                self.camera.set_bop(
                    num(&m, "rate").unwrap_or(funkin_core::camera::DEFAULT_ZOOM_RATE),
                    num(&m, "offset").unwrap_or(0.0),
                    num(&m, "intensity").unwrap_or(1.0),
                );
            }
            "PlayAnimation" => {
                let m = ev.value_as_struct("anim");
                let Some(anim) = text(&m, "anim") else { return };
                let force = m.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
                let target = text(&m, "target").unwrap_or_else(|| "bf".to_owned());
                let idx = match target.as_str() {
                    "boyfriend" | "bf" | "player" => Some(0),
                    "dad" | "opponent" => Some(1),
                    "girlfriend" | "gf" => Some(2),
                    _ => None,
                };
                match idx {
                    Some(i) => {
                        if let Some(c) = &mut self.chars[i] {
                            c.actor.play(&anim, force, force);
                        }
                    }
                    None => {
                        if let Some(p) = self.props.iter_mut().find(|p| p.name == target) {
                            p.play(&anim, force, false);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        for s in &self.order {
            match s {
                Slot::Prop(i) => self.props[*i].draw(gfx, &self.camera),
                Slot::Char(i) => {
                    if let Some(c) = &self.chars[*i] {
                        c.actor.draw(gfx, &self.camera)
                    }
                }
            }
        }
    }
}
