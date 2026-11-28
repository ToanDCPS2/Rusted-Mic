//! Game progress that is not a setting (`Save.hx`): which characters the
//! player has seen in character select, and whether its intro already played.
//! Kept in `save.json` next to `settings.json`.

use serde::{Deserialize, Serialize};

use crate::settings::data_dir;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Save {
    /// `Save.oldChar`: the character select lights video has played.
    pub old_char: bool,
    /// `Save.charactersSeen`: characters whose unlock animation has played.
    pub characters_seen: Vec<String>,
    /// `Save.getLevelScore`: best story mode score, keyed `level:difficulty`.
    pub level_scores: std::collections::HashMap<String, i64>,
    /// `Save.getSongScore`: best freeplay result, keyed `song:difficulty:variation`.
    pub song_scores: std::collections::HashMap<String, SongScore>,
}

/// A song's saved best (`SaveScoreData`, the parts used here).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SongScore {
    pub score: i64,
    pub rank: Option<funkin_core::scoring::ScoringRank>,
    /// `Scoring.tallyCompletion` of the tallies saved with the best score.
    pub completion: f64,
}

impl Default for Save {
    fn default() -> Self {
        Self { old_char: false, characters_seen: vec!["bf".into()], level_scores: Default::default(), song_scores: Default::default() }
    }
}

impl Save {
    pub fn load() -> Self {
        let p = data_dir().join("save.json");
        match std::fs::read_to_string(&p) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                log::warn!("{}: {e}", p.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let dir = data_dir();
        let p = dir.join("save.json");
        let r = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&p, serde_json::to_string_pretty(self).unwrap_or_default()));
        if let Err(e) = r {
            log::warn!("{}: {e}", p.display());
        }
    }

    pub fn is_seen(&self, id: &str) -> bool {
        self.characters_seen.iter().any(|c| c == id)
    }

    pub fn add_seen(&mut self, id: &str) {
        if !self.is_seen(id) {
            self.characters_seen.push(id.to_owned());
        }
    }
}
