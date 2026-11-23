//! `SongChartData` (`<id>-chart[-<variation>].json`).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::constants::KEY_COUNT;

/// Accepted chart versions (`SongRegistry.SONG_CHART_DATA_VERSION_RULE`).
pub const SONG_CHART_DATA_VERSION_RULE: &str = "2.0.x";

/// Port of `SongChartData`. Holds every difficulty of one variation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongChartData {
    pub version: String,
    #[serde(default)]
    pub scroll_speed: HashMap<String, f64>,
    #[serde(default)]
    pub events: Vec<SongEventData>,
    #[serde(default)]
    pub notes: HashMap<String, Vec<SongNoteData>>,
    #[serde(default)]
    pub generated_by: String,
    #[serde(skip)]
    pub variation: String,
}

impl SongChartData {
    /// Port of `getScrollSpeed`: falls back to `default`, then to `1.0`.
    pub fn scroll_speed(&self, diff: &str) -> f64 {
        let result = self.scroll_speed.get(diff).copied().unwrap_or(0.0);
        if result == 0.0 && diff != "default" {
            return self.scroll_speed("default");
        }
        if result == 0.0 {
            1.0
        } else {
            result
        }
    }

    /// Notes for a difficulty, if charted.
    pub fn notes(&self, diff: &str) -> Option<&[SongNoteData]> {
        self.notes.get(diff).map(Vec::as_slice)
    }
}

/// Port of `SongNoteData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongNoteData {
    /// Strum time in milliseconds.
    #[serde(rename = "t")]
    pub time: f64,
    /// Lane + strumline index: `0..4` player, `4..8` opponent.
    #[serde(rename = "d")]
    pub data: i32,
    /// Hold length in milliseconds (0 for taps).
    #[serde(rename = "l", default)]
    pub length: f64,
    /// Note kind (e.g. `"alt"`, `"hurt"`); `None`/empty means a normal note.
    #[serde(rename = "k", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(rename = "p", default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<NoteParamData>,
}

impl SongNoteData {
    /// Lane (0 = left, 1 = down, 2 = up, 3 = right).
    #[inline]
    pub fn direction(&self) -> usize {
        self.data.rem_euclid(KEY_COUNT as i32) as usize
    }
    /// 0 = player, 1 = opponent.
    #[inline]
    pub fn strumline_index(&self) -> i32 {
        self.data.div_euclid(KEY_COUNT as i32)
    }
    /// True if this note belongs to the player's strumline.
    #[inline]
    pub fn must_hit(&self) -> bool {
        self.strumline_index() == 0
    }
    #[inline]
    pub fn is_hold(&self) -> bool {
        self.length > 0.0
    }
    /// Normalized kind: empty strings are treated as no kind (`get_kind` in Haxe).
    pub fn kind(&self) -> Option<&str> {
        self.kind.as_deref().filter(|k| !k.is_empty())
    }
}

/// Port of `NoteParamData`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NoteParamData {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "v")]
    pub value: serde_json::Value,
}

/// Port of `SongEventData`. The value is kept as raw JSON; event handlers
/// (camera focus, play animation, ...) interpret it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SongEventData {
    #[serde(rename = "t")]
    pub time: f64,
    #[serde(rename = "e")]
    pub event_kind: String,
    #[serde(rename = "v", default)]
    pub value: serde_json::Value,
}

impl SongEventData {
    /// Port of `valueAsStruct`: non-object values are wrapped as `{default_key: value}`.
    pub fn value_as_struct(&self, default_key: &str) -> serde_json::Map<String, serde_json::Value> {
        match &self.value {
            serde_json::Value::Object(m) => m.clone(),
            serde_json::Value::Null => serde_json::Map::new(),
            other => {
                let mut m = serde_json::Map::new();
                m.insert(default_key.to_owned(), other.clone());
                m
            }
        }
    }
}
