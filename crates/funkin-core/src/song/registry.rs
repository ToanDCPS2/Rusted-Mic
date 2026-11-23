//! Discovers and loads songs and levels from an asset root.
//! Port of `SongRegistry` / `LevelRegistry` plus the audio path logic in `Song.hx`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;
use crate::constants::DEFAULT_VARIATION;

/// One playable variation of a song (e.g. `default`, `erect`, `pico`).
#[derive(Debug, Clone)]
pub struct SongVariation {
    pub metadata: SongMetadata,
    pub chart: SongChartData,
}

/// A fully loaded song with all of its variations.
#[derive(Debug, Clone)]
pub struct Song {
    pub id: String,
    /// Keyed by variation id. Always contains `default` when loaded successfully.
    pub variations: BTreeMap<String, SongVariation>,
}

impl Song {
    pub fn variation(&self, id: &str) -> Option<&SongVariation> {
        self.variations.get(id)
    }

    /// Find the variation that provides `difficulty`, preferring `preferred`.
    /// Mirrors how Freeplay/Story pick the variation for a difficulty.
    pub fn variation_for_difficulty(
        &self,
        difficulty: &str,
        preferred: Option<&str>,
    ) -> Option<&SongVariation> {
        if let Some(v) = preferred.and_then(|p| self.variations.get(p)) {
            if v.metadata
                .play_data
                .difficulties
                .iter()
                .any(|d| d == difficulty)
            {
                return Some(v);
            }
        }
        self.variations.values().find(|v| {
            v.metadata
                .play_data
                .difficulties
                .iter()
                .any(|d| d == difficulty)
        })
    }

    /// Notes for (variation, difficulty).
    pub fn notes(&self, variation: &str, difficulty: &str) -> Result<&[SongNoteData], SongError> {
        self.variations
            .get(variation)
            .and_then(|v| v.chart.notes(difficulty))
            .ok_or_else(|| SongError::MissingDifficulty {
                song: self.id.clone(),
                variation: variation.to_owned(),
                difficulty: difficulty.to_owned(),
            })
    }
}

/// Indexes songs and levels in an asset root (the game's `assets/` directory).
#[derive(Debug, Clone)]
pub struct SongRegistry {
    root: PathBuf,
}

impl SongRegistry {
    pub fn new(asset_root: impl Into<PathBuf>) -> Self {
        Self {
            root: asset_root.into(),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn songs_data_dir(&self) -> PathBuf {
        self.root.join("data").join("songs")
    }

    /// Path of `<id>-metadata[-<variation>].json`.
    pub fn metadata_path(&self, id: &str, variation: &str) -> PathBuf {
        self.songs_data_dir()
            .join(id)
            .join(format!("{id}-metadata{}.json", variation_suffix(variation)))
    }

    /// Path of `<id>-chart[-<variation>].json`.
    pub fn chart_path(&self, id: &str, variation: &str) -> PathBuf {
        self.songs_data_dir()
            .join(id)
            .join(format!("{id}-chart{}.json", variation_suffix(variation)))
    }

    /// List song ids: every `data/songs/<id>/` that contains `<id>-metadata.json`.
    pub fn list_song_ids(&self) -> Vec<String> {
        let Ok(dir) = crate::vfs::read_dir(&self.songs_data_dir()) else {
            return Vec::new();
        };
        let mut ids: Vec<String> = dir
            .into_iter()
            .filter(|e| e.is_dir)
            .map(|e| e.name)
            .filter(|id| crate::vfs::is_file(&self.metadata_path(id, DEFAULT_VARIATION)))
            .collect();
        ids.sort();
        ids
    }

    /// Load just the metadata for a variation.
    pub fn load_metadata(&self, id: &str, variation: &str) -> Result<SongMetadata, SongError> {
        let path = self.metadata_path(id, variation);
        if !crate::vfs::is_file(&path) {
            return Err(SongError::NotFound(format!("{id}:{variation}")));
        }
        let mut meta: SongMetadata = read_json(&path)?;
        check_version(&path, &meta.version, SONG_METADATA_VERSION_RULE)?;
        meta.variation = variation.to_owned();
        Ok(meta)
    }

    /// Load the chart for a variation.
    pub fn load_chart(&self, id: &str, variation: &str) -> Result<SongChartData, SongError> {
        let path = self.chart_path(id, variation);
        let mut chart: SongChartData = read_json(&path)?;
        check_version(&path, &chart.version, SONG_CHART_DATA_VERSION_RULE)?;
        chart.variation = variation.to_owned();
        // Charts are not guaranteed to be sorted on disk; gameplay relies on it.
        for notes in chart.notes.values_mut() {
            notes.sort_by(|a, b| a.time.total_cmp(&b.time));
        }
        chart.events.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(chart)
    }

    /// Load a song and every variation listed in its default metadata.
    /// Variations that fail to load are skipped with a warning (as in Funkin').
    pub fn load_song(&self, id: &str) -> Result<Song, SongError> {
        let default_meta = self.load_metadata(id, DEFAULT_VARIATION)?;
        let default_chart = self.load_chart(id, DEFAULT_VARIATION)?;
        let mut variations = BTreeMap::new();
        let extra: Vec<String> = default_meta.play_data.song_variations.clone();
        variations.insert(
            DEFAULT_VARIATION.to_owned(),
            SongVariation {
                metadata: default_meta,
                chart: default_chart,
            },
        );
        for var in extra {
            match self
                .load_metadata(id, &var)
                .and_then(|m| Ok((m, self.load_chart(id, &var)?)))
            {
                Ok((metadata, chart)) => {
                    variations.insert(var, SongVariation { metadata, chart });
                }
                Err(e) => log::warn!("song '{id}': skipping variation '{var}': {e}"),
            }
        }
        Ok(Song {
            id: id.to_owned(),
            variations,
        })
    }

    /// Load every level in `data/levels/`, sorted by id.
    pub fn load_levels(&self) -> Vec<LevelData> {
        let dir = self.root.join("data").join("levels");
        let Ok(entries) = crate::vfs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut levels: Vec<LevelData> = entries
            .into_iter()
            .map(|e| e.path)
            .filter(|p| {
                p.extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("json"))
            })
            .filter_map(|p| match read_json::<LevelData>(&p) {
                Ok(mut l) => {
                    l.id = p.file_stem()?.to_string_lossy().into_owned();
                    Some(l)
                }
                Err(e) => {
                    log::warn!("skipping level: {e}");
                    None
                }
            })
            .collect();
        levels.sort_by(|a, b| a.id.cmp(&b.id));
        levels
    }

    // -----------------------------------------------------------------------
    // Audio paths (port of Paths.inst / Paths.voices / Song.buildVoiceList)
    // -----------------------------------------------------------------------

    fn songs_audio_dir(&self, id: &str) -> PathBuf {
        self.root.join("songs").join(id.to_lowercase())
    }

    /// `songs/<id>/Inst[-<suffix>].ogg`. `instrumental` is the metadata's
    /// `characters.instrumental` (or an alt instrumental id).
    pub fn inst_path(&self, id: &str, instrumental: &str) -> PathBuf {
        let suffix = if instrumental.is_empty() {
            String::new()
        } else {
            format!("-{instrumental}")
        };
        self.songs_audio_dir(id).join(format!("Inst{suffix}.ogg"))
    }

    fn voices_path(&self, id: &str, suffix: &str) -> PathBuf {
        self.songs_audio_dir(id).join(format!("Voices{suffix}.ogg"))
    }

    /// Port of `SongDifficulty.buildVoiceList`: player voices, then opponent voices,
    /// falling back to a single shared `Voices.ogg`.
    pub fn voice_list(&self, id: &str, meta: &SongMetadata) -> Vec<PathBuf> {
        let suffix = variation_suffix(&meta.variation);
        let mut result = self.player_voices(id, meta);
        result.extend(self.opponent_voices(id, meta));
        if result.is_empty() && crate::vfs::is_file(&self.voices_path(id, "")) {
            // Faithful to the original: checks `Voices.ogg` but pushes the suffixed path.
            result.push(self.voices_path(id, &suffix));
        }
        result
    }

    /// Port of `buildPlayerVoiceList`.
    pub fn player_voices(&self, id: &str, meta: &SongMetadata) -> Vec<PathBuf> {
        let c = &meta.play_data.characters;
        self.resolve_voices(
            id,
            &c.player,
            c.player_vocals.as_deref(),
            &variation_suffix(&meta.variation),
        )
    }

    /// Port of `buildOpponentVoiceList`.
    pub fn opponent_voices(&self, id: &str, meta: &SongMetadata) -> Vec<PathBuf> {
        let c = &meta.play_data.characters;
        self.resolve_voices(
            id,
            &c.opponent,
            c.opponent_vocals.as_deref(),
            &variation_suffix(&meta.variation),
        )
    }

    /// Shared resolution logic for player / opponent voices.
    fn resolve_voices(
        &self,
        id: &str,
        char_id: &str,
        explicit: Option<&[String]>,
        suffix: &str,
    ) -> Vec<PathBuf> {
        if let Some(ids) = explicit {
            let paths: Vec<PathBuf> = ids
                .iter()
                .map(|c| self.voices_path(id, &format!("-{c}{suffix}")))
                .collect();
            if paths.iter().all(|p| crate::vfs::is_file(p)) {
                return paths;
            }
        }

        // Strip `-xxx` suffixes from the character id until a file exists:
        // `Voices-bf-car-erect` → `Voices-bf-erect`.
        let strip = |s: &str| -> String {
            let mut parts: Vec<&str> = s.split('-').collect();
            parts.pop();
            parts.join("-")
        };

        let mut cid = char_id.to_owned();
        let mut found = (!cid.is_empty()).then(|| self.voices_path(id, &format!("-{cid}{suffix}")));
        while let Some(p) = &found {
            if crate::vfs::is_file(p) {
                break;
            }
            cid = strip(&cid);
            found = (!cid.is_empty()).then(|| self.voices_path(id, &format!("-{cid}{suffix}")));
        }

        if found.is_none() {
            // Retry without the variation suffix. NOTE: the original re-applies the
            // suffix on subsequent attempts; that quirk is preserved here.
            cid = char_id.to_owned();
            found = (!cid.is_empty()).then(|| self.voices_path(id, &format!("-{cid}")));
            while let Some(p) = &found {
                if crate::vfs::is_file(p) {
                    break;
                }
                cid = strip(&cid);
                found = (!cid.is_empty()).then(|| self.voices_path(id, &format!("-{cid}{suffix}")));
            }
        }
        found.into_iter().collect()
    }
}
