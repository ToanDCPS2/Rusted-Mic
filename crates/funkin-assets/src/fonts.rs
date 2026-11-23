//! Fonts: the official build embeds its TrueType fonts (`5by7`, `VCR OSD Mono`, ...)
//! inside `Funkin.exe` rather than shipping `.ttf` files. This finds each embedded
//! sfnt blob in the user's own executable and writes it out as `<Family>.ttf`.

use std::fs;
use std::io;
use std::path::Path;

fn be16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes(d.get(o..o + 2)?.try_into().ok()?))
}

fn be32(d: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(d.get(o..o + 4)?.try_into().ok()?))
}

/// If a TrueType font starts at `o`, its length and family name.
fn sfnt_at(data: &[u8], o: usize) -> Option<(usize, String)> {
    if be32(data, o)? != 0x0001_0000 {
        return None;
    }
    let n = be16(data, o + 4)? as usize;
    if !(4..=40).contains(&n) {
        return None;
    }
    let (mut end, mut name, mut glyf) = (0usize, None, false);
    for i in 0..n {
        let r = o + 12 + 16 * i;
        let tag = data.get(r..r + 4)?;
        if !tag.iter().all(|c| c.is_ascii_alphanumeric() || *c == b' ' || *c == b'/') {
            return None;
        }
        let (off, len) = (be32(data, r + 8)? as usize, be32(data, r + 12)? as usize);
        if off > 20_000_000 || len > 20_000_000 {
            return None;
        }
        end = end.max(off + len);
        match tag {
            b"name" => name = Some(off),
            b"glyf" | b"CFF " => glyf = true,
            _ => {}
        }
    }
    let blob = data.get(o..o + end)?;
    let (noff, true) = (name?, glyf) else { return None };
    let count = be16(blob, noff + 2)? as usize;
    let strings = noff + be16(blob, noff + 4)? as usize;
    let mut family = None;
    for i in 0..count {
        let r = noff + 6 + 12 * i;
        let (pid, nid) = (be16(blob, r)?, be16(blob, r + 6)?);
        let (len, off) = (be16(blob, r + 8)? as usize, be16(blob, r + 10)? as usize);
        if nid != 1 && nid != 4 {
            continue;
        }
        let raw = blob.get(strings + off..strings + off + len)?;
        let s = if pid == 0 || pid == 3 {
            String::from_utf16_lossy(&raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect::<Vec<_>>())
        } else {
            raw.iter().map(|&b| b as char).collect()
        };
        family = Some(s);
        if nid == 4 {
            break;
        }
    }
    Some((end, family?))
}

/// File name for a font family: `VCR OSD Mono` → `VCR_OSD_Mono.ttf`.
pub fn font_file_name(family: &str) -> String {
    let mut s: String = family.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    while s.contains("__") {
        s = s.replace("__", "_");
    }
    format!("{}.ttf", s.trim_matches('_'))
}

/// Every embedded TrueType font in `data`, as (file name, bytes).
pub fn embedded_fonts(data: &[u8]) -> Vec<(String, &[u8])> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 12 < data.len() {
        if data[i] == 0 && data[i + 1] == 1 && data[i + 2] == 0 && data[i + 3] == 0 {
            if let Some((len, family)) = sfnt_at(data, i) {
                out.push((font_file_name(&family), &data[i..i + len]));
                i += len;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Extract every embedded TrueType font from `exe` into `out`. Returns how many.
pub fn extract_embedded_fonts(exe: &Path, out: &Path) -> io::Result<usize> {
    let data = fs::read(exe)?;
    fs::create_dir_all(out)?;
    let fonts = embedded_fonts(&data);
    for (name, bytes) in &fonts {
        fs::write(out.join(name), bytes)?;
    }
    Ok(fonts.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(font_file_name("VCR OSD Mono"), "VCR_OSD_Mono.ttf");
        assert_eq!(font_file_name("5by7"), "5by7.ttf");
    }

    #[test]
    fn rejects_garbage() {
        let mut d = vec![0u8; 64];
        d[1] = 1;
        assert!(sfnt_at(&d, 0).is_none());
    }
}
