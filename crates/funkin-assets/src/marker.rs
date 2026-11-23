//! The `install.json` marker that lets later launches skip setup.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::import::ImportMode;
use crate::paths;

pub const MARKER_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallMarker {
    pub schema: u32,
    pub mode: ImportMode,
    pub source_install_dir: PathBuf,
    /// The directory gameplay reads from.
    pub asset_root: PathBuf,
    pub imported_at_unix: u64,
    pub file_count: u64,
    pub total_bytes: u64,
    pub song_count: usize,
    pub version_hint: Option<String>,
}

/// Outcome of the boot-time asset check.
#[derive(Debug, Clone)]
pub enum AssetStatus {
    /// Assets are present; play from this root.
    Ready(InstallMarker),
    /// Setup must run. The string explains why, for display to the user.
    SetupRequired(String),
}

pub fn write_marker(marker: &InstallMarker) -> io::Result<()> {
    let path = paths::marker_path();
    let json = serde_json::to_string_pretty(marker).map_err(io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, path)
}

pub fn load_marker() -> Option<InstallMarker> {
    let text = std::fs::read_to_string(paths::marker_path()).ok()?;
    serde_json::from_str(&text).ok()
}

/// Delete the marker (forces setup on next launch).
pub fn clear_marker() -> io::Result<()> {
    match std::fs::remove_file(paths::marker_path()) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        r => r,
    }
}

fn looks_usable(root: &Path) -> bool {
    use funkin_core::vfs;
    vfs::is_dir(&root.join("data").join("songs")) && vfs::is_dir(&root.join("songs"))
}

/// Mount `assets.pak` next to the executable if it is there and readable.
fn mount_pack() -> Option<PathBuf> {
    let pack = paths::pack_path();
    if !pack.is_file() {
        return None;
    }
    match funkin_core::vfs::mount(&pack) {
        Ok(()) if looks_usable(&pack) => Some(pack),
        Ok(()) => {
            funkin_core::vfs::unmount();
            log::warn!("{} holds no song data", pack.display());
            None
        }
        Err(e) => {
            log::warn!("{}: {e}", pack.display());
            None
        }
    }
}

/// Run on every boot: decide whether gameplay may start. An `assets.pak` next
/// to the executable wins (and is mounted); otherwise the marker is used.
pub fn resolve_asset_root() -> AssetStatus {
    if let Some(pack) = mount_pack() {
        let marker = load_marker().filter(|m| m.mode == ImportMode::Pack);
        return AssetStatus::Ready(match marker {
            Some(m) => InstallMarker { asset_root: pack, ..m },
            None => InstallMarker {
                schema: MARKER_SCHEMA,
                mode: ImportMode::Pack,
                source_install_dir: PathBuf::new(),
                asset_root: pack,
                imported_at_unix: 0,
                file_count: 0,
                total_bytes: 0,
                song_count: 0,
                version_hint: None,
            },
        });
    }
    let Some(marker) = load_marker() else {
        return AssetStatus::SetupRequired(
            "No game assets have been imported yet. This port does not include any of the game's art, \
             music or sounds: point it at your own copy of Friday Night Funkin' to continue."
                .into(),
        );
    };
    // Loose copies from before the pack existed (or a pack that has since been
    // deleted) are not used: the game only runs from assets.pak or a reference.
    if matches!(marker.mode, ImportMode::Copy | ImportMode::Pack) {
        return AssetStatus::SetupRequired(format!(
            "{} was not found next to Rusted Mic. Set up again to rebuild it.",
            paths::PACK_FILE_NAME
        ));
    }
    if marker.schema != MARKER_SCHEMA {
        return AssetStatus::SetupRequired(
            "The asset import is from an incompatible version of this port. Please import again."
                .into(),
        );
    }
    if !looks_usable(&marker.asset_root) {
        let why = match marker.mode {
            ImportMode::Reference => format!(
                "The game installation at '{}' can no longer be found. Was it moved or uninstalled?",
                marker.source_install_dir.display()
            ),
            ImportMode::Copy | ImportMode::Pack => format!("The imported assets at '{}' are missing or damaged.", marker.asset_root.display()),
        };
        return AssetStatus::SetupRequired(why);
    }
    AssetStatus::Ready(marker)
}
