//! Where this port keeps its own data.
//!
//! On Windows 11 this is `%LOCALAPPDATA%\Rusted Mic\` (local, not roaming:
//! the imported assets are large and should not sync across machines).

use std::path::PathBuf;

/// Folder name under the platform's local data directory.
pub const APP_DIR_NAME: &str = "Rusted Mic";

/// Environment variable that overrides the data directory (useful for tests
/// and portable installs).
pub const DATA_DIR_ENV: &str = "RUSTED_MIC_DATA_DIR";

/// Root data directory for the port.
pub fn data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os(DATA_DIR_ENV) {
        return PathBuf::from(p);
    }
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(APP_DIR_NAME)
}

/// Where imported assets are copied to.
pub fn imported_assets_dir() -> PathBuf {
    data_dir().join("assets")
}

/// The first-boot marker file.
pub fn marker_path() -> PathBuf {
    data_dir().join("install.json")
}

/// File name of the asset pack.
pub const PACK_FILE_NAME: &str = "assets.pak";

/// The asset pack: `assets.pak` next to the executable.
pub fn pack_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_owned()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(PACK_FILE_NAME)
}

/// Fonts extracted from the game executable (`<data_dir>/fonts`).
pub fn fonts_dir() -> PathBuf {
    data_dir().join("fonts")
}
