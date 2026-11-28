//! Embedded audio: the official build keeps a few songs (Spaghetti's) inside
//! `Funkin.exe` instead of under `assets/`. The install's `manifest/*.json`
//! still lists them with their sizes; this finds the Ogg stream of that size in
//! the user's own executable and writes it out under the port's data folder,
//! which [`funkin_core::vfs::set_overlay`] then serves under the asset root.
//!
//! The credits are hardcoded the same way; [`crate::credits`] recovers them as
//! `data/credits.json` in the same folder.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// `%xx` escapes, as Haxe's serializer writes strings.
fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Audio files a manifest lists that aren't on disk: (path under `assets/`, size).
pub fn missing_audio(install_dir: &Path) -> Vec<(String, u64)> {
    let mut out = Vec::new();
    let Ok(dir) = fs::read_dir(install_dir.join("manifest")) else { return out };
    for e in dir.flatten() {
        let Ok(text) = fs::read_to_string(e.path()) else { continue };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else { continue };
        let Some(list) = v.get("assets").and_then(|a| a.as_str()) else { continue };
        let s = url_decode(list);
        // Each entry is a serialized object. Embedded ones (no file on disk) give
        // their size before the path: `go R2 i<size> ... y<n>:assets/x.ogg`.
        for (start, _) in s.match_indices(":assets/") {
            let path_start = start + 1;
            let rest = &s[path_start..];
            let Some(end) = rest.find(".ogg") else { continue };
            let rel = &rest[..end + 4];
            if rel.contains(':') || !rel.starts_with("assets/") {
                continue;
            }
            let entry_start = s[..start].rfind("go").map_or(0, |i| i + 2);
            let entry = &s[entry_start..start];
            let Some(size) = entry.split('i').skip(1).find_map(|t| {
                let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
                (digits.len() > 3).then(|| digits.parse::<u64>().ok()).flatten()
            }) else {
                continue;
            };
            let key = rel.trim_start_matches("assets/").to_owned();
            if !install_dir.join("assets").join(&key).is_file() && !out.iter().any(|(k, _)| *k == key) {
                out.push((key, size));
            }
        }
    }
    out
}

/// Every complete Ogg stream in `data`, as (offset, length).
fn ogg_streams(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = data[i..].windows(4).position(|w| w == b"OggS") {
        let start = i + p;
        // Beginning-of-stream page: follow pages to the end-of-stream one.
        if data.get(start + 5).is_some_and(|f| f & 2 != 0) {
            let mut j = start;
            let mut done = false;
            while j + 27 <= data.len() && &data[j..j + 4] == b"OggS" {
                let n = data[j + 26] as usize;
                let Some(segs) = data.get(j + 27..j + 27 + n) else { break };
                let eos = data[j + 5] & 4 != 0;
                j += 27 + n + segs.iter().map(|&s| s as usize).sum::<usize>();
                if eos {
                    done = j <= data.len();
                    break;
                }
            }
            if done {
                out.push((start, j - start));
                i = j;
                continue;
            }
        }
        i = start + 4;
    }
    out
}

/// Embedded audio for `install_dir`'s executable `exe`: (path under `assets/`, bytes).
pub fn embedded_audio(install_dir: &Path, exe: &[u8]) -> Vec<(String, Vec<u8>)> {
    let want = missing_audio(install_dir);
    if want.is_empty() {
        return Vec::new();
    }
    let streams = ogg_streams(exe);
    let mut out = Vec::new();
    for (key, size) in want {
        // Sizes are unique in practice; skip anything ambiguous.
        let hits: Vec<_> = streams.iter().filter(|(_, len)| *len as u64 == size).collect();
        if let [(off, len)] = hits[..] {
            out.push((key, exe[*off..*off + *len].to_vec()));
        } else {
            log::warn!("embedded audio {key}: {} streams of {size} bytes", hits.len());
        }
    }
    out
}

/// Where extracted embedded files live.
pub fn embedded_dir() -> PathBuf {
    crate::paths::data_dir().join("embedded")
}

/// Extract the install's embedded audio into [`embedded_dir`] (once) and serve
/// it under `asset_root`.
pub fn prepare(install_dir: &Path, asset_root: &Path) {
    let dir = embedded_dir();
    let done = dir.join(".extracted");
    if !done.is_file() {
        if let Err(e) = extract(install_dir, &dir) {
            log::warn!("embedded audio: {e}");
        } else {
            let _ = fs::write(&done, b"");
        }
    }
    // Tried once per import (installs from before this also get it once).
    let credits_done = dir.join(".credits");
    if !credits_done.is_file() {
        match extract_credits(install_dir, &dir) {
            Ok(true) => {}
            Ok(false) => log::warn!("no hardcoded credits found in the game executable"),
            Err(e) => log::warn!("credits: {e}"),
        }
        let _ = fs::create_dir_all(&dir);
        let _ = fs::write(&credits_done, b"");
    }
    funkin_core::vfs::set_overlay(asset_root, &dir);
}

fn executable(install_dir: &Path) -> io::Result<PathBuf> {
    crate::validate::EXECUTABLE_NAMES
        .iter()
        .map(|n| install_dir.join(n))
        .find(|p| p.is_file())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no game executable"))
}

/// Write the executable's hardcoded credits to `dir/data/credits.json`.
fn extract_credits(install_dir: &Path, dir: &Path) -> io::Result<bool> {
    let exe = executable(install_dir)?;
    let Some(credits) = crate::credits::extract_credits(&fs::read(&exe)?) else { return Ok(false) };
    let to = dir.join("data").join("credits.json");
    fs::create_dir_all(to.parent().unwrap())?;
    fs::write(&to, serde_json::to_string_pretty(&credits).map_err(io::Error::other)?)?;
    let n = credits["entries"].as_array().map_or(0, Vec::len);
    log::info!("extracted {n} credits sections from {}", exe.display());
    Ok(true)
}

fn extract(install_dir: &Path, dir: &Path) -> io::Result<usize> {
    let exe = executable(install_dir)?;
    let data = fs::read(&exe)?;
    let files = embedded_audio(install_dir, &data);
    for (key, bytes) in &files {
        let to = dir.join(key);
        if let Some(p) = to.parent() {
            fs::create_dir_all(p)?;
        }
        fs::write(&to, bytes)?;
    }
    log::info!("extracted {} embedded audio files from {}", files.len(), exe.display());
    Ok(files.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes() {
        assert_eq!(url_decode("assets%2Fsongs%2Fa.ogg"), "assets/songs/a.ogg");
    }

    #[test]
    fn finds_streams() {
        // Two one-page streams (BOS|EOS) with one 3-byte segment each.
        let page = |b: u8| {
            let mut p = b"OggS".to_vec();
            p.extend([0, 6]);
            p.extend([0u8; 20]);
            p.push(1);
            p.push(3);
            p.extend([b; 3]);
            p
        };
        let mut d = vec![9u8; 5];
        d.extend(page(1));
        d.extend([7u8; 4]);
        d.extend(page(2));
        let s = ogg_streams(&d);
        assert_eq!(s, vec![(5, 31), (40, 31)]);
    }

    /// Needs the real game: `RUSTED_MIC_TEST_ASSETS=<install>/assets`.
    #[test]
    #[ignore]
    fn spaghetti_is_embedded() {
        let Some(root) = std::env::var_os("RUSTED_MIC_TEST_ASSETS") else { return };
        let install = Path::new(&root).parent().unwrap().to_owned();
        let missing = missing_audio(&install);
        assert!(missing.iter().any(|(k, _)| k == "songs/spaghetti/Inst.ogg"), "{missing:?}");
        let exe = fs::read(install.join("Funkin.exe")).unwrap();
        let got = embedded_audio(&install, &exe);
        assert_eq!(got.len(), missing.len());
        assert!(got.iter().all(|(_, b)| b.starts_with(b"OggS")));
    }
}
