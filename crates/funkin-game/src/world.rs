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
    pub(crate) hold_timer: f64,
    pub focus: [f64; 2],
    /// The character id (`characterId`).
    pub id: String,
    /// `getDeathCameraOffsets`, `getDeathCameraZoom`, `getDeathPreTransitionDelay`.
    pub death: funkin_core::stage::DeathData,
    /// Where the game over camera looks (`GameOverSubState.setCameraTarget`).
    pub death_focus: [f64; 2],
    /// `Bopper.originalPosition`: where the stage put the character
    /// (`resetPosition` returns it there).
    pub original: [f32; 2],
}

fn dir_upper(d: NoteDirection) -> &'static str {
    match d {
        NoteDirection::Left => "LEFT",
        NoteDirection::Down => "DOWN",
        NoteDirection::Up => "UP",
        NoteDirection::Right => "RIGHT",
    }
}

/// `CharacterDataParser.fetchCharacter(id)`: a character on its own, not in a
/// stage slot (scripts place it), scaled and flipped by its data and in its idle.
pub fn character_actor(gfx: &mut Gfx, assets: &Assets, id: &str) -> Result<Actor, String> {
    let data = load_character(&assets.root, id).map_err(|e| e.to_string())?;
    let mut actor = Actor::animated(gfx, assets, id, &data.render_type, &data.asset_path, None, &data.animations, !data.is_pixel)?;
    actor.should_bop = false;
    actor.char_dance = true;
    actor.set_swf_mode(true);
    actor.dance_every = data.dance_every;
    actor.global_offsets = [data.offsets[0] as f32, data.offsets[1] as f32];
    actor.flip_x = data.flip_x;
    actor.bop(true);
    actor.update_hitbox();
    let s = data.scale as f32;
    actor.scale = [s, s];
    Ok(actor)
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
        let original = [actor.x, actor.y];
        Ok(Self { actor, kind, sing_time_steps: data.sing_time, hold_timer: 0.0, focus, id: id.to_owned(), death, death_focus, original })
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

    /// `BaseCharacter.playSingAnimation(dir, miss, suffix)`.
    pub fn sing_suffix(&mut self, dir: NoteDirection, miss: bool, suffix: &str) {
        let suffix = if suffix.is_empty() { String::new() } else { format!("-{suffix}") };
        let name = format!("sing{}{}{suffix}", dir_upper(dir), if miss { "miss" } else { "" });
        self.actor.play(&name, true, false);
        self.hold_timer = 0.0;
    }

    /// `BaseCharacter.onNoteHit` / `onNoteMiss` (also ghost misses, `must_hit` true):
    /// the player's notes make BF sing, the opponent's the dad.
    pub fn on_note(&mut self, hit: bool, must_hit: bool, dir: NoteDirection, kind: &str) {
        let mine = match self.kind {
            CharType::Bf => must_hit,
            CharType::Dad => !must_hit,
            CharType::Gf => false,
        };
        if mine && !(hit && kind == "noanim") {
            self.sing(dir, !hit);
        }
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
    Trail(usize),
}

/// `FunkTrail` / `FlxTrail`: fading copies of a character's recent frames
/// (Spirit's `evilTrail`), made by a script and drawn once `add`ed.
struct Trail {
    /// Index into `chars`.
    owner: usize,
    z: i32,
    /// Seconds between saved frames.
    delay: f32,
    length: usize,
    alpha: f32,
    diff: f32,
    timer: f32,
    /// Newest first.
    poses: std::collections::VecDeque<crate::actor::Pose>,
}

/// A script's `FlxCamera`: its own scroll and zoom (the script usually copies
/// `camGame`'s), optionally following the game's follow point.
pub(crate) struct ExtraCam {
    pub cam: GameCamera,
    /// `follow(point, LOCKON, lerp)`: track `camGame`'s follow point.
    pub follow: Option<f64>,
    pub alpha: f32,
    pub visible: bool,
}

impl ExtraCam {
    pub fn new(w: f64, h: f64, zoom: f64) -> Self {
        Self { cam: GameCamera::new(zoom, w, h), follow: None, alpha: 1.0, visible: true }
    }
}

/// A character a script fetched (a prop) and gave a `characterType`: it sings
/// that side's notes like a `BaseCharacter`.
pub(crate) struct PropSinger {
    pub prop: usize,
    pub kind: CharType,
    hold: f64,
    sing_steps: f64,
}

pub struct World {
    #[allow(dead_code)]
    pub stage: StageData,
    pub(crate) props: Vec<Actor>,
    /// Props a script put on a camera other than `camGame`, with the camera:
    /// 1 `camHUD`, 2 `camCutscene`, 3+ a script's own `FlxCamera` (see `extra_cams`).
    pub(crate) screen: Vec<(usize, u64)>,
    /// Cameras scripts made (`new FlxCamera()`), camera id `3 + index`.
    pub(crate) extra_cams: Vec<ExtraCam>,
    /// `FlxG.cameras.list` order by camera id (0 game, 1 HUD, 2 cutscene, 3+ extra).
    pub(crate) cam_order: Vec<u64>,
    /// Indexed by `CharType as usize` order: bf, dad, gf.
    pub chars: [Option<Character>; 3],
    order: Vec<Slot>,
    pub camera: GameCamera,
    abot: Option<ABot>,
    /// Camera bops on the beat (`Preferences.zoomCamera`).
    pub zoom_camera: bool,
    trails: Vec<Trail>,
    pub(crate) singers: Vec<PropSinger>,
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
                Actor::animated(gfx, assets, &p.name, &p.anim_type, &p.asset_path, level, &p.animations, smooth)
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
            a.apply_stage_matrix = p.atlas_settings.apply_stage_matrix;
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

        Ok(Self { stage, props, screen: Vec::new(), extra_cams: Vec::new(), cam_order: vec![0, 1, 2], chars, order: order.into_iter().map(|(_, s)| s).collect(), camera, abot, zoom_camera: true, trails: Vec::new(), singers: Vec::new() })
    }

    /// No stage: a container for sprites outside a song (the results screen).
    pub fn empty() -> Self {
        let stage: StageData = serde_json::from_value(serde_json::json!({})).expect("empty stage data");
        Self {
            stage,
            props: Vec::new(),
            screen: Vec::new(),
            extra_cams: Vec::new(),
            cam_order: vec![0, 1, 2],
            chars: [None, None, None],
            order: Vec::new(),
            camera: GameCamera::new(1.0, 1280.0, 720.0),
            abot: None,
            zoom_camera: false,
            trails: Vec::new(),
            singers: Vec::new(),
        }
    }

    /// `Stage.refresh`: re-sort by zIndex (stable, so equal ones keep their order).
    pub fn refresh(&mut self) {
        let z = |s: &Slot, w: &World| match s {
            Slot::Prop(i) => w.props[*i].z_index,
            Slot::Char(i) => w.chars[*i].as_ref().map(|c| c.actor.z_index).unwrap_or(0),
            Slot::Trail(i) => w.trails[*i].z,
        };
        let mut order = std::mem::take(&mut self.order);
        order.sort_by_key(|s| z(s, self));
        self.order = order;
    }

    /// A sprite a script made (`new FlxSprite()`): it isn't drawn until added.
    pub fn new_prop(&mut self, a: Actor) -> usize {
        self.props.push(a);
        self.props.len() - 1
    }

    /// `new FunkTrail(character, null, length, delay, alpha, diff)`: drawn once
    /// [`World::show_trail`] adds it.
    pub fn new_trail(&mut self, owner: usize, length: usize, delay: f32, alpha: f32, diff: f32) -> usize {
        let z = self.chars.get(owner).and_then(Option::as_ref).map_or(0, |c| c.actor.z_index);
        self.trails.push(Trail { owner, z, delay, length, alpha, diff, timer: 0.0, poses: Default::default() });
        self.trails.len() - 1
    }

    pub fn trail_z(&self, i: usize) -> Option<i32> {
        self.trails.get(i).map(|t| t.z)
    }

    pub fn set_trail_z(&mut self, i: usize, z: i32) {
        if let Some(t) = self.trails.get_mut(i) {
            t.z = z;
            self.refresh();
        }
    }

    /// `add(trail)` / `remove(trail)`.
    pub fn show_trail(&mut self, i: usize, show: bool) {
        let listed = self.order.iter().any(|s| matches!(s, Slot::Trail(j) if *j == i));
        if show && !listed && i < self.trails.len() {
            self.order.push(Slot::Trail(i));
            self.refresh();
        } else if !show {
            self.order.retain(|s| !matches!(s, Slot::Trail(j) if *j == i));
        }
    }

    /// `add(sprite)`: draw a script-made prop (sorted in by zIndex).
    pub fn add_prop(&mut self, i: usize) {
        if i < self.props.len() && !self.order.iter().any(|s| matches!(s, Slot::Prop(j) if *j == i)) {
            self.order.push(Slot::Prop(i));
            self.refresh();
        }
    }

    /// `remove(sprite)`.
    pub fn remove_prop(&mut self, i: usize) {
        self.order.retain(|s| !matches!(s, Slot::Prop(j) if *j == i));
    }

    /// `character.characterType = t` on a fetched character.
    pub fn set_singer(&mut self, prop: usize, kind: CharType, sing_steps: f64) {
        match self.singers.iter_mut().find(|s| s.prop == prop) {
            Some(s) => s.kind = kind,
            None => self.singers.push(PropSinger { prop, kind, hold: 0.0, sing_steps }),
        }
    }

    /// A fetched character's `characterType` (OTHER until a script sets one).
    pub fn singer_kind(&self, prop: usize) -> Option<CharType> {
        self.singers.iter().find(|s| s.prop == prop).map(|s| s.kind)
    }

    /// Note hits and misses for [`PropSinger`]s (`BaseCharacter.onNoteHit`).
    pub fn singers_note(&mut self, hit: bool, must_hit: bool, dir: NoteDirection, kind: &str) {
        for s in &mut self.singers {
            let mine = match s.kind {
                CharType::Bf => must_hit,
                CharType::Dad => !must_hit,
                CharType::Gf => false,
            };
            let Some(a) = self.props.get_mut(s.prop) else { continue };
            if mine && a.visible && !(hit && kind == "noanim") {
                a.play(&format!("sing{}{}", dir_upper(dir), if hit { "" } else { "miss" }), true, false);
                s.hold = 0.0;
            }
        }
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
        for c in &mut self.extra_cams {
            if let Some(rate) = c.follow {
                c.cam.follow_point = self.camera.follow_point;
                c.cam.update_follow(elapsed, rate);
            }
            c.cam.update_shake(elapsed);
        }
        for p in &mut self.props {
            p.update(elapsed as f32);
        }
        for s in &mut self.singers {
            let Some(a) = self.props.get(s.prop) else { continue };
            let cur = a.current();
            if !(cur.starts_with("sing") && !cur.ends_with("-end")) {
                s.hold = 0.0;
                continue;
            }
            s.hold += elapsed;
            let mut sec = s.sing_steps * step_ms / 1000.0;
            if cur.ends_with("miss") {
                sec *= 2.0;
            }
            let held = match s.kind {
                CharType::Bf => holding[0] || keys_held,
                CharType::Dad => holding[1],
                CharType::Gf => false,
            };
            if s.hold > sec && !held {
                s.hold = 0.0;
                let end = format!("{}-end", cur.strip_suffix("-hold").unwrap_or(cur));
                let a = &mut self.props[s.prop];
                if a.has_anim(&end) {
                    a.play(&end, false, false);
                } else {
                    a.bop(false);
                }
            }
        }
        for c in self.chars.iter_mut().flatten() {
            let h = match c.kind {
                CharType::Bf => holding[0],
                CharType::Dad => holding[1],
                CharType::Gf => false,
            };
            c.update(elapsed, step_ms, h, keys_held && c.kind == CharType::Bf);
        }
        // FlxTrail: save the owner's frame every `delay`.
        for t in &mut self.trails {
            t.timer += elapsed as f32;
            if t.timer >= t.delay {
                t.timer = 0.0;
                if let Some(c) = &self.chars[t.owner] {
                    t.poses.push_front(c.actor.pose());
                    t.poses.truncate(t.length);
                }
            }
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

    /// Script props on the cutscene camera: screen space, no zoom.
    /// Draw a stage actor, with the sprite a script put inside its atlas (lip sync).
    fn draw_actor(&self, gfx: &mut Gfx, a: &Actor) {
        let (zoom, host) = (self.camera.zoom() as f32, a.linear_matrix());
        let fs: Vec<(&str, Box<dyn Fn(&mut Gfx, &crate::gfx::Affine, f32) + '_>)> = a
            .mouth
            .iter()
            .filter_map(|(kw, p)| {
                let inner = self.props.get(*p)?;
                let f: Box<dyn Fn(&mut Gfx, &crate::gfx::Affine, f32) + '_> = Box::new(move |g: &mut Gfx, m: &crate::gfx::Affine, alpha: f32| inner.draw_nested(g, m, alpha, zoom, &host));
                Some((kw.as_str(), f))
            })
            .collect();
        if fs.is_empty() {
            a.draw(gfx, &self.camera);
        } else {
            let list: Vec<(&str, crate::gfx::animate::Inject)> = fs.iter().map(|(k, f)| (*k, f.as_ref() as crate::gfx::animate::Inject)).collect();
            a.draw_with(gfx, &self.camera, &list);
        }
    }

    /// The camera a prop is on (0 = `camGame`).
    pub fn prop_cam(&self, i: usize) -> u64 {
        self.screen.iter().find(|(j, _)| *j == i).map_or(0, |(_, c)| *c)
    }

    /// Props shown on camera `cam`, in draw (zIndex) order, with their zIndex.
    pub fn props_on(&self, cam: u64) -> Vec<(i32, usize)> {
        self.order
            .iter()
            .filter_map(|s| match s {
                Slot::Prop(i) if self.prop_cam(*i) == cam => Some((self.props[*i].z_index, *i)),
                _ => None,
            })
            .collect()
    }

    /// A prop on a screen camera (`camHUD` / `camCutscene`): screen space.
    pub fn draw_screen_prop(&self, gfx: &mut Gfx, i: usize) {
        let cam = GameCamera::new(1.0, crate::gfx::VIEW_W as f64, crate::gfx::VIEW_H as f64);
        if let Some(p) = self.props.get(i) {
            p.draw(gfx, &cam);
        }
    }

    /// `camCutscene`'s script sprites.
    pub fn draw_screen(&self, gfx: &mut Gfx) {
        for (_, i) in self.props_on(2) {
            self.draw_screen_prop(gfx, i);
        }
    }

    /// The script cameras listed before (`below`) or after `camGame`, in list order.
    pub fn extra_cams_around_game(&self, below: bool) -> Vec<u64> {
        let game = self.cam_order.iter().position(|&c| c == 0).unwrap_or(0);
        let hud = self.cam_order.iter().position(|&c| c == 1).unwrap_or(self.cam_order.len());
        self.cam_order
            .iter()
            .enumerate()
            .filter(|&(k, &c)| c >= 3 && if below { k < game } else { k > game && k < hud })
            .map(|(_, &c)| c)
            .collect()
    }

    /// The script cameras listed after `camHUD`.
    pub fn extra_cams_above_hud(&self) -> Vec<u64> {
        let hud = self.cam_order.iter().position(|&c| c == 1).unwrap_or(self.cam_order.len());
        self.cam_order.iter().enumerate().filter(|&(k, &c)| c >= 3 && k > hud).map(|(_, &c)| c).collect()
    }

    /// Draw a script camera's sprites through that camera.
    /// Script camera `id`'s world-to-screen transform and alpha, when it shows.
    pub fn extra_cam_view(&self, id: u64) -> Option<(crate::gfx::Affine, f32)> {
        let c = self.extra_cams.get(id.checked_sub(3)? as usize)?;
        if !c.visible || c.alpha <= 0.0 {
            return None;
        }
        let o = c.cam.world_to_screen(0.0, 0.0, [1.0, 1.0]);
        let e = c.cam.world_to_screen(1.0, 1.0, [1.0, 1.0]);
        Some((crate::gfx::Affine([(e[0] - o[0]) as f32, 0.0, 0.0, (e[1] - o[1]) as f32, o[0] as f32, o[1] as f32]), c.alpha))
    }

    pub fn draw_extra_cam(&self, gfx: &mut Gfx, id: u64) {
        let Some(c) = self.extra_cams.get((id - 3) as usize) else { return };
        if !c.visible || c.alpha <= 0.0 {
            return;
        }
        let prev = gfx.alpha_mul;
        gfx.alpha_mul *= c.alpha;
        for (_, i) in self.props_on(id) {
            self.props[i].draw(gfx, &c.cam);
        }
        gfx.alpha_mul = prev;
    }

    pub fn draw(&self, gfx: &mut Gfx) {
        for s in &self.order {
            match s {
                Slot::Prop(i) if self.prop_cam(*i) != 0 => {}
                Slot::Prop(i) => self.draw_actor(gfx, &self.props[*i]),
                Slot::Char(i) => {
                    if let Some(c) = &self.chars[*i] {
                        self.draw_actor(gfx, &c.actor)
                    }
                }
                Slot::Trail(i) => {
                    let t = &self.trails[*i];
                    if let Some(c) = &self.chars[t.owner] {
                        // Oldest (faintest) first, so newer copies sit on top.
                        for (n, pose) in t.poses.iter().enumerate().rev() {
                            c.actor.draw_pose(gfx, &self.camera, pose, t.alpha - t.diff * n as f32);
                        }
                    }
                }
            }
        }
    }
}
