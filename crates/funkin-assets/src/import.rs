//! Pack (or reference) the assets of a validated installation.
//!
//! The default import reads every file of the installation's `assets/` folder,
//! plus the TrueType fonts embedded in its executable, into one `assets.pak`
//! next to this program (see [`funkin_core::vfs`]). Text data (JSON, XML,
//! scripts) is zstd-compressed; audio, images and video are already compressed
//! and are stored as they are, so nothing loses quality or load speed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::marker::{write_marker, InstallMarker, MARKER_SCHEMA};
use crate::paths;
use crate::validate::{validate_installation, Installation, ValidationError};

/// How to make the assets available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImportMode {
    /// Pack `assets/` into `assets.pak` next to the executable. Survives the
    /// original installation being moved, updated or uninstalled.
    Pack,
    /// Older imports: a loose copy in the port's data directory.
    Copy,
    /// Read directly from the original installation (no extra disk space).
    Reference,
}

/// Progress callback payload.
#[derive(Debug, Clone, Copy)]
pub struct ImportProgress {
    pub files_done: u64,
    pub files_total: u64,
    pub bytes_done: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error(transparent)]
    Invalid(#[from] ValidationError),
    #[error("I/O error at '{path}': {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the imported copy failed validation: {0}")]
    PostCheck(ValidationError),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ImportError + '_ {
    move |source| ImportError::Io {
        path: path.to_owned(),
        source,
    }
}

/// Validate `source`, import its assets, and write the first-boot marker.
pub fn import_installation(
    source: &Path,
    mode: ImportMode,
    mut progress: impl FnMut(ImportProgress),
) -> Result<InstallMarker, ImportError> {
    let inst = validate_installation(source)?;
    let data_dir = paths::data_dir();
    fs::create_dir_all(&data_dir).map_err(io_err(&data_dir))?;

    let (asset_root, file_count, total_bytes) = match mode {
        ImportMode::Reference => (inst.asset_root.clone(), 0, 0),
        ImportMode::Pack => {
            let (pack, count, bytes) = build_pack(&inst, &mut progress)?;
            // The pack replaces any loose copy from an older import.
            for old in [paths::imported_assets_dir(), paths::fonts_dir()] {
                if old.is_dir() {
                    if let Err(e) = fs::remove_dir_all(&old) {
                        log::warn!("could not remove {}: {e}", old.display());
                    }
                }
            }
            (pack, count, bytes)
        }
        ImportMode::Copy => {
            let dest = paths::imported_assets_dir();
            let staging = data_dir.join("assets.importing");
            if staging.exists() {
                fs::remove_dir_all(&staging).map_err(io_err(&staging))?;
            }
            let files = collect_files(&inst.asset_root).map_err(io_err(&inst.asset_root))?;
            let total = files.len() as u64;
            let mut bytes = 0u64;
            for (i, rel) in files.iter().enumerate() {
                let from = inst.asset_root.join(rel);
                let to = staging.join(rel);
                if let Some(parent) = to.parent() {
                    fs::create_dir_all(parent).map_err(io_err(parent))?;
                }
                bytes += convert_file(&from, &to).map_err(io_err(&from))?;
                progress(ImportProgress {
                    files_done: i as u64 + 1,
                    files_total: total,
                    bytes_done: bytes,
                });
            }
            // Swap in atomically-ish: only replace the old copy once the new one is complete.
            if dest.exists() {
                fs::remove_dir_all(&dest).map_err(io_err(&dest))?;
            }
            fs::rename(&staging, &dest).map_err(io_err(&dest))?;
            (dest, total, bytes)
        }
    };

    post_check(&asset_root, &inst)?;

    // The game's TrueType fonts live inside the executable; pull them out too
    // (a pack already holds them under `fonts/`).
    if mode != ImportMode::Pack {
        match crate::fonts::extract_embedded_fonts(&inst.executable, &paths::fonts_dir()) {
            Ok(n) => log::info!("extracted {n} fonts from {}", inst.executable.display()),
            Err(e) => log::warn!("could not extract fonts: {e}"),
        }
    }

    // Audio and credits the executable embeds, extracted afresh for this install.
    let _ = fs::remove_file(crate::embedded::embedded_dir().join(".extracted"));
    let _ = fs::remove_file(crate::embedded::embedded_dir().join(".credits"));
    let _ = fs::remove_file(crate::embedded::embedded_dir().join("data").join("credits.json"));
    crate::embedded::prepare(&inst.install_dir, &asset_root);

    let marker = InstallMarker {
        schema: MARKER_SCHEMA,
        mode,
        source_install_dir: inst.install_dir.clone(),
        asset_root,
        imported_at_unix: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        file_count,
        total_bytes,
        song_count: inst.song_count,
        version_hint: inst.version_hint.clone(),
    };
    write_marker(&marker).map_err(io_err(&paths::marker_path()))?;
    Ok(marker)
}

/// The copied tree must still contain loadable song data.
fn post_check(asset_root: &Path, inst: &Installation) -> Result<(), ImportError> {
    let registry = funkin_core::song::SongRegistry::new(asset_root);
    if registry.list_song_ids().len() != inst.song_count {
        return Err(ImportError::PostCheck(ValidationError::MissingFolder(
            asset_root.join("data").join("songs"),
        )));
    }
    Ok(())
}

/// Extensions worth compressing (text); everything else is stored as is.
const TEXT_EXTENSIONS: &[&str] = &["json", "xml", "txt", "hxc", "hx", "hxs", "frag", "vert", "srt", "csv", "ini", "md", "lua", "fnt"];

fn is_text(rel: &Path) -> bool {
    rel.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| TEXT_EXTENSIONS.iter().any(|t| t.eq_ignore_ascii_case(e)))
}

/// Write `assets/` and the executable's fonts into `assets.pak` (via a temporary
/// file, so a failed import never leaves a broken pack). Mounts the new pack.
fn build_pack(inst: &Installation, progress: &mut impl FnMut(ImportProgress)) -> Result<(PathBuf, u64, u64), ImportError> {
    use funkin_core::vfs;
    let dest = paths::pack_path();
    let tmp = dest.with_extension("pak.tmp");
    let files = collect_files(&inst.asset_root).map_err(io_err(&inst.asset_root))?;
    let total = files.len() as u64 + 1;
    let mut w = vfs::PackWriter::create(&tmp).map_err(io_err(&tmp))?;
    let mut bytes = 0u64;
    for (i, rel) in files.iter().enumerate() {
        let from = inst.asset_root.join(rel);
        let data = fs::read(&from).map_err(io_err(&from))?;
        bytes += data.len() as u64;
        let key: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        w.add(&key.join("/"), &data, is_text(rel)).map_err(io_err(&tmp))?;
        progress(ImportProgress { files_done: i as u64 + 1, files_total: total, bytes_done: bytes });
    }
    match fs::read(&inst.executable) {
        Ok(exe) => {
            for (name, data) in crate::fonts::embedded_fonts(&exe) {
                w.add(&format!("fonts/{name}"), data, true).map_err(io_err(&tmp))?;
            }
        }
        Err(e) => log::warn!("could not read {} for fonts: {e}", inst.executable.display()),
    }
    w.finish().map_err(io_err(&tmp))?;
    progress(ImportProgress { files_done: total, files_total: total, bytes_done: bytes });
    // Windows can't replace a file that is open: drop the old mount first.
    vfs::unmount();
    if dest.exists() {
        fs::remove_file(&dest).map_err(io_err(&dest))?;
    }
    fs::rename(&tmp, &dest).map_err(io_err(&dest))?;
    vfs::mount(&dest).map_err(io_err(&dest))?;
    Ok((dest, files.len() as u64, bytes))
}

/// Recursively list files under `root` as relative paths (sorted for determinism).
fn collect_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        for entry in fs::read_dir(root.join(&rel))? {
            let entry = entry?;
            let ty = entry.file_type()?;
            let child = rel.join(entry.file_name());
            if ty.is_dir() {
                stack.push(child);
            } else if ty.is_file() {
                out.push(child);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Copy or convert a single file. Returns bytes written.
/// Hook point for future per-format conversion.
fn convert_file(from: &Path, to: &Path) -> io::Result<u64> {
    fs::copy(from, to)
}
