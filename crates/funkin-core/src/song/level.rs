//! Story mode levels (`data/levels/<id>.json`), port of `LevelData`.
//! Only the fields needed for progression are typed; visual props are kept raw.

use serde::{Deserialize, Serialize};

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelData {
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub title_asset: String,
    #[serde(default)]
    pub songs: Vec<String>,
    #[serde(default = "yes")]
    pub visible: bool,
    #[serde(default)]
    pub background: String,
    /// Visual props (characters on the story menu); interpreted by the frontend.
    #[serde(default)]
    pub props: serde_json::Value,
    #[serde(skip)]
    pub id: String,
}
