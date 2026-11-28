//! Port of `StoryMenuState`: the level titles scrolling under the coloured
//! band, the level's props bopping to the menu music, the difficulty picker
//! and the track list. Picking a level plays its songs one after another.

use std::collections::HashMap;
use std::sync::Arc;

use funkin_core::camera::ease;
use funkin_core::constants::{DEFAULT_DIFFICULTY, DEFAULT_DIFFICULTY_LIST, DEFAULT_VARIATION};
use funkin_core::song::{LevelData, SongRegistry};
use funkin_core::stage::AnimationData;
use funkin_core::state::PlayRequest;
use serde::Deserialize;
use winit::keyboard::KeyCode;

use crate::actor::Actor;
use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Color, Gfx, VIEW_W};
use crate::play::format_thousands;

use super::freeplay::BASE_LEVELS;
use super::title::MENU_BPM;
use super::{anim, screen_camera, smooth_lerp, MenuAction};

/// `StoryMenuState.DEFAULT_BACKGROUND_COLOR`.
const DEFAULT_BG: Color = [249.0 / 255.0, 207.0 / 255.0, 81.0 / 255.0, 1.0];
const BG_Y: f32 = 56.0;
const BG_H: f32 = 400.0;
/// `tracklistText.color`.
const TRACKS_COLOR: Color = [229.0 / 255.0, 87.0 / 255.0, 119.0 / 255.0, 1.0];
const FLASH_COLOR: Color = [0.2, 1.0, 1.0, 1.0];
const WHITE: Color = [1.0, 1.0, 1.0, 1.0];
/// `selectLevel`: the timer before the camera fade, and the fade itself.
const CONFIRM_WAIT: f32 = 1.0;
const CONFIRM_FADE: f32 = 0.2;

fn one() -> f32 {
    1.0
}
fn one_f64() -> f64 {
    1.0
}

/// Port of `LevelPropData`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PropData {
    asset_path: String,
    #[serde(default = "one")]
    scale: f32,
    #[serde(default)]
    offsets: [f32; 2],
    #[serde(default)]
    animations: Vec<AnimationData>,
    #[serde(default)]
    is_pixel: bool,
    #[serde(default = "one")]
    alpha: f32,
    #[serde(default)]
    flip_x: bool,
    #[serde(default = "one_f64")]
    dance_every: f64,
    #[serde(default)]
    starting_animation: String,
}

/// `FlxColor.fromString("#RRGGBB")`.
fn parse_color(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    let v = u32::from_str_radix(h, 16).ok()?;
    let v = if h.len() == 8 { v & 0xFF_FFFF } else { v };
    Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0])
}

struct Level {
    data: LevelData,
    /// `LevelTitle`: the title graphic, its y and where it is headed.
    title: Option<Actor>,
    y: f32,
    target_y: f32,
    /// `getDifficulties()` limited to the default list ("For now, NO erect in story mode").
    diffs: Vec<String>,
    /// `getSongDisplayNames`.
    names: Vec<String>,
    /// The first song's variation for each difficulty (`getFirstValidVariation`).
    variations: HashMap<String, String>,
    /// A solid colour background, or an image.
    color: Option<Color>,
    props: Vec<(String, PropData)>,
}

impl Level {
    fn title_h(&self) -> f32 {
        self.title.as_ref().map(|t| t.size()[1]).unwrap_or(0.0)
    }
}

pub struct StoryMenu {
    levels: Vec<Level>,
    selected: usize,
    diff: String,
    time: f32,
    /// Shown props: their JSON (to reuse an unchanged prop, as `buildProps` does) and sprite.
    props: Vec<(String, Actor)>,
    prop_cache: HashMap<String, Actor>,
    last_step: Option<i64>,
    /// Background colour tween (`FlxTween.color`, 0.9 s quartOut): from, to, start.
    bg: (Color, Color, f32),
    /// Image backgrounds by level, and crossfades (`alpha` tweens, 0.6 s).
    bg_images: HashMap<String, Actor>,
    bg_fade: Option<(usize, f32)>,
    arrows: [Actor; 2],
    held: [bool; 2],
    diff_sprites: HashMap<String, Actor>,
    /// When the difficulty sprite was rebuilt (its 0.07 s drop-in).
    diff_at: f32,
    high_score: i64,
    score_lerp: f32,
    scores: HashMap<String, i64>,
    selected_at: Option<f32>,
    exiting: bool,
    sfx_scroll: Option<Arc<Pcm>>,
    sfx_confirm: Option<Arc<Pcm>>,
    sfx_cancel: Option<Arc<Pcm>>,
    gfx_assets: Assets,
}

impl StoryMenu {
    /// `remembered` is (`rememberedLevelId`, `rememberedDifficulty`);
    /// `scores` the saved level scores keyed `level:difficulty`.
    pub fn new(
        gfx: &mut Gfx,
        assets: &Assets,
        registry: &SongRegistry,
        remembered: Option<(String, String)>,
        scores: HashMap<String, i64>,
    ) -> Result<Self, String> {
        // `listSortedLevelIds`: base levels first, then alphabetically; hidden ones left out.
        let mut datas: Vec<LevelData> = registry.load_levels().into_iter().filter(|l| l.visible).collect();
        datas.sort_by_key(|l| (BASE_LEVELS.iter().position(|b| *b == l.id).unwrap_or(BASE_LEVELS.len()), l.id.clone()));
        if datas.is_empty() {
            return Err("no levels in data/levels".into());
        }
        let mut levels = Vec::new();
        for data in datas {
            let title = match Actor::image(gfx, assets, &data.id, &format!("default:{}", data.title_asset), None, true) {
                Ok(t) => Some(t),
                Err(e) => {
                    log::warn!("story title {}: {e}", data.id);
                    None
                }
            };
            let mut diffs: Vec<String> = Vec::new();
            let mut variations = HashMap::new();
            if let Some(first) = data.songs.first() {
                for var in [DEFAULT_VARIATION, "erect"] {
                    if let Ok(m) = registry.load_metadata(first, var) {
                        for d in &m.play_data.difficulties {
                            if !diffs.contains(d) {
                                diffs.push(d.clone());
                                variations.insert(d.clone(), var.to_owned());
                            }
                        }
                    }
                }
            }
            diffs.retain(|d| DEFAULT_DIFFICULTY_LIST.contains(&d.as_str()));
            diffs.sort_by_key(|d| DEFAULT_DIFFICULTY_LIST.iter().position(|x| x == d));
            if diffs.is_empty() {
                diffs.push(DEFAULT_DIFFICULTY.into());
            }
            let names = data
                .songs
                .iter()
                .map(|s| registry.load_metadata(s, DEFAULT_VARIATION).map(|m| m.song_name).unwrap_or_else(|_| "Unknown".into()))
                .collect();
            let props = match &data.props {
                serde_json::Value::Array(a) => a
                    .iter()
                    .filter_map(|v| match serde_json::from_value::<PropData>(v.clone()) {
                        Ok(p) => Some((v.to_string(), p)),
                        Err(e) => {
                            log::warn!("story prop in {}: {e}", data.id);
                            None
                        }
                    })
                    .collect(),
                _ => Vec::new(),
            };
            let color = if data.background.starts_with('#') { Some(parse_color(&data.background).unwrap_or(DEFAULT_BG)) } else { None };
            levels.push(Level { data, title, y: BG_Y + BG_H + 10.0, target_y: 0.0, diffs, names, variations, color, props });
        }
        // Image backgrounds (levels whose `background` isn't a colour).
        let mut bg_images = HashMap::new();
        for l in &levels {
            if l.color.is_none() {
                match Actor::image(gfx, assets, &l.data.id, &format!("default:{}", l.data.background), None, true) {
                    Ok(mut a) => {
                        a.y = BG_Y;
                        bg_images.insert(l.data.id.clone(), a);
                    }
                    Err(e) => log::warn!("story background {}: {e}", l.data.id),
                }
            }
        }
        // Initial `targetY = (height + 20) * index`, until `changeLevel` repositions them.
        let mut acc = 0.0;
        for l in &mut levels {
            l.target_y = acc;
            acc += l.title_h() + 20.0;
        }

        let arrow = |gfx: &mut Gfx, side: &str, x: f32| -> Result<Actor, String> {
            let mut a = Actor::animated(
                gfx,
                assets,
                "arrow",
                "sparrow",
                "default:storymenu/ui/arrows",
                None,
                &[anim("idle", &format!("{side}Idle0"), 24.0, false, &[]), anim("press", &format!("{side}Confirm0"), 24.0, false, &[])],
                true,
            )?;
            a.x = x;
            a.y = 480.0;
            a.play("idle", true, false);
            Ok(a)
        };
        let arrows = [arrow(gfx, "left", VIEW_W - 410.0)?, arrow(gfx, "right", VIEW_W - 35.0)?];

        let mut diff_sprites = HashMap::new();
        for d in DEFAULT_DIFFICULTY_LIST {
            match Actor::image(gfx, assets, d, &format!("default:storymenu/difficulties/{d}"), None, true) {
                Ok(a) => {
                    diff_sprites.insert(d.to_owned(), a);
                }
                Err(e) => log::warn!("story difficulty {d}: {e}"),
            }
        }

        let (level_id, diff) = remembered.unwrap_or_else(|| ("tutorial".into(), DEFAULT_DIFFICULTY.into()));
        let selected = levels.iter().position(|l| l.data.id == level_id).unwrap_or(0);
        let mut m = Self {
            bg: (DEFAULT_BG, DEFAULT_BG, 0.0),
            levels,
            selected,
            diff,
            time: 0.0,
            props: Vec::new(),
            prop_cache: HashMap::new(),
            last_step: None,
            bg_images,
            bg_fade: None,
            arrows,
            held: [false; 2],
            diff_sprites,
            diff_at: 0.0,
            high_score: 0,
            score_lerp: 12_345_678.0,
            scores,
            selected_at: None,
            exiting: false,
            sfx_scroll: assets.sound("default:scrollMenu"),
            sfx_confirm: assets.sound("default:confirmMenu"),
            sfx_cancel: assets.sound("default:cancelMenu"),
            gfx_assets: assets.clone(),
        };
        let c = m.levels[selected].color.unwrap_or(DEFAULT_BG);
        m.bg = (c, c, 0.0);
        m.change_difficulty(None, 0);
        m.change_level(gfx, None, 0);
        Ok(m)
    }

    /// (`rememberedLevelId`, `rememberedDifficulty`).
    pub fn remembered(&self) -> (String, String) {
        (self.levels[self.selected].data.id.clone(), self.diff.clone())
    }

    fn sfx(&self, audio: Option<&Audio>, s: &Option<Arc<Pcm>>, v: f32) {
        if let (Some(a), Some(s)) = (audio, s) {
            a.play_sfx(s.clone(), v);
        }
    }

    /// `changeLevel`.
    fn change_level(&mut self, gfx: &mut Gfx, audio: Option<&Audio>, by: i32) {
        let n = self.levels.len() as i32;
        let prev = self.selected;
        self.selected = (self.selected as i32 + by).rem_euclid(n) as usize;
        if self.selected != prev {
            self.sfx(audio, &self.sfx_scroll, 0.4);
        }
        // repositionTitles: the selected title at y 480, the rest stacked around it.
        let i = self.selected;
        self.levels[i].target_y = 480.0;
        for k in (0..i).rev() {
            let h = self.levels[k].title_h();
            self.levels[k].target_y = self.levels[k + 1].target_y - (h + 20.0).max(125.0);
        }
        for k in i + 1..self.levels.len() {
            let h = self.levels[k - 1].title_h();
            self.levels[k].target_y = self.levels[k - 1].target_y + h + 20.0;
        }
        self.update_text();
        // updateBackground.
        if prev != self.selected {
            let (a, b) = (self.levels[prev].color, self.levels[self.selected].color);
            match (a, b) {
                (Some(_), Some(to)) => {
                    let now = self.bg_color();
                    if now != to {
                        self.bg = (now, to, self.time);
                    }
                }
                _ => self.bg_fade = Some((prev, self.time)),
            }
        }
        self.update_props(gfx);
    }

    /// `changeDifficulty`.
    fn change_difficulty(&mut self, audio: Option<&Audio>, by: i32) {
        let list = &self.levels[self.selected].diffs;
        let n = list.len() as i32;
        let cur = list.iter().position(|d| *d == self.diff).map(|i| i as i32).unwrap_or(-1);
        let next = (cur + by).rem_euclid(n.max(1)) as usize;
        let next = list[next.min(list.len() - 1)].clone();
        if next != self.diff {
            self.diff = next;
            self.diff_at = self.time;
            self.sfx(audio, &self.sfx_scroll, 0.4);
        }
        self.update_text();
    }

    fn update_text(&mut self) {
        let key = format!("{}:{}", self.levels[self.selected].data.id, self.diff);
        self.high_score = self.scores.get(&key).copied().unwrap_or(0);
    }

    /// `Level.buildProps`: reuse unchanged props (so their animation carries on),
    /// build the others.
    fn update_props(&mut self, gfx: &mut Gfx) {
        let wanted = self.levels[self.selected].props.clone();
        let mut old: Vec<Option<(String, Actor)>> = std::mem::take(&mut self.props).into_iter().map(Some).collect();
        for (i, (key, pd)) in wanted.iter().enumerate() {
            let keep = old.get(i).and_then(|o| o.as_ref()).is_some_and(|(k, _)| k == key);
            let actor = if keep {
                old[i].take().map(|(_, a)| a)
            } else if let Some(a) = self.prop_cache.remove(key) {
                Some(a)
            } else {
                match build_prop(gfx, &self.gfx_assets, pd) {
                    Ok(a) => Some(a),
                    Err(e) => {
                        log::warn!("story prop {}: {e}", pd.asset_path);
                        None
                    }
                }
            };
            if let Some(mut a) = actor {
                // Scaled about the centre (no updateHitbox), where Actor scales from the top-left.
                let s = a.scale[0];
                let hw = a.size()[0] / s.abs().max(1e-6);
                a.x = pd.offsets[0] + VIEW_W * 0.25 * i as f32 + (hw - hw * s) / 2.0;
                let hh = a.size()[1] / s.abs().max(1e-6);
                a.y = pd.offsets[1] + (hh - hh * s) / 2.0;
                self.props.push((key.clone(), a));
            }
        }
        for (k, a) in old.into_iter().flatten() {
            self.prop_cache.insert(k, a);
        }
    }

    fn bg_color(&self) -> Color {
        let (from, to, start) = self.bg;
        let k = ((self.time - start) / 0.9).clamp(0.0, 1.0);
        let k = ease("quartOut", k as f64) as f32;
        std::array::from_fn(|i| from[i] + (to[i] - from[i]) * k)
    }

    /// A key went down (`pressed`) or up. `code` is the menu key it maps to.
    pub fn key(&mut self, gfx: &mut Gfx, code: KeyCode, pressed: bool, audio: &Audio) -> MenuAction {
        let side = match code {
            KeyCode::ArrowLeft => Some(0),
            KeyCode::ArrowRight => Some(1),
            _ => None,
        };
        if let Some(s) = side {
            self.held[s] = pressed && self.selected_at.is_none();
        }
        if !pressed || self.exiting {
            return MenuAction::None;
        }
        let audio = Some(audio);
        match code {
            KeyCode::ArrowUp | KeyCode::ArrowDown if self.selected_at.is_none() => {
                self.change_level(gfx, audio, if code == KeyCode::ArrowUp { -1 } else { 1 });
                self.change_difficulty(audio, 0);
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight if self.selected_at.is_none() => {
                self.change_difficulty(audio, if code == KeyCode::ArrowLeft { -1 } else { 1 });
            }
            KeyCode::Enter if self.selected_at.is_none() => {
                // selectLevel.
                self.sfx(audio, &self.sfx_confirm, 1.0);
                self.selected_at = Some(self.time);
                self.held = [false; 2];
                for (_, p) in &mut self.props {
                    if p.has_anim("confirm") {
                        p.play("confirm", true, true);
                    }
                }
            }
            KeyCode::Escape if self.selected_at.is_none() => {
                // goBack.
                self.exiting = true;
                self.sfx(audio, &self.sfx_cancel, 1.0);
                return MenuAction::CloseStory;
            }
            _ => {}
        }
        MenuAction::None
    }

    pub fn update(&mut self, dt: f32, music_ms: f64) -> MenuAction {
        self.time += dt;
        // highScoreLerp.
        self.score_lerp = smooth_lerp(self.score_lerp, self.high_score as f32, dt, 0.307);
        if (self.score_lerp - self.high_score as f32).abs() < 1.0 {
            self.score_lerp = self.high_score as f32;
        }
        // LevelTitle.update: ease toward targetY; the picked one flashes at 20 fps.
        let flashing = self.selected_at.map(|at| ((self.time - at) / 0.05) as i64 % 2 == 1).unwrap_or(false);
        for (i, l) in self.levels.iter_mut().enumerate() {
            l.y = smooth_lerp(l.y, l.target_y, dt, 0.451);
            if let Some(t) = &mut l.title {
                t.x = (VIEW_W - t.size()[0]) / 2.0;
                t.y = l.y;
                t.color = if i == self.selected && flashing { FLASH_COLOR } else { WHITE };
            }
        }
        // buildDifficultySprite: centred on normal's width, dropping in over 0.07 s.
        let nsize = self.diff_sprites.get(DEFAULT_DIFFICULTY).map(|a| a.size());
        let left = (self.arrows[0].x + self.arrows[0].size()[0], self.arrows[0].y);
        if let (Some(spr), Some([nw, nh])) = (self.diff_sprites.get_mut(&self.diff), nsize) {
            let [w, h] = spr.size();
            let k = ((self.time - self.diff_at) / 0.07).clamp(0.0, 1.0);
            let y0 = left.1 - 15.0;
            let y1 = left.1 + 10.0 - (h - nh) / 2.0;
            spr.x = left.0 + 10.0 + (nw - w) / 2.0;
            spr.y = y0 + (y1 - y0) * k;
        }
        for (i, a) in self.arrows.iter_mut().enumerate() {
            a.play(if self.held[i] { "press" } else { "idle" }, false, false);
            a.update(dt);
        }
        // The props bop to freakyMenu (Conductor at the menu BPM).
        let step = (music_ms / (60_000.0 / MENU_BPM / 4.0)).floor() as i64;
        if let Some(last) = self.last_step {
            if step > last {
                for s in (last + 1)..=step.min(last + 4) {
                    for (_, p) in &mut self.props {
                        p.on_step(s);
                    }
                }
            }
        }
        self.last_step = Some(step);
        for (_, p) in &mut self.props {
            p.update(dt);
        }
        if let Some(at) = self.selected_at {
            if self.time - at >= CONFIRM_WAIT + CONFIRM_FADE {
                self.selected_at = None;
                self.exiting = true;
                let l = &self.levels[self.selected];
                let mut songs = l.data.songs.clone();
                if songs.is_empty() {
                    return MenuAction::None;
                }
                let first = songs.remove(0);
                return MenuAction::Play(PlayRequest {
                    song_id: first,
                    difficulty: self.diff.clone(),
                    variation: l.variations.get(&self.diff).cloned().unwrap_or_else(|| DEFAULT_VARIATION.into()),
                    playlist: songs,
                    is_story: true,
                    level_id: Some(l.data.id.clone()),
                });
            }
        }
        MenuAction::None
    }

    fn draw_bg(&self, gfx: &mut Gfx, level: usize, alpha: f32) {
        let cam = screen_camera(0.0, 0.0);
        let l = &self.levels[level];
        match l.color {
            Some(_) => {
                let mut c = self.bg_color();
                c[3] = alpha;
                gfx.rect(0.0, BG_Y, VIEW_W, BG_H, c);
            }
            None => {
                if let Some(a) = self.bg_images.get(&l.data.id) {
                    a.draw_alpha(gfx, &cam, alpha);
                }
            }
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, vcr: Option<&TtfFont>) {
        gfx.begin([0.0, 0.0, 0.0, 1.0]);
        let cam = screen_camera(0.0, 0.0);
        let level = &self.levels[self.selected];

        // Difficulty arrows (hidden with a single difficulty) and the difficulty sprite.
        let many = level.diffs.len() > 1;
        if many {
            for a in &self.arrows {
                a.draw(gfx, &cam);
            }
        }
        if let (Some(spr), Some(normal)) = (self.diff_sprites.get(&self.diff), self.diff_sprites.get(DEFAULT_DIFFICULTY)) {
            let _ = normal;
            let k = ((self.time - self.diff_at) / 0.07).clamp(0.0, 1.0);
            spr.draw_alpha(gfx, &cam, k);
        }

        if let Some(f) = vcr {
            // Track list, centred text shifted left by a third of the screen.
            let mut lines = vec!["TRACKS".to_owned(), String::new()];
            lines.extend(level.names.iter().map(|n| n.to_uppercase()));
            let widths: Vec<f32> = lines.iter().map(|s| f.width(gfx, s, 32.0)).collect();
            let w = widths.iter().cloned().fold(0.0, f32::max) + 4.0;
            let x = (VIEW_W - w) / 2.0 - VIEW_W * 0.33;
            let lh = f.line_height(32.0);
            for (i, s) in lines.iter().enumerate() {
                f.draw(gfx, s, x + (w - 4.0 - widths[i]) / 2.0, BG_Y + BG_H + 100.0 + lh * i as f32, 32.0, TRACKS_COLOR);
            }
        }

        // Level titles (under the band; only the part below it shows).
        for (i, l) in self.levels.iter().enumerate() {
            let Some(t) = &l.title else { continue };
            if l.y > 720.0 || l.y + l.title_h() < BG_Y + BG_H {
                continue;
            }
            t.draw_alpha(gfx, &cam, if i == self.selected { 1.0 } else { 0.6 });
        }

        // The black strip above the band, then the band itself.
        gfx.rect(0.0, 0.0, VIEW_W, BG_H + BG_Y, [0.0, 0.0, 0.0, 1.0]);
        match self.bg_fade {
            Some((prev, at)) if self.time - at < 0.6 => {
                let k = (self.time - at) / 0.6;
                self.draw_bg(gfx, prev, 1.0 - k);
                self.draw_bg(gfx, self.selected, k);
            }
            _ => self.draw_bg(gfx, self.selected, 1.0),
        }

        for (_, p) in &self.props {
            p.draw(gfx, &cam);
        }

        if let Some(f) = vcr {
            let score = format!("LEVEL SCORE: {}", format_thousands(self.score_lerp.round() as i64));
            f.draw(gfx, &score, 10.0, 10.0, 32.0, WHITE);
            let name = &level.data.name;
            let w = f.width(gfx, name, 32.0) + 4.0;
            f.draw(gfx, name, VIEW_W - (w + 10.0), 10.0, 32.0, [1.0, 1.0, 1.0, 0.7]);
        }

        // `FlxG.camera.fade(BLACK, 0.2)` after the confirm timer.
        if let Some(at) = self.selected_at {
            let k = ((self.time - at - CONFIRM_WAIT) / CONFIRM_FADE).clamp(0.0, 1.0);
            if k > 0.0 {
                gfx.rect(0.0, 0.0, VIEW_W, 720.0, [0.0, 0.0, 0.0, k]);
            }
        }
    }
}

/// `LevelProp.applyData`.
fn build_prop(gfx: &mut Gfx, assets: &Assets, pd: &PropData) -> Result<Actor, String> {
    let spec = format!("default:{}", pd.asset_path);
    let smooth = !pd.is_pixel;
    let mut a = if pd.animations.is_empty() {
        Actor::image(gfx, assets, &pd.asset_path, &spec, None, smooth)?
    } else {
        Actor::animated(gfx, assets, &pd.asset_path, "sparrow", &spec, None, &pd.animations, smooth)?
    };
    let s = pd.scale * if pd.is_pixel { 6.0 } else { 1.0 };
    a.scale = [s, s];
    a.alpha = pd.alpha;
    a.flip_x = pd.flip_x;
    a.dance_every = pd.dance_every;
    if !pd.starting_animation.is_empty() && a.has_anim(&pd.starting_animation) {
        a.play(&pd.starting_animation, true, false);
    } else {
        a.bop(true);
    }
    Ok(a)
}
