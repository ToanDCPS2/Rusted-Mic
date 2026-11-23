//! Automatic discovery of installations in common locations.
//!
//! Windows 11 is the primary target. Locations scanned, in order:
//! * Steam: the install path from the registry plus every library listed in
//!   `steamapps\libraryfolders.vdf`, checking each `steamapps\common\*` folder
//! * itch app: `%APPDATA%\itch\apps\*`
//! * `%ProgramFiles%`, `%ProgramFiles(x86)%`, `%LOCALAPPDATA%\Programs`
//! * the user's Desktop, Downloads and Games folders
//!
//! Only one directory level below each location is checked, so scanning is fast.

use std::path::{Path, PathBuf};

use crate::validate::{validate_installation, Installation};

/// Scan common locations and return every valid installation found.
pub fn scan_common_locations() -> Vec<Installation> {
    let mut found: Vec<Installation> = Vec::new();
    for root in candidate_roots() {
        for dir in std::iter::once(root.clone()).chain(subdirs(&root)) {
            if let Ok(inst) = validate_installation(&dir) {
                let canon = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_owned());
                if !found
                    .iter()
                    .any(|f| canon(&f.install_dir) == canon(&inst.install_dir))
                {
                    log::info!("found installation at {}", inst.install_dir.display());
                    found.push(inst);
                }
            }
        }
    }
    found
}

fn subdirs(dir: &Path) -> impl Iterator<Item = PathBuf> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
}

fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for lib in steam_libraries() {
        roots.push(lib.join("steamapps").join("common"));
    }
    if let Some(appdata) = dirs::config_dir() {
        // %APPDATA%\itch\apps on Windows.
        roots.push(appdata.join("itch").join("apps"));
    }
    #[cfg(windows)]
    {
        for var in ["ProgramFiles", "ProgramFiles(x86)"] {
            if let Some(p) = std::env::var_os(var) {
                roots.push(PathBuf::from(p));
            }
        }
        if let Some(local) = dirs::data_local_dir() {
            roots.push(local.join("Programs"));
        }
    }
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Games"));
    }
    roots.extend(dirs::desktop_dir());
    roots.extend(dirs::download_dir());
    roots.retain(|p| p.is_dir());
    roots.dedup();
    roots
}

/// Steam library roots (each contains a `steamapps` folder).
pub fn steam_libraries() -> Vec<PathBuf> {
    let mut steam_roots: Vec<PathBuf> = Vec::new();

    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
        use winreg::RegKey;
        if let Ok(key) = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Software\Valve\Steam") {
            if let Ok(p) = key.get_value::<String, _>("SteamPath") {
                steam_roots.push(PathBuf::from(p));
            }
        }
        if let Ok(key) =
            RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(r"SOFTWARE\WOW6432Node\Valve\Steam")
        {
            if let Ok(p) = key.get_value::<String, _>("InstallPath") {
                steam_roots.push(PathBuf::from(p));
            }
        }
        steam_roots.push(PathBuf::from(r"C:\Program Files (x86)\Steam"));
    }
    #[cfg(not(windows))]
    {
        if let Some(home) = dirs::home_dir() {
            steam_roots.push(home.join(".steam/steam"));
            steam_roots.push(home.join(".local/share/Steam"));
            steam_roots.push(home.join("Library/Application Support/Steam"));
        }
    }

    let mut libs: Vec<PathBuf> = Vec::new();
    for root in steam_roots.into_iter().filter(|p| p.is_dir()) {
        let vdf = root.join("steamapps").join("libraryfolders.vdf");
        if let Ok(text) = std::fs::read_to_string(&vdf) {
            libs.extend(parse_library_folders(&text));
        }
        libs.push(root);
    }
    libs.retain(|p| p.join("steamapps").is_dir());
    libs.sort();
    libs.dedup();
    libs
}

/// Extract `"path"  "D:\\SteamLibrary"` entries from `libraryfolders.vdf`.
fn parse_library_folders(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split('"').filter(|s| !s.trim().is_empty());
            match (parts.next(), parts.next()) {
                (Some("path"), Some(v)) => Some(PathBuf::from(v.replace("\\\\", "\\"))),
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vdf_paths() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}";
        let libs = parse_library_folders(vdf);
        assert_eq!(
            libs,
            vec![
                PathBuf::from(r"C:\Program Files (x86)\Steam"),
                PathBuf::from(r"D:\SteamLibrary")
            ]
        );
    }
}
