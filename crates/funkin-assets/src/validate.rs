//! Checks that a folder is an official Funkin' installation.

use std::path::{Path, PathBuf};

use funkin_core::constants::DEFAULT_VARIATION;
use funkin_core::song::SongRegistry;
use thiserror::Error;

/// Executable names of official desktop builds.
pub const EXECUTABLE_NAMES: &[&str] = &["Funkin.exe", "Funkin", "Funkin.app"];

/// Songs that every official build ships with; at least one must load.
pub const REFERENCE_SONGS: &[&str] = &["tutorial", "bopeebo", "fresh", "dadbattle"];

/// A validated installation.
#[derive(Debug, Clone)]
pub struct Installation {
    /// The folder containing the executable.
    pub install_dir: PathBuf,
    /// `<install_dir>/assets` (or the macOS bundle's resources folder).
    pub asset_root: PathBuf,
    pub executable: PathBuf,
    pub song_count: usize,
    /// `generatedBy` of a reference song's metadata, as a rough version hint.
    pub version_hint: Option<String>,
}

#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("'{0}' does not exist or is not a folder")]
    NotAFolder(PathBuf),
    #[error(
        "'{0}' does not contain the game executable (looked for {names}).\n\
         Select the folder where Friday Night Funkin' is installed, i.e. the one containing Funkin.exe.",
        names = EXECUTABLE_NAMES.join(", ")
    )]
    NoExecutable(PathBuf),
    #[error("'{0}' has no 'assets' folder. This doesn't look like a complete official build.")]
    NoAssets(PathBuf),
    #[error("missing expected folder '{0}'. The installation may be incomplete or modified.")]
    MissingFolder(PathBuf),
    #[error(
        "none of the base-game songs ({songs}) could be loaded from '{path}'.\n\
         Only official builds using song data format 2.x are supported.\nLast error: {last}",
        songs = REFERENCE_SONGS.join(", ")
    )]
    NoReferenceSong { path: PathBuf, last: String },
    #[error("song '{song}' has no instrumental at '{path}'. Audio files are missing.")]
    MissingAudio { song: String, path: PathBuf },
}

/// Normalize what the user picked: accept the install folder, the `assets`
/// folder inside it, or the executable itself.
pub fn normalize_selection(path: &Path) -> PathBuf {
    let mut p = path.to_path_buf();
    if p.is_file() {
        if let Some(parent) = p.parent() {
            p = parent.to_path_buf();
        }
    }
    if p.file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("assets"))
    {
        if let Some(parent) = p.parent() {
            p = parent.to_path_buf();
        }
    }
    p
}

/// Validate an installation folder. Returns a descriptive error for the first
/// failed check so the setup screen can explain exactly what is wrong.
pub fn validate_installation(path: &Path) -> Result<Installation, ValidationError> {
    let install_dir = normalize_selection(path);
    if !install_dir.is_dir() {
        return Err(ValidationError::NotAFolder(install_dir));
    }

    let executable = EXECUTABLE_NAMES
        .iter()
        .map(|n| install_dir.join(n))
        .find(|p| p.exists())
        .ok_or_else(|| ValidationError::NoExecutable(install_dir.clone()))?;

    // macOS bundles keep assets inside the app.
    let asset_root = if executable.extension().is_some_and(|e| e == "app") {
        executable.join("Contents").join("Resources").join("assets")
    } else {
        install_dir.join("assets")
    };
    if !asset_root.is_dir() {
        return Err(ValidationError::NoAssets(install_dir));
    }
    for sub in [["data", "songs"], ["songs", ""], ["shared", ""]] {
        let p = sub
            .iter()
            .filter(|s| !s.is_empty())
            .fold(asset_root.clone(), |acc, s| acc.join(s));
        if !p.is_dir() {
            return Err(ValidationError::MissingFolder(p));
        }
    }

    let registry = SongRegistry::new(&asset_root);
    let mut last_err = String::from("no song data found");
    let mut reference = None;
    for id in REFERENCE_SONGS {
        match registry.load_metadata(id, DEFAULT_VARIATION) {
            Ok(meta) => {
                reference = Some((id.to_string(), meta));
                break;
            }
            Err(e) => last_err = e.to_string(),
        }
    }
    let (ref_id, ref_meta) = reference.ok_or_else(|| ValidationError::NoReferenceSong {
        path: asset_root.clone(),
        last: last_err,
    })?;

    let inst = registry.inst_path(&ref_id, &ref_meta.play_data.characters.instrumental);
    if !inst.is_file() {
        return Err(ValidationError::MissingAudio {
            song: ref_id,
            path: inst,
        });
    }

    Ok(Installation {
        song_count: registry.list_song_ids().len(),
        version_hint: Some(ref_meta.generated_by).filter(|s| !s.is_empty()),
        install_dir,
        asset_root,
        executable,
    })
}
