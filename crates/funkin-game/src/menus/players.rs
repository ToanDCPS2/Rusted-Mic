//! `PlayerRegistry` / `PlayableCharacter`: the characters you can play as in
//! freeplay (`data/players/*.json`).

use std::collections::HashMap;

use serde::Deserialize;

use crate::assets::Assets;

pub const DEFAULT_CHARACTER: &str = "bf";

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PlayerData {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub owned_chars: Vec<String>,
    pub show_unowned_chars: bool,
    pub unlocked: bool,
    pub freeplay_style: Option<String>,
    pub sticker_pack: Option<String>,
    #[serde(rename = "freeplayDJ")]
    pub freeplay_dj: Option<DjData>,
    pub char_select: Option<CharSelectData>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DjData {
    pub asset_path: String,
    pub text1: Option<String>,
    pub text2: Option<String>,
    pub text3: Option<String>,
    pub char_select: Option<DjCharSelect>,
    pub animations: Vec<DjAnim>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct DjCharSelect {
    pub transition_delay: Option<f32>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct DjAnim {
    pub name: String,
    pub prefix: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CharSelectData {
    pub position: Option<usize>,
    pub gf: Option<CharSelectGf>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CharSelectGf {
    pub asset_path: String,
    pub anim_info_path: Option<String>,
    pub visualizer: bool,
}

/// `FreeplayStyle` (`data/ui/freeplay/styles/*.json`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct FreeplayStyle {
    pub bg_asset: String,
    pub selector_asset: String,
    pub numbers_asset: String,
    pub capsule_asset: String,
    pub capsule_text_colors: Vec<String>,
    pub start_delay: f32,
}

impl Default for FreeplayStyle {
    fn default() -> Self {
        Self {
            bg_asset: "freeplay/freeplayBGweek1-bf".into(),
            selector_asset: "freeplay/freeplaySelector/freeplaySelector".into(),
            numbers_asset: "digital_numbers".into(),
            capsule_asset: "freeplay/freeplayCapsule/capsule/freeplayCapsule".into(),
            capsule_text_colors: vec!["#00ccff".into(), "#00ccff".into()],
            start_delay: 1.0,
        }
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &std::path::Path) -> Option<T> {
    let text = funkin_core::vfs::read_to_string(path).ok()?;
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| log::warn!("{}: {e}", path.display())).ok()
}

/// Every playable character, sorted by id.
pub fn list_players(assets: &Assets) -> Vec<PlayerData> {
    let dir = assets.root().join("data").join("players");
    let mut out: Vec<PlayerData> = funkin_core::vfs::read_dir(&dir)
        .map(|entries| {
            entries
                .into_iter()
                .filter(|e| e.name.ends_with(".json"))
                .filter_map(|e| {
                    let mut p: PlayerData = read_json(&dir.join(&e.name))?;
                    p.id = e.name.trim_end_matches(".json").to_owned();
                    Some(p)
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

pub fn load_style(assets: &Assets, id: &str) -> FreeplayStyle {
    let path = assets.root().join("data").join("ui").join("freeplay").join("styles").join(format!("{id}.json"));
    read_json(&path).unwrap_or_default()
}

/// The registry: who owns which stage characters.
pub struct Players {
    pub list: Vec<PlayerData>,
    owner: HashMap<String, String>,
}

impl Players {
    pub fn load(assets: &Assets) -> Self {
        let list = list_players(assets);
        let mut owner = HashMap::new();
        for p in &list {
            for c in &p.owned_chars {
                owner.insert(c.clone(), p.id.clone());
            }
        }
        Self { list, owner }
    }

    pub fn get(&self, id: &str) -> Option<&PlayerData> {
        self.list.iter().find(|p| p.id == id)
    }

    /// `PlayableCharacter.shouldShowCharacter`: songs whose player is `stage_char`
    /// show up in this character's freeplay.
    pub fn shows(&self, player: &str, stage_char: &str) -> bool {
        let Some(p) = self.get(player) else { return player == DEFAULT_CHARACTER };
        p.owned_chars.iter().any(|c| c == stage_char) || (p.show_unowned_chars && !self.owner.contains_key(stage_char))
    }

    /// `countUnlockedCharacters`. There is no save data yet, so every
    /// character marked `unlocked` counts (Pico's script would also want
    /// Weekend 1 beaten).
    pub fn unlocked_count(&self) -> usize {
        self.list.iter().filter(|p| p.unlocked).count()
    }
}

/// Parse `#rrggbb`.
pub fn parse_hex(s: &str) -> Option<[f32; 4]> {
    let h = s.trim_start_matches('#');
    let v = u32::from_str_radix(h.get(h.len().saturating_sub(6)..)?, 16).ok()?;
    Some([((v >> 16) & 255) as f32 / 255.0, ((v >> 8) & 255) as f32 / 255.0, (v & 255) as f32 / 255.0, 1.0])
}
