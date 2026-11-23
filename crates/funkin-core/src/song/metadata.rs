//! `SongMetadata` and friends (`<id>-metadata[-<variation>].json`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::constants;

fn unknown() -> String {
    constants::DEFAULT_SONGNAME.to_owned()
}
fn four() -> u32 {
    4
}
fn preview_end() -> f64 {
    constants::DEFAULT_PREVIEW_END_TIME
}
fn default_ratings() -> HashMap<String, i32> {
    HashMap::from([("normal".to_owned(), 0)])
}

/// Accepted metadata versions (`SongRegistry.SONG_METADATA_VERSION_RULE`).
pub const SONG_METADATA_VERSION_RULE: &str = "2.2.x";

/// Port of `SongMetadata`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongMetadata {
    pub version: String,
    #[serde(default = "unknown")]
    pub song_name: String,
    #[serde(default = "unknown")]
    pub artist: String,
    #[serde(default)]
    pub charter: Option<String>,
    #[serde(default)]
    pub divisions: Option<i32>,
    #[serde(default)]
    pub looped: bool,
    #[serde(default)]
    pub offsets: Option<SongOffsets>,
    pub play_data: SongPlayData,
    #[serde(default)]
    pub generated_by: String,
    #[serde(default)]
    pub time_format: SongTimeFormat,
    #[serde(default)]
    pub time_changes: Vec<SongTimeChange>,
    /// Not serialized; filled in by the registry from the file name.
    #[serde(skip)]
    pub variation: String,
}

impl SongMetadata {
    /// Instrumental offset for the given alt instrumental (or the default).
    pub fn instrumental_offset(&self, instrumental: Option<&str>) -> f64 {
        self.offsets
            .as_ref()
            .map(|o| o.instrumental_offset(instrumental))
            .unwrap_or(0.0)
    }
}

/// Port of `SongTimeFormat`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SongTimeFormat {
    #[serde(rename = "ticks")]
    Ticks,
    #[serde(rename = "float")]
    Float,
    #[default]
    #[serde(rename = "ms")]
    Milliseconds,
}

/// Port of `SongTimeChange` (a BPM / time signature change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongTimeChange {
    /// Timestamp in milliseconds.
    #[serde(rename = "t")]
    pub time_stamp: f64,
    /// Time in beats. Recomputed by the conductor when mapped.
    #[serde(rename = "b", default)]
    pub beat_time: Option<f64>,
    #[serde(rename = "bpm")]
    pub bpm: f64,
    #[serde(rename = "n", default = "four")]
    pub time_signature_num: u32,
    #[serde(rename = "d", default = "four")]
    pub time_signature_den: u32,
    #[serde(rename = "bt", default)]
    pub beat_tuplets: Vec<i32>,
}

impl SongTimeChange {
    pub fn new(time_stamp: f64, bpm: f64) -> Self {
        Self {
            time_stamp,
            beat_time: None,
            bpm,
            time_signature_num: 4,
            time_signature_den: 4,
            beat_tuplets: vec![4, 4, 4, 4],
        }
    }
}

/// Port of `SongOffsets`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongOffsets {
    #[serde(default)]
    pub instrumental: f64,
    #[serde(default)]
    pub alt_instrumentals: HashMap<String, f64>,
    #[serde(default)]
    pub vocals: HashMap<String, f64>,
    #[serde(default)]
    pub alt_vocals: HashMap<String, HashMap<String, f64>>,
}

impl SongOffsets {
    pub fn instrumental_offset(&self, instrumental: Option<&str>) -> f64 {
        match instrumental {
            None | Some("") => self.instrumental,
            Some(id) => *self.alt_instrumentals.get(id).unwrap_or(&self.instrumental),
        }
    }

    pub fn vocal_offset(&self, char_id: &str, instrumental: Option<&str>) -> f64 {
        match instrumental {
            None => *self.vocals.get(char_id).unwrap_or(&0.0),
            Some(inst) => self
                .alt_vocals
                .get(inst)
                .and_then(|m| m.get(char_id))
                .copied()
                .unwrap_or(0.0),
        }
    }
}

/// Port of `SongPlayData`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongPlayData {
    #[serde(default)]
    pub song_variations: Vec<String>,
    #[serde(default)]
    pub difficulties: Vec<String>,
    pub characters: SongCharacterData,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub note_style: String,
    #[serde(default = "default_ratings")]
    pub ratings: HashMap<String, i32>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default, rename = "discordRPCImage")]
    pub discord_rpc_image: Option<String>,
    #[serde(default)]
    pub sticker_pack: Option<String>,
    #[serde(default)]
    pub preview_start: f64,
    #[serde(default = "preview_end")]
    pub preview_end: f64,
}

/// Port of `SongCharacterData`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongCharacterData {
    #[serde(default)]
    pub player: String,
    #[serde(default)]
    pub girlfriend: String,
    #[serde(default)]
    pub opponent: String,
    /// Instrumental suffix: `""` → `Inst.ogg`, `"erect"` → `Inst-erect.ogg`.
    #[serde(default)]
    pub instrumental: String,
    #[serde(default)]
    pub alt_instrumentals: Vec<String>,
    #[serde(default)]
    pub opponent_vocals: Option<Vec<String>>,
    #[serde(default)]
    pub player_vocals: Option<Vec<String>>,
}
