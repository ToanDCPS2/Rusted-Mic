//! The mod menu: Tab on the main menu lists what is in the `mods` folder next
//! to the executable, with each mod's icon; picking one shows its metadata.
//!
//! Two layouts are understood:
//! * V-Slice / Polymod: `_polymod_meta.json` and `_polymod_icon.png`;
//! * Psych Engine: `pack.json` and `pack.png` (a horizontal strip of square
//!   frames is animated at `iconFramerate`).
//!
//! Any other folder is still listed, under its folder name.
//!
//! V-Slice mods load unless switched off here (Space, or the switch in the
//! details panel), in the load order set here ([ and ], or the buttons in the
//! details panel; later mods override earlier ones). Both are kept in
//! `mods.json` next to `settings.json`. Leaving the menu with a different set
//! of mods (or order) restarts the game, so everything loads with it.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use winit::keyboard::KeyCode;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Color, Gfx, MaskOp, TexId, VIEW_H, VIEW_W};

use super::{Fonts, MenuAction};

/// The `mods` folder next to the executable.
pub fn mods_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("mods")
}

#[derive(serde::Serialize, Deserialize, Default)]
#[serde(default)]
struct ModList {
    /// Folder names of the mods switched off.
    disabled: Vec<String>,
    /// Load order, by folder name (first loads first; later ones win).
    /// Mods not listed load after these, by folder name.
    order: Vec<String>,
}

fn read_mod_list() -> ModList {
    std::fs::read_to_string(mod_list_path()).ok().and_then(|t| serde_json::from_str::<ModList>(&t).ok()).unwrap_or_default()
}

fn write_mod_list(l: &ModList) {
    let p = mod_list_path();
    let r = p.parent().map(std::fs::create_dir_all).unwrap_or(Ok(())).and_then(|_| std::fs::write(&p, serde_json::to_string_pretty(l).unwrap_or_default()));
    if let Err(e) = r {
        log::warn!("{}: {e}", p.display());
    }
}

/// Where a folder sits in the saved load order (unlisted: after all of them).
fn order_pos(order: &[String], folder: &str) -> usize {
    let f = folder.to_lowercase();
    order.iter().position(|o| o.to_lowercase() == f).unwrap_or(usize::MAX)
}

fn mod_list_path() -> PathBuf {
    crate::settings::data_dir().join("mods.json")
}

/// The folders switched off in the mod menu.
pub fn disabled_mods() -> std::collections::HashSet<String> {
    read_mod_list().disabled.into_iter().collect()
}

fn save_disabled(d: &std::collections::HashSet<String>) {
    let mut l = read_mod_list();
    l.disabled = d.iter().cloned().collect();
    l.disabled.sort();
    write_mod_list(&l);
}

fn save_order(order: Vec<String>) {
    let mut l = read_mod_list();
    l.order = order;
    write_mod_list(&l);
}

/// The V-Slice mods to layer over the assets, in load order (by folder name),
/// leaving out those switched off in the menu.
/// `RUSTED_MIC_MODS=a,b` picks folders by name instead; `none` loads nothing.
pub fn enabled_mod_dirs() -> Vec<PathBuf> {
    let only: Option<Vec<String>> = std::env::var("RUSTED_MIC_MODS").ok().map(|v| v.split(',').map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty()).collect());
    let list = read_mod_list();
    let disabled: std::collections::HashSet<String> = if only.is_none() { list.disabled.into_iter().collect() } else { Default::default() };
    let Ok(rd) = std::fs::read_dir(mods_dir()) else { return Vec::new() };
    let mut dirs: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("_polymod_meta.json").is_file())
        .filter(|p| {
            let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
            only.as_ref().is_none_or(|o| o.contains(&name)) && !disabled.iter().any(|d| d.to_lowercase() == name)
        })
        .collect();
    let folder = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    dirs.sort_by_key(|p| (order_pos(&list.order, &folder(p)), folder(p).to_lowercase()));
    dirs
}

/// The menu's order: V-Slice mods in load order, then the rest by title.
fn sort_for_menu(mods: &mut [ModInfo], order: &[String]) {
    mods.sort_by_key(|m| {
        let v = m.kind == ModKind::VSlice;
        (!v, if v { order_pos(order, &m.folder) } else { 0 }, if v { m.folder.to_lowercase() } else { m.title.to_lowercase() })
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModKind {
    VSlice,
    Psych,
    Unknown,
}

impl ModKind {
    fn label(self) -> &'static str {
        match self {
            ModKind::VSlice => "V-Slice",
            ModKind::Psych => "Psych Engine",
            ModKind::Unknown => "Unknown format",
        }
    }
}

/// What the menu shows about one mod.
#[derive(Debug, Clone)]
pub struct ModInfo {
    pub folder: String,
    pub kind: ModKind,
    pub title: String,
    pub id: Option<String>,
    pub description: String,
    pub author: Option<String>,
    pub version: Option<String>,
    pub api_version: Option<String>,
    pub license: Option<String>,
    /// (name, role)
    pub contributors: Vec<(String, Option<String>)>,
    pub color: Option<Color>,
    pub icon_path: Option<PathBuf>,
    pub icon_fps: f32,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PolymodMeta {
    title: Option<String>,
    id: Option<String>,
    description: Option<String>,
    author: Option<String>,
    mod_version: Option<String>,
    api_version: Option<String>,
    license: Option<String>,
    contributors: Vec<PolymodContributor>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PolymodContributor {
    name: String,
    role: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct PsychPack {
    name: Option<String>,
    description: Option<String>,
    color: Option<Vec<f32>>,
    icon_framerate: Option<f32>,
}

/// Read one mod folder's metadata (no textures).
pub fn read_mod(dir: &Path) -> ModInfo {
    let folder = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut m = ModInfo {
        folder: folder.clone(),
        kind: ModKind::Unknown,
        title: folder,
        id: None,
        description: String::new(),
        author: None,
        version: None,
        api_version: None,
        license: None,
        contributors: Vec::new(),
        color: None,
        icon_path: None,
        icon_fps: 10.0,
    };
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).ok();
    let nonempty = |s: Option<String>| s.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    if let Some(text) = read("_polymod_meta.json") {
        m.kind = ModKind::VSlice;
        match serde_json::from_str::<PolymodMeta>(&text) {
            Ok(p) => {
                if let Some(t) = nonempty(p.title) {
                    m.title = t;
                }
                m.id = nonempty(p.id);
                m.description = p.description.unwrap_or_default();
                m.author = nonempty(p.author);
                m.version = nonempty(p.mod_version);
                m.api_version = nonempty(p.api_version);
                m.license = nonempty(p.license);
                m.contributors = p.contributors.into_iter().filter(|c| !c.name.is_empty()).map(|c| (c.name, nonempty(c.role))).collect();
            }
            Err(e) => log::warn!("{}: _polymod_meta.json: {e}", dir.display()),
        }
        m.icon_path = Some(dir.join("_polymod_icon.png")).filter(|p| p.is_file());
    } else if let Some(text) = read("pack.json") {
        m.kind = ModKind::Psych;
        match serde_json::from_str::<PsychPack>(&text) {
            Ok(p) => {
                if let Some(t) = nonempty(p.name) {
                    m.title = t;
                }
                m.description = p.description.unwrap_or_default();
                if let Some([r, g, b, ..]) = p.color.as_deref() {
                    m.color = Some([r / 255.0, g / 255.0, b / 255.0, 1.0]);
                }
                if let Some(f) = p.icon_framerate.filter(|f| *f > 0.0) {
                    m.icon_fps = f;
                }
            }
            Err(e) => log::warn!("{}: pack.json: {e}", dir.display()),
        }
        m.icon_path = Some(dir.join("pack.png")).filter(|p| p.is_file());
    }
    m
}

/// Every mod folder in `dir`, sorted by title.
pub fn scan(dir: &Path) -> Vec<ModInfo> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut mods: Vec<ModInfo> = entries.flatten().filter(|e| e.path().is_dir()).map(|e| read_mod(&e.path())).collect();
    mods.sort_by_key(|m| m.title.to_lowercase());
    mods
}

const PAD: f32 = 32.0;
const TOP: f32 = 120.0;
const BOTTOM: f32 = 52.0;
const CELL_W: f32 = 176.0;
const CELL_H: f32 = 212.0;
const ICON: f32 = 144.0;
/// The details panel's left edge.
const PANEL_X: f32 = 676.0;
const ACCENT: Color = [0.55, 0.36, 1.0, 1.0];

struct Icon {
    tex: TexId,
    /// Source rectangle of frame 0, and how many frames follow it.
    frame: [f32; 4],
    frames: u32,
}

pub struct ModMenu {
    dir: PathBuf,
    mods: Vec<ModInfo>,
    icons: Vec<Option<Icon>>,
    pub tex_mark: usize,
    selected: usize,
    /// The mod whose details are shown.
    open: Option<usize>,
    hover: Option<usize>,
    scroll: f32,
    time: f32,
    closing: Option<f32>,
    scroll_sfx: Option<std::sync::Arc<Pcm>>,
    confirm_sfx: Option<std::sync::Arc<Pcm>>,
    cancel_sfx: Option<std::sync::Arc<Pcm>>,
    /// Folders switched off.
    disabled: std::collections::HashSet<String>,
    /// A mod was switched on or off, or moved, since the menu opened.
    pub changed: bool,
    /// The mods that were loaded when the menu opened (a different set on
    /// leaving restarts the game).
    pub loaded: Vec<PathBuf>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl ModMenu {
    pub fn new(gfx: &mut Gfx, assets: &Assets) -> Self {
        let dir = mods_dir();
        let mut mods = scan(&dir);
        sort_for_menu(&mut mods, &read_mod_list().order);
        let tex_mark = gfx.texture_mark();
        let icons = mods
            .iter()
            .map(|m| {
                let path = m.icon_path.as_ref()?;
                let tex = gfx.load_texture(path).map_err(|e| log::warn!("mod icon: {e}")).ok()?;
                let (w, h) = gfx.texture_size(tex);
                // Psych: a strip of square frames.
                let n = if m.kind == ModKind::Psych && h > 0.0 && w > h && (w % h).abs() < 0.5 { (w / h).round() as u32 } else { 1 };
                let fw = if n > 1 { h } else { w };
                Some(Icon { tex, frame: [0.0, 0.0, fw, h], frames: n })
            })
            .collect();
        gfx.end_scope();
        let sfx = |p: &str| assets.sound(&format!("default:{p}"));
        Self {
            dir,
            mods,
            icons,
            tex_mark,
            selected: 0,
            open: None,
            hover: None,
            scroll: 0.0,
            time: 0.0,
            closing: None,
            scroll_sfx: sfx("scrollMenu"),
            confirm_sfx: sfx("confirmMenu"),
            cancel_sfx: sfx("cancelMenu"),
            disabled: disabled_mods(),
            changed: false,
            loaded: funkin_core::vfs::mods(),
        }
    }

    /// The enabled mods differ from the loaded ones: leaving restarts the game.
    pub fn needs_restart(&self) -> bool {
        self.changed && enabled_mod_dirs() != self.loaded
    }

    /// How many mods can load (V-Slice ones, listed first).
    fn loadable(&self) -> usize {
        self.mods.iter().take_while(|m| m.kind == ModKind::VSlice).count()
    }

    /// Move a mod one place earlier (`-1`) or later (`1`) in the load order.
    fn shift(&mut self, i: usize, by: isize, audio: &Audio) {
        let n = self.loadable();
        let j = i as isize + by;
        if i >= n || j < 0 || j as usize >= n {
            self.play(audio, &self.cancel_sfx.clone(), 0.6);
            return;
        }
        let j = j as usize;
        self.mods.swap(i, j);
        self.icons.swap(i, j);
        save_order(self.mods[..n].iter().map(|m| m.folder.clone()).collect());
        self.changed = true;
        self.selected = j;
        if self.open.is_some() {
            self.open = Some(j);
        }
        self.play(audio, &self.scroll_sfx.clone(), 0.5);
    }

    /// The details panel's "earlier" / "later" buttons.
    fn order_rects(&self) -> [[f32; 4]; 2] {
        let [sx, sy, sw, sh] = self.switch_rect();
        let w = (sw - 10.0) / 2.0;
        [[sx, sy + sh + 8.0, w, 30.0], [sx + w + 10.0, sy + sh + 8.0, w, 30.0]]
    }

    fn enabled(&self, i: usize) -> bool {
        self.mods[i].kind == ModKind::VSlice && !self.disabled.contains(&self.mods[i].folder)
    }

    /// Switch a mod on or off (V-Slice mods only; the others can't load).
    fn toggle(&mut self, i: usize, audio: &Audio) {
        let Some(m) = self.mods.get(i) else { return };
        if m.kind != ModKind::VSlice {
            self.play(audio, &self.cancel_sfx.clone(), 0.6);
            return;
        }
        let f = m.folder.clone();
        if !self.disabled.remove(&f) {
            self.disabled.insert(f);
        }
        save_disabled(&self.disabled);
        self.changed = true;
        self.play(audio, &self.confirm_sfx.clone(), 0.7);
    }

    /// The details panel's on/off switch.
    fn switch_rect(&self) -> [f32; 4] {
        let y = TOP - 10.0;
        [PANEL_X + 26.0, y + 20.0 + 112.0 + 16.0, VIEW_W - PAD - PANEL_X - 46.0, 34.0]
    }

    fn grid_right(&self) -> f32 {
        if self.open.is_some() { PANEL_X - 20.0 } else { VIEW_W - PAD }
    }

    fn columns(&self) -> usize {
        (((self.grid_right() - PAD) / CELL_W).floor() as usize).max(1)
    }

    /// Top-left of a tile, scrolled.
    fn cell(&self, i: usize) -> (f32, f32) {
        let cols = self.columns();
        let used = cols as f32 * CELL_W;
        let x0 = PAD + ((self.grid_right() - PAD) - used) / 2.0;
        (x0 + (i % cols) as f32 * CELL_W, TOP + (i / cols) as f32 * CELL_H - self.scroll)
    }

    fn hit(&self, x: f32, y: f32) -> Option<usize> {
        if !(TOP..VIEW_H - BOTTOM).contains(&y) {
            return None;
        }
        (0..self.mods.len()).find(|&i| {
            let (cx, cy) = self.cell(i);
            x >= cx + (CELL_W - ICON) / 2.0 - 8.0 && x <= cx + (CELL_W + ICON) / 2.0 + 8.0 && y >= cy && y <= cy + CELL_H - 12.0
        })
    }

    fn play(&self, audio: &Audio, s: &Option<std::sync::Arc<Pcm>>, vol: f32) {
        if let Some(s) = s {
            audio.play_sfx(s.clone(), vol);
        }
    }

    fn select(&mut self, i: usize, audio: &Audio) {
        if i != self.selected {
            self.selected = i;
            self.play(audio, &self.scroll_sfx.clone(), 0.4);
        }
        if self.open.is_some() {
            self.open = Some(i);
        }
    }

    fn close(&mut self, audio: &Audio) {
        if self.closing.is_none() {
            self.play(audio, &self.cancel_sfx.clone(), 1.0);
            self.closing = Some(self.time);
        }
    }

    pub fn key(&mut self, code: KeyCode, menu: Option<KeyCode>, audio: &Audio) -> MenuAction {
        if self.closing.is_some() {
            return MenuAction::None;
        }
        if code == KeyCode::Tab {
            self.close(audio);
            return MenuAction::None;
        }
        let n = self.mods.len();
        if code == KeyCode::Space && n > 0 {
            self.toggle(self.selected, audio);
            return MenuAction::None;
        }
        if matches!(code, KeyCode::BracketLeft | KeyCode::PageUp) && n > 0 {
            self.shift(self.selected, -1, audio);
            return MenuAction::None;
        }
        if matches!(code, KeyCode::BracketRight | KeyCode::PageDown) && n > 0 {
            self.shift(self.selected, 1, audio);
            return MenuAction::None;
        }
        let cols = self.columns();
        match menu {
            Some(KeyCode::Escape) => {
                if self.open.take().is_some() {
                    self.play(audio, &self.cancel_sfx.clone(), 1.0);
                } else {
                    self.close(audio);
                }
            }
            _ if n == 0 => {}
            Some(KeyCode::ArrowLeft) => self.select((self.selected + n - 1) % n, audio),
            Some(KeyCode::ArrowRight) => self.select((self.selected + 1) % n, audio),
            Some(KeyCode::ArrowUp) if self.selected >= cols => self.select(self.selected - cols, audio),
            Some(KeyCode::ArrowDown) if self.selected + cols < n => self.select(self.selected + cols, audio),
            Some(KeyCode::ArrowDown) if self.selected / cols < (n - 1) / cols => self.select(n - 1, audio),
            Some(KeyCode::Enter) => {
                self.open = if self.open == Some(self.selected) { None } else { Some(self.selected) };
                self.play(audio, &self.confirm_sfx.clone(), 0.7);
            }
            _ => {}
        }
        MenuAction::None
    }

    pub fn mouse_moved(&mut self, x: f32, y: f32) {
        self.hover = self.hit(x, y);
    }

    pub fn click(&mut self, x: f32, y: f32, audio: &Audio) {
        if self.closing.is_some() {
            return;
        }
        if let Some(i) = self.open {
            let [rx, ry, rw, rh] = self.switch_rect();
            if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                self.toggle(i, audio);
                return;
            }
            if self.mods[i].kind == ModKind::VSlice {
                for (k, [rx, ry, rw, rh]) in self.order_rects().into_iter().enumerate() {
                    if x >= rx && x <= rx + rw && y >= ry && y <= ry + rh {
                        self.shift(i, if k == 0 { -1 } else { 1 }, audio);
                        return;
                    }
                }
            }
        }
        if let Some(i) = self.hit(x, y) {
            self.selected = i;
            self.open = if self.open == Some(i) { None } else { Some(i) };
            self.play(audio, &self.confirm_sfx.clone(), 0.7);
            self.hover = self.hit(x, y);
        }
    }

    pub fn update(&mut self, dt: f32) -> MenuAction {
        self.time += dt;
        if let Some(at) = self.closing {
            if self.time - at >= 0.2 {
                return MenuAction::CloseMods;
            }
        }
        // Keep the selected row in view.
        if !self.mods.is_empty() {
            let row = (self.selected / self.columns()) as f32;
            let (top, view) = (row * CELL_H, VIEW_H - BOTTOM - TOP);
            let target = if top - self.scroll < 0.0 {
                top
            } else if top + CELL_H - self.scroll > view {
                top + CELL_H - view
            } else {
                self.scroll
            };
            self.scroll = lerp(self.scroll, target.max(0.0), 1.0 - (-dt * 14.0).exp());
        }
        MenuAction::None
    }

    /// How opaque the menu is (fades in and out over 0.2 s).
    fn alpha(&self) -> f32 {
        match self.closing {
            Some(at) => (1.0 - (self.time - at) / 0.2).clamp(0.0, 1.0),
            None => (self.time / 0.2).clamp(0.0, 1.0),
        }
    }

    fn draw_icon(&self, gfx: &mut Gfx, font: &TtfFont, i: usize, x: f32, y: f32, size: f32, a: f32) {
        let m = &self.mods[i];
        match &self.icons[i] {
            Some(icon) => {
                let [_, _, fw, fh] = icon.frame;
                let f = if icon.frames > 1 { (self.time * m.icon_fps) as u32 % icon.frames } else { 0 };
                let s = (size / fw).min(size / fh);
                let (w, h) = (fw * s, fh * s);
                gfx.image(icon.tex, [f as f32 * fw, 0.0, fw, fh], x + (size - w) / 2.0, y + (size - h) / 2.0, w, h, [1.0, 1.0, 1.0, a]);
            }
            None => {
                let c = m.color.unwrap_or(ACCENT);
                gfx.rect(x, y, size, size, [c[0] * 0.35, c[1] * 0.35, c[2] * 0.35, a]);
                // No icon: the title's first letter.
                let letter: String = m.title.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_else(|| "?".into());
                let fs = size * 0.55;
                let w = font.width(gfx, &letter, fs);
                font.draw(gfx, &letter, x + (size - w) / 2.0, y + (size - font.line_height(fs)) / 2.0, fs, [1.0, 1.0, 1.0, 0.85 * a]);
            }
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, fonts: &Fonts) {
        let a = self.alpha();
        gfx.rect(0.0, 0.0, VIEW_W, VIEW_H, [0.02, 0.01, 0.05, 0.86 * a]);
        let white = [1.0, 1.0, 1.0, a];
        let dim = [0.72, 0.72, 0.8, a];
        fonts.bold.draw(gfx, "MODS", PAD, 26.0, 0.9, white);
        let Some(font) = fonts.vcr.as_ref() else { return };
        let count = match self.mods.len() {
            1 => "1 mod".to_owned(),
            n => format!("{n} mods"),
        };
        let w = font.width(gfx, &count, 22.0);
        font.draw(gfx, &count, VIEW_W - PAD - w, 40.0, 22.0, dim);
        let dir = self.dir.display().to_string();
        let w = font.width(gfx, &dir, 16.0).min(VIEW_W - 2.0 * PAD);
        font.draw(gfx, &dir, VIEW_W - PAD - w, 70.0, 16.0, [0.5, 0.5, 0.58, a]);

        if self.mods.is_empty() {
            let msg = "No mods found. Put mod folders in:";
            let w = font.width(gfx, msg, 26.0);
            font.draw(gfx, msg, (VIEW_W - w) / 2.0, VIEW_H / 2.0 - 40.0, 26.0, white);
            let w = font.width(gfx, &dir, 20.0);
            font.draw(gfx, &dir, (VIEW_W - w) / 2.0, VIEW_H / 2.0, 20.0, dim);
        }

        // The grid, clipped between the header and the footer.
        let clip = |gfx: &mut Gfx| gfx.rect(0.0, TOP - 10.0, VIEW_W, VIEW_H - BOTTOM - TOP + 10.0, [1.0; 4]);
        gfx.mask(MaskOp::Begin);
        clip(gfx);
        gfx.mask(MaskOp::Content);
        for i in 0..self.mods.len() {
            let (cx, cy) = self.cell(i);
            if cy > VIEW_H || cy + CELL_H < 0.0 {
                continue;
            }
            let m = &self.mods[i];
            let accent = m.color.unwrap_or(ACCENT);
            let ix = cx + (CELL_W - ICON) / 2.0;
            let (sel, hov) = (i == self.selected, self.hover == Some(i));
            if sel || hov || self.open == Some(i) {
                let c = if sel { [accent[0], accent[1], accent[2], a] } else { [1.0, 1.0, 1.0, 0.35 * a] };
                let t = if self.open == Some(i) { 4.0 } else { 2.0 };
                frame(gfx, ix - 6.0, cy - 6.0, ICON + 12.0, ICON + 12.0, t, c);
            }
            gfx.rect(ix, cy, ICON, ICON, [0.0, 0.0, 0.0, 0.35 * a]);
            let on = self.enabled(i);
            self.draw_icon(gfx, font, i, ix, cy, ICON, if on { a } else { 0.35 * a });
            if m.kind == ModKind::VSlice {
                // Its place in the load order.
                let num = format!("#{}", i + 1);
                let nw = font.width(gfx, &num, 14.0) + 12.0;
                gfx.rect(ix + 4.0, cy + 4.0, nw, 20.0, [0.0, 0.0, 0.0, 0.6 * a]);
                font.draw(gfx, &num, ix + 10.0, cy + 6.0, 14.0, white);
                let (label, c) = if on { ("ON", [0.2, 0.75, 0.35, a]) } else { ("OFF", [0.35, 0.35, 0.4, a]) };
                let bw = font.width(gfx, label, 14.0) + 12.0;
                gfx.rect(ix + ICON - bw - 4.0, cy + 4.0, bw, 20.0, c);
                font.draw(gfx, label, ix + ICON - bw + 2.0, cy + 6.0, 14.0, white);
            }
            // Up to two lines of title under the icon.
            let lines = wrap(gfx, font, &m.title, 17.0, CELL_W - 12.0);
            for (k, l) in lines.iter().take(2).enumerate() {
                let l = if k == 1 && lines.len() > 2 { ellipsize(gfx, font, l, 17.0, CELL_W - 12.0) } else { l.clone() };
                let w = font.width(gfx, &l, 17.0);
                font.draw(gfx, &l, cx + (CELL_W - w) / 2.0, cy + ICON + 8.0 + k as f32 * 19.0, 17.0, if sel { white } else { dim });
            }
        }
        gfx.mask(MaskOp::End);
        clip(gfx);
        gfx.mask(MaskOp::Done);

        if let Some(i) = self.open {
            self.draw_details(gfx, font, i, a);
        }

        let hint = if self.open.is_some() { "Arrows / click: pick    Space: on/off    [ ]: load order    Esc: hide details    Tab: back" } else { "Arrows: move    Enter / click: details    Space: on/off    [ ]: load order    Esc / Tab: back" };
        let w = font.width(gfx, hint, 18.0);
        font.draw(gfx, hint, (VIEW_W - w) / 2.0, VIEW_H - 36.0, 18.0, [0.6, 0.6, 0.68, a]);
        if self.needs_restart() {
            let msg = "The game restarts when you leave this menu, to load the new mod set.";
            let w = font.width(gfx, msg, 18.0);
            font.draw(gfx, msg, (VIEW_W - w) / 2.0, 92.0, 18.0, [1.0, 0.82, 0.35, a]);
        }
    }

    fn draw_details(&self, gfx: &mut Gfx, font: &TtfFont, i: usize, a: f32) {
        let m = &self.mods[i];
        let accent = m.color.unwrap_or(ACCENT);
        let (x, y, w, h) = (PANEL_X, TOP - 10.0, VIEW_W - PAD - PANEL_X, VIEW_H - BOTTOM - TOP + 4.0);
        gfx.rect(x, y, w, h, [0.09, 0.08, 0.13, 0.96 * a]);
        gfx.rect(x, y, 6.0, h, [accent[0], accent[1], accent[2], a]);
        let white = [1.0, 1.0, 1.0, a];
        let dim = [0.72, 0.72, 0.8, a];
        let (lx, inner) = (x + 26.0, w - 46.0);

        // Icon and title.
        let icon = 112.0;
        gfx.rect(lx, y + 20.0, icon, icon, [0.0, 0.0, 0.0, 0.4 * a]);
        self.draw_icon(gfx, font, i, lx, y + 20.0, icon, a);
        let tx = lx + icon + 18.0;
        let tw = x + w - 20.0 - tx;
        let mut ty = y + 22.0;
        for l in wrap(gfx, font, &m.title, 28.0, tw).iter().take(2) {
            font.draw(gfx, l, tx, ty, 28.0, white);
            ty += 30.0;
        }
        if let Some(author) = &m.author {
            let l = ellipsize(gfx, font, &format!("by {author}"), 18.0, tw);
            font.draw(gfx, &l, tx, ty + 2.0, 18.0, dim);
            ty += 24.0;
        }
        let tag = m.kind.label();
        let tagw = font.width(gfx, tag, 15.0) + 14.0;
        gfx.rect(tx, ty + 6.0, tagw, 22.0, [accent[0] * 0.5, accent[1] * 0.5, accent[2] * 0.5, a]);
        font.draw(gfx, tag, tx + 7.0, ty + 8.0, 15.0, white);

        // The on/off switch.
        let [sx, sy, sw, sh] = self.switch_rect();
        let (label, c) = match (m.kind, self.enabled(i)) {
            (ModKind::VSlice, true) => ("Enabled  (Space to switch off)", [0.16, 0.6, 0.3, a]),
            (ModKind::VSlice, false) => ("Disabled  (Space to switch on)", [0.3, 0.3, 0.36, a]),
            _ => ("Can't be loaded (not a V-Slice mod)", [0.4, 0.18, 0.2, a]),
        };
        gfx.rect(sx, sy, sw, sh, c);
        font.draw(gfx, label, sx + 12.0, sy + (sh - font.line_height(18.0)) / 2.0, 18.0, white);

        // Load order buttons.
        let mut cy = sy + sh + 14.0;
        let mut facts: Vec<(&str, String)> = Vec::new();
        if m.kind == ModKind::VSlice {
            let n = self.loadable();
            for (k, [bx, by, bw, bh]) in self.order_rects().into_iter().enumerate() {
                let (label, live) = if k == 0 { ("< Load earlier  [", i > 0) } else { ("Load later  ] >", i + 1 < n) };
                gfx.rect(bx, by, bw, bh, if live { [0.22, 0.2, 0.3, a] } else { [0.14, 0.13, 0.18, a] });
                let lw = font.width(gfx, label, 16.0);
                font.draw(gfx, label, bx + (bw - lw) / 2.0, by + (bh - font.line_height(16.0)) / 2.0, 16.0, if live { white } else { [0.45, 0.45, 0.5, a] });
            }
            cy += 38.0;
            facts.push(("Load order", format!("{} of {n} (later mods win)", i + 1)));
        }
        if let Some(v) = &m.version {
            facts.push(("Version", v.clone()));
        }
        if let Some(v) = &m.api_version {
            facts.push(("API", v.clone()));
        }
        if let Some(v) = &m.license {
            facts.push(("License", v.clone()));
        }
        facts.push(("Folder", m.folder.clone()));
        if let Some(id) = m.id.as_ref().filter(|id| **id != m.folder) {
            facts.push(("ID", id.clone()));
        }
        for (k, v) in &facts {
            font.draw(gfx, k, lx, cy, 17.0, [accent[0].max(0.5), accent[1].max(0.5), accent[2].max(0.5), a]);
            let v = ellipsize(gfx, font, v, 17.0, inner - 110.0);
            font.draw(gfx, &v, lx + 110.0, cy, 17.0, white);
            cy += 22.0;
        }
        cy += 10.0;

        // Description, then contributors in what room is left.
        let bottom = y + h - 16.0;
        let desc = if m.description.trim().is_empty() { "No description." } else { m.description.trim() };
        let lines = wrap(gfx, font, desc, 18.0, inner);
        let room_for_desc = if m.contributors.is_empty() { usize::MAX } else { 7 };
        let shown = lines.len().min(room_for_desc).min(((bottom - cy) / 21.0).max(0.0) as usize);
        for (k, l) in lines.iter().take(shown).enumerate() {
            let l = if k + 1 == shown && shown < lines.len() { ellipsize(gfx, font, &format!("{l}…"), 18.0, inner) } else { l.clone() };
            font.draw(gfx, &l, lx, cy, 18.0, dim);
            cy += 21.0;
        }
        if !m.contributors.is_empty() && cy + 50.0 < bottom {
            cy += 12.0;
            font.draw(gfx, &format!("Contributors ({})", m.contributors.len()), lx, cy, 20.0, white);
            cy += 28.0;
            // Two columns when there are many.
            let two = m.contributors.len() > ((bottom - cy) / 20.0) as usize;
            let colw = if two { inner / 2.0 } else { inner };
            let rows = (((bottom - cy) / 20.0).floor() as usize).max(1);
            let cap = if two { rows * 2 } else { rows };
            let more = m.contributors.len().saturating_sub(cap);
            let n = if more > 0 { cap - 1 } else { m.contributors.len() };
            for (k, (name, role)) in m.contributors.iter().take(n).enumerate() {
                let (col, row) = (k / rows, k % rows);
                let text = match role {
                    Some(r) => format!("{name} - {r}"),
                    None => name.clone(),
                };
                let t = ellipsize(gfx, font, &text, 15.0, colw - 12.0);
                font.draw(gfx, &t, lx + col as f32 * colw, cy + row as f32 * 20.0, 15.0, dim);
            }
            if more > 0 {
                let k = n;
                let (col, row) = (k / rows, k % rows);
                font.draw(gfx, &format!("…and {} more", more + 1), lx + col as f32 * colw, cy + row as f32 * 20.0, 15.0, [0.55, 0.55, 0.62, a]);
            }
        }
    }
}

/// A rectangle outline `t` px thick.
fn frame(gfx: &mut Gfx, x: f32, y: f32, w: f32, h: f32, t: f32, c: Color) {
    gfx.rect(x, y, w, t, c);
    gfx.rect(x, y + h - t, w, t, c);
    gfx.rect(x, y + t, t, h - 2.0 * t, c);
    gfx.rect(x + w - t, y + t, t, h - 2.0 * t, c);
}

/// Greedy word wrap to `width` px (long words are cut).
fn wrap(gfx: &mut Gfx, font: &TtfFont, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let trial = if cur.is_empty() { word.to_owned() } else { format!("{cur} {word}") };
            if font.width(gfx, &trial, size) <= width {
                cur = trial;
                continue;
            }
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            // A word wider than the line: split it by characters.
            let mut piece = String::new();
            for ch in word.chars() {
                piece.push(ch);
                if font.width(gfx, &piece, size) > width && piece.chars().count() > 1 {
                    piece.pop();
                    out.push(std::mem::take(&mut piece));
                    piece.push(ch);
                }
            }
            cur = piece;
        }
        out.push(cur);
    }
    while out.last().is_some_and(|l| l.is_empty()) && out.len() > 1 {
        out.pop();
    }
    out
}

/// Cut `text` with "…" to fit `width` px.
fn ellipsize(gfx: &mut Gfx, font: &TtfFont, text: &str, size: f32, width: f32) -> String {
    if font.width(gfx, text, size) <= width {
        return text.to_owned();
    }
    let mut s: String = text.trim_end_matches('…').to_owned();
    while !s.is_empty() && font.width(gfx, &format!("{s}…"), size) > width {
        s.pop();
    }
    format!("{}…", s.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rusted-mic-mods-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn reads_polymod_and_psych() {
        let root = temp("meta");
        let v = root.join("vslice");
        std::fs::create_dir_all(&v).unwrap();
        std::fs::write(v.join("_polymod_meta.json"), r#"{"title":"Test Mod","description":"Hi","author":"Me","mod_version":"1.2.0","api_version":"0.8.6","license":"MIT","contributors":[{"name":"A","role":"Art"},{"name":"B"}]}"#).unwrap();
        let p = root.join("psych");
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("pack.json"), r#"{"name":"Psych One","description":"Yo","color":[255,0,0],"iconFramerate":5}"#).unwrap();
        std::fs::create_dir_all(root.join("plain")).unwrap();
        let mods = scan(&root);
        assert_eq!(mods.iter().map(|m| m.title.as_str()).collect::<Vec<_>>(), ["plain", "Psych One", "Test Mod"]);
        let t = &mods[2];
        assert_eq!(t.kind, ModKind::VSlice);
        assert_eq!(t.version.as_deref(), Some("1.2.0"));
        assert_eq!(t.contributors, vec![("A".into(), Some("Art".into())), ("B".into(), None)]);
        let ps = &mods[1];
        assert_eq!(ps.kind, ModKind::Psych);
        assert_eq!(ps.color, Some([1.0, 0.0, 0.0, 1.0]));
        assert_eq!(ps.icon_fps, 5.0);
        assert_eq!(mods[0].kind, ModKind::Unknown);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn menu_follows_load_order() {
        let m = |folder: &str, kind| ModInfo {
            folder: folder.into(),
            kind,
            title: folder.into(),
            id: None,
            description: String::new(),
            author: None,
            version: None,
            api_version: None,
            license: None,
            contributors: vec![],
            color: None,
            icon_path: None,
            icon_fps: 0.0,
        };
        let mut mods = vec![m("b", ModKind::VSlice), m("psych", ModKind::Psych), m("c", ModKind::VSlice), m("a", ModKind::VSlice)];
        // "c" first, then the unlisted V-Slice mods by folder, then those that can't load.
        sort_for_menu(&mut mods, &["C".into()]);
        assert_eq!(mods.iter().map(|m| m.folder.as_str()).collect::<Vec<_>>(), ["c", "a", "b", "psych"]);
    }
}
