//! Song data: metadata, charts, levels (weeks), and a registry that discovers
//! them from an asset directory.
//!
//! Ported from `funkin/data/song/SongData.hx`, `SongRegistry.hx`,
//! `funkin/data/story/level/LevelData.hx` and `funkin/play/song/Song.hx`.
//!
//! On-disk layout (relative to the asset root, i.e. the game's `assets/` folder):
//!
//! ```text
//! data/songs/<id>/<id>-metadata[-<variation>].json
//! data/songs/<id>/<id>-chart[-<variation>].json
//! data/levels/<levelId>.json
//! songs/<id>/Inst[-<suffix>].ogg
//! songs/<id>/Voices-<char>[-<variation>].ogg
//! ```

mod chart;
mod level;
mod metadata;
mod registry;

pub use chart::*;
pub use level::*;
pub use metadata::*;
pub use registry::*;

use thiserror::Error;

/// Errors produced while loading song data.
#[derive(Debug, Error)]
pub enum SongError {
    #[error("could not read {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not parse {path}: {source}")]
    Parse {
        path: std::path::PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("{path}: unsupported data version {found} (this port supports {supported})")]
    UnsupportedVersion {
        path: std::path::PathBuf,
        found: String,
        supported: &'static str,
    },
    #[error("song '{0}' was not found")]
    NotFound(String),
    #[error("song '{song}' has no chart for difficulty '{difficulty}' (variation '{variation}')")]
    MissingDifficulty {
        song: String,
        variation: String,
        difficulty: String,
    },
}

/// Read and deserialize a JSON file with good error context.
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
) -> Result<T, SongError> {
    let text = crate::vfs::read_to_string(path).map_err(|source| SongError::Io {
        path: path.to_owned(),
        source,
    })?;
    // Some shipped JSON files start with a UTF-8 BOM.
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    serde_json::from_str(text).map_err(|source| SongError::Parse {
        path: path.to_owned(),
        source,
    })
}

/// Check a semver string against a `major.minor.x`-style rule.
/// Mirrors `VersionUtil.validateVersion` for the rules used by the song registry.
pub(crate) fn check_version(
    path: &std::path::Path,
    version: &str,
    req: &'static str,
) -> Result<(), SongError> {
    let ok = semver::Version::parse(version.trim())
        .ok()
        .zip(semver::VersionReq::parse(req).ok())
        .map(|(v, r)| r.matches(&v))
        .unwrap_or(false);
    if ok {
        Ok(())
    } else {
        Err(SongError::UnsupportedVersion {
            path: path.to_owned(),
            found: version.to_owned(),
            supported: req,
        })
    }
}

/// `-<variation>` for non-default variations, empty otherwise.
pub fn variation_suffix(variation: &str) -> String {
    if variation.is_empty() || variation == crate::constants::DEFAULT_VARIATION {
        String::new()
    } else {
        format!("-{variation}")
    }
}
