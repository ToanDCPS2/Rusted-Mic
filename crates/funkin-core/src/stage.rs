//! Stage and character data (`data/stages/*.json`, `data/characters/*.json`).
//! Ports of `StageData.hx` and `CharacterData.hx`.

use serde::{Deserialize, Deserializer, Serialize};
use std::path::Path;

use crate::song::{read_json, SongError};

fn one() -> f64 {
    1.0
}
fn ones() -> [f64; 2] {
    [1.0, 1.0]
}
fn fps24() -> f64 {
    24.0
}
fn sing_time() -> f64 {
    8.0
}
fn sparrow() -> String {
    "sparrow".to_owned()
}

/// Accepts either `1.5` or `[1.5, 1.5]`.
fn scale_pair<'de, D: Deserializer<'de>>(d: D) -> Result<[f64; 2], D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum S {
        One(f64),
        Two([f64; 2]),
    }
    Ok(match S::deserialize(d)? {
        S::One(v) => [v, v],
        S::Two(v) => v,
    })
}

/// One animation of a character or prop (`AnimationData`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationData {
    pub name: String,
    #[serde(default)]
    pub prefix: String,
    /// Per-animation atlas (multi-atlas characters).
    #[serde(default)]
    pub asset_path: Option<String>,
    #[serde(default)]
    pub offsets: [f64; 2],
    #[serde(default)]
    pub looped: bool,
    #[serde(default = "fps24")]
    pub frame_rate: f64,
    #[serde(default)]
    pub frame_indices: Vec<i32>,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub flip_y: bool,
    /// Texture atlases: `framelabel` (default) or `symbol`.
    #[serde(default)]
    pub anim_type: Option<String>,
}

/// Port of `CharacterData`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CharacterData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    /// `sparrow`, `multisparrow`, `animateatlas`, `multianimateatlas`, `packer`.
    #[serde(default = "sparrow")]
    pub render_type: String,
    pub asset_path: String,
    #[serde(default = "one")]
    pub scale: f64,
    /// Global offsets.
    #[serde(default)]
    pub offsets: [f64; 2],
    #[serde(default)]
    pub camera_offsets: [f64; 2],
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "one")]
    pub dance_every: f64,
    /// Steps a sing animation is held before returning to idle.
    #[serde(default = "sing_time")]
    pub sing_time: f64,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub starting_animation: Option<String>,
    #[serde(default)]
    pub animations: Vec<AnimationData>,
    #[serde(skip)]
    pub id: String,
}

/// A character slot on a stage.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageCharacter {
    #[serde(default)]
    pub z_index: i32,
    /// Position of the character's feet.
    #[serde(default)]
    pub position: [f64; 2],
    #[serde(default)]
    pub camera_offsets: [f64; 2],
    #[serde(default = "one")]
    pub scale: f64,
    #[serde(default = "ones")]
    pub scroll: [f64; 2],
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StageCharacters {
    #[serde(default)]
    pub bf: Option<StageCharacter>,
    #[serde(default)]
    pub dad: Option<StageCharacter>,
    #[serde(default)]
    pub gf: Option<StageCharacter>,
}

/// A stage prop (`StageDataProp`).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageProp {
    #[serde(default)]
    pub name: String,
    /// Image key, or `#RRGGBB` for a solid colour block.
    pub asset_path: String,
    #[serde(default)]
    pub position: [f64; 2],
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub is_pixel: bool,
    #[serde(default = "ones", deserialize_with = "scale_pair")]
    pub scale: [f64; 2],
    #[serde(default = "one")]
    pub alpha: f64,
    #[serde(default)]
    pub dance_every: f64,
    #[serde(default = "ones")]
    pub scroll: [f64; 2],
    #[serde(default = "sparrow")]
    pub anim_type: String,
    #[serde(default)]
    pub starting_animation: Option<String>,
    #[serde(default)]
    pub animations: Vec<AnimationData>,
    #[serde(default)]
    pub flip_x: bool,
    #[serde(default)]
    pub angle: f64,
    /// Tint, e.g. `#FFFFFF`.
    #[serde(default)]
    pub color: Option<String>,
}

/// Port of `StageData`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default = "one")]
    pub camera_zoom: f64,
    #[serde(default)]
    pub props: Vec<StageProp>,
    #[serde(default)]
    pub characters: StageCharacters,
    /// Asset library for the stage's images (e.g. `week1`).
    #[serde(default)]
    pub directory: Option<String>,
    #[serde(skip)]
    pub id: String,
}

/// Load `data/stages/<id>.json`.
pub fn load_stage(asset_root: &Path, id: &str) -> Result<StageData, SongError> {
    let path = asset_root.join("data").join("stages").join(format!("{id}.json"));
    let mut s: StageData = read_json(&path)?;
    s.id = id.to_owned();
    Ok(s)
}

/// Load `data/characters/<id>.json`.
pub fn load_character(asset_root: &Path, id: &str) -> Result<CharacterData, SongError> {
    let path = asset_root.join("data").join("characters").join(format!("{id}.json"));
    let mut c: CharacterData = read_json(&path)?;
    c.id = id.to_owned();
    Ok(c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_prop_scale_forms() {
        let p: StageProp = serde_json::from_str(r##"{"assetPath":"x","scale":1.5}"##).unwrap();
        assert_eq!(p.scale, [1.5, 1.5]);
        let p: StageProp = serde_json::from_str(r##"{"assetPath":"#000000","scale":[2,3]}"##).unwrap();
        assert_eq!(p.scale, [2.0, 3.0]);
        assert_eq!(p.scroll, [1.0, 1.0]);
    }
}

#[cfg(test)]
mod asset_tests {
    use super::*;

    /// Parses every stage and character in a real install (set FUNKIN_ASSETS).
    #[test]
    #[ignore]
    fn parse_all_installed() {
        let root = std::path::PathBuf::from(std::env::var("FUNKIN_ASSETS").expect("FUNKIN_ASSETS"));
        let mut bad = Vec::new();
        for (dir, is_stage) in [("stages", true), ("characters", false)] {
            for e in std::fs::read_dir(root.join("data").join(dir)).unwrap() {
                let p = e.unwrap().path();
                if p.extension().and_then(|x| x.to_str()) != Some("json") {
                    continue;
                }
                let id = p.file_stem().unwrap().to_string_lossy().to_string();
                let r = if is_stage { load_stage(&root, &id).map(|_| ()) } else { load_character(&root, &id).map(|_| ()) };
                if let Err(e) = r {
                    bad.push(format!("{dir}/{id}: {e}"));
                }
            }
        }
        assert!(bad.is_empty(), "{bad:#?}");
    }
}
