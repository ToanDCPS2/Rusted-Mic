//! A tiny virtual file system: game files are read from a mounted `.pak` (one
//! file holding every asset) when the path lies inside it, else from disk.
//!
//! An overlay folder can stand in for files the asset root lacks (audio the
//! official build embeds in its executable; see [`set_overlay`]).
//!
//! Mods ([`set_mods`]) sit on top, like Polymod: a mod's `images/x.png`
//! replaces `<root>/images/x.png`, directory listings include every layer, a
//! mod's `_append/<path>` is added to the end of a text file and
//! `_merge/<path>` patches a JSON file (a JSON Patch array, as Polymod reads).
//!
//! A mounted pack behaves like a folder at its own path: with the pack at
//! `C:\Game\assets.pak`, `C:\Game\assets.pak\data\songs\x.json` reads the entry
//! `data/songs/x.json`. Lookups ignore case, like the Windows file system.
//!
//! ## Format (little-endian)
//! ```text
//! header  "RMPAK\0\0\x01"  u64 index_offset  u64 index_len
//! data    entry blobs, each raw or a zstd frame
//! index   u32 count, then per entry:
//!         u16 path_len, path (UTF-8, '/'-separated), u8 codec, u64 offset, u64 stored_len, u64 raw_len
//! ```

use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, RwLock};

pub const MAGIC: &[u8; 8] = b"RMPAK\0\0\x01";
pub const HEADER_LEN: u64 = 24;
pub const CODEC_RAW: u8 = 0;
pub const CODEC_ZSTD: u8 = 1;

#[derive(Debug, Clone)]
pub struct PackEntry {
    pub path: String,
    pub codec: u8,
    pub offset: u64,
    pub stored_len: u64,
    pub raw_len: u64,
}

pub struct Pack {
    file: File,
    path: PathBuf,
    entries: Vec<PackEntry>,
    /// lower-case path → entry index
    by_path: HashMap<String, usize>,
    /// lower-case dir path ("" = root) → lower-case child name → (name, is_dir)
    dirs: HashMap<String, BTreeMap<String, (String, bool)>>,
}

fn read_at(file: &File, buf: &mut [u8], offset: u64) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileExt;
        let mut done = 0;
        while done < buf.len() {
            let n = file.seek_read(&mut buf[done..], offset + done as u64)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            done += n;
        }
        Ok(())
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileExt;
        file.read_exact_at(buf, offset)
    }
}

fn bad(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_owned())
}

impl Pack {
    pub fn open(path: &Path) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut head = [0u8; HEADER_LEN as usize];
        file.read_exact(&mut head)?;
        if &head[..8] != MAGIC {
            return Err(bad("not a Rusted Mic asset pack"));
        }
        let index_offset = u64::from_le_bytes(head[8..16].try_into().unwrap());
        let index_len = u64::from_le_bytes(head[16..24].try_into().unwrap());
        if index_len > 64 << 20 {
            return Err(bad("pack index too large"));
        }
        let mut index = vec![0u8; index_len as usize];
        read_at(&file, &mut index, index_offset)?;

        let mut cur = 0usize;
        let mut take = |n: usize| -> io::Result<&[u8]> {
            let s = index.get(cur..cur + n).ok_or_else(|| bad("truncated pack index"))?;
            cur += n;
            Ok(s)
        };
        let count = u32::from_le_bytes(take(4)?.try_into().unwrap()) as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
            let path = String::from_utf8(take(len)?.to_vec()).map_err(|_| bad("bad path in pack"))?;
            let codec = take(1)?[0];
            let offset = u64::from_le_bytes(take(8)?.try_into().unwrap());
            let stored_len = u64::from_le_bytes(take(8)?.try_into().unwrap());
            let raw_len = u64::from_le_bytes(take(8)?.try_into().unwrap());
            entries.push(PackEntry { path, codec, offset, stored_len, raw_len });
        }

        let mut by_path = HashMap::with_capacity(entries.len());
        let mut dirs: HashMap<String, BTreeMap<String, (String, bool)>> = HashMap::new();
        dirs.insert(String::new(), BTreeMap::new());
        for (i, e) in entries.iter().enumerate() {
            by_path.insert(e.path.to_lowercase(), i);
            let parts: Vec<&str> = e.path.split('/').collect();
            let mut dir = String::new();
            for (k, part) in parts.iter().enumerate() {
                let is_dir = k + 1 < parts.len();
                dirs.entry(dir.clone()).or_default().insert(part.to_lowercase(), (part.to_string(), is_dir));
                if is_dir {
                    if !dir.is_empty() {
                        dir.push('/');
                    }
                    dir.push_str(&part.to_lowercase());
                }
            }
        }
        Ok(Self { file, path: path.to_owned(), entries, by_path, dirs })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> &[PackEntry] {
        &self.entries
    }

    /// Total size of every file once unpacked.
    pub fn raw_size(&self) -> u64 {
        self.entries.iter().map(|e| e.raw_len).sum()
    }

    /// `path` relative to the pack ("" for the pack itself), lower-cased, or
    /// `None` if it lies outside.
    fn key(&self, path: &Path) -> Option<String> {
        let rel = path.strip_prefix(&self.path).ok()?;
        let mut parts: Vec<String> = Vec::new();
        for c in rel.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_string_lossy().to_lowercase()),
                Component::ParentDir => {
                    parts.pop()?;
                }
                _ => {}
            }
        }
        Some(parts.join("/"))
    }

    /// Read an entry by its pack-relative path (any case).
    pub fn read_entry(&self, rel: &str) -> io::Result<Vec<u8>> {
        let i = *self.by_path.get(&rel.to_lowercase()).ok_or(io::ErrorKind::NotFound)?;
        let e = &self.entries[i];
        let mut stored = vec![0u8; e.stored_len as usize];
        read_at(&self.file, &mut stored, e.offset)?;
        match e.codec {
            CODEC_RAW => Ok(stored),
            CODEC_ZSTD => zstd::bulk::decompress(&stored, e.raw_len as usize),
            _ => Err(bad("unknown codec in pack")),
        }
    }
}

static MOUNT: RwLock<Option<Arc<Pack>>> = RwLock::new(None);

/// Mount the pack at `path` (replacing any mounted one).
pub fn mount(path: &Path) -> io::Result<()> {
    let pack = Pack::open(path)?;
    *MOUNT.write().unwrap() = Some(Arc::new(pack));
    Ok(())
}

pub fn unmount() {
    *MOUNT.write().unwrap() = None;
}

/// (asset root, folder): files missing under the root are looked up in the folder.
static OVERLAY: RwLock<Option<(PathBuf, PathBuf)>> = RwLock::new(None);

/// Serve `dir/<rel>` as `root/<rel>` when the root has no such file.
pub fn set_overlay(root: &Path, dir: &Path) {
    *OVERLAY.write().unwrap() = Some((root.to_owned(), dir.to_owned()));
}

/// (asset root, mod folders lowest priority first).
static MODS: RwLock<Option<(PathBuf, Vec<PathBuf>)>> = RwLock::new(None);

/// Layer these mod folders over `root` (later ones win).
pub fn set_mods(root: &Path, mods: Vec<PathBuf>) {
    *MODS.write().unwrap() = if mods.is_empty() { None } else { Some((root.to_owned(), mods)) };
}

/// The mod folders currently layered over the assets.
pub fn mods() -> Vec<PathBuf> {
    MODS.read().unwrap().as_ref().map(|(_, m)| m.clone()).unwrap_or_default()
}

/// `path` relative to the mods' asset root, and the mod folders.
fn mod_rel(path: &Path) -> Option<(PathBuf, Vec<PathBuf>)> {
    let guard = MODS.read().unwrap();
    let (root, mods) = guard.as_ref()?;
    let rel = path.strip_prefix(root).ok()?.to_owned();
    Some((rel, mods.clone()))
}

/// The highest mod's own copy of `path`.
fn mod_file(path: &Path) -> Option<PathBuf> {
    let (rel, mods) = mod_rel(path)?;
    if rel.as_os_str().is_empty() {
        return None;
    }
    mods.iter().rev().map(|m| m.join(&rel)).find(|p| p.is_file())
}

/// Mods' `_append` and `_merge` edits to the file at `path`.
fn apply_mod_edits(path: &Path, data: Vec<u8>) -> Vec<u8> {
    let Some((rel, mods)) = mod_rel(path) else { return data };
    let mut data = data;
    for m in &mods {
        let append = m.join("_append").join(&rel);
        if let Ok(extra) = std::fs::read(&append) {
            if !data.ends_with(b"\n") && !data.is_empty() {
                data.push(b'\n');
            }
            data.extend_from_slice(&extra);
        }
        if rel.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")) {
            let merge = m.join("_merge").join(&rel);
            if let Ok(patch) = std::fs::read(&merge) {
                match json_merge(&data, &patch) {
                    Ok(d) => data = d,
                    Err(e) => log::warn!("{}: {e}", merge.display()),
                }
            }
        }
    }
    data
}

fn parse_json(b: &[u8]) -> Result<serde_json::Value, String> {
    let text = String::from_utf8_lossy(b);
    serde_json::from_str(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

/// Polymod JSON merge: a JSON Patch array (`add`, `remove`, `replace`,
/// `move`, `copy`, `test`), or an object merged key by key.
fn json_merge(base: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    use serde_json::Value;
    let mut doc = parse_json(base)?;
    match parse_json(patch)? {
        Value::Array(ops) => {
            for op in ops {
                if let Err(e) = json_patch_op(&mut doc, &op) {
                    log::warn!("json patch {op}: {e}");
                }
            }
        }
        p @ Value::Object(_) => merge_objects(&mut doc, p),
        _ => return Err("merge file is neither a patch list nor an object".into()),
    }
    serde_json::to_vec(&doc).map_err(|e| e.to_string())
}

fn merge_objects(doc: &mut serde_json::Value, patch: serde_json::Value) {
    match (doc, patch) {
        (serde_json::Value::Object(d), serde_json::Value::Object(p)) => {
            for (k, v) in p {
                merge_objects(d.entry(k).or_insert(serde_json::Value::Null), v);
            }
        }
        (d, p) => *d = p,
    }
}

fn pointer_parts(ptr: &str) -> Vec<String> {
    ptr.split('/').skip(1).map(|p| p.replace("~1", "/").replace("~0", "~")).collect()
}

/// The value at a split JSON pointer.
fn walk<'a>(doc: &'a mut serde_json::Value, parts: &[String]) -> Result<&'a mut serde_json::Value, String> {
    use serde_json::Value;
    let mut cur = doc;
    for p in parts {
        cur = match cur {
            Value::Object(m) => m.get_mut(p).ok_or_else(|| format!("no key {p}"))?,
            Value::Array(a) => a.get_mut(p.parse::<usize>().map_err(|_| format!("bad index {p}"))?).ok_or_else(|| format!("no index {p}"))?,
            _ => return Err(format!("{p}: not a container")),
        };
    }
    Ok(cur)
}

/// Insert / set at a JSON pointer (`-` appends to an array).
fn json_add(doc: &mut serde_json::Value, ptr: &str, value: serde_json::Value, replace: bool) -> Result<(), String> {
    use serde_json::Value;
    let parts = pointer_parts(ptr);
    let Some((last, parents)) = parts.split_last() else {
        *doc = value;
        return Ok(());
    };
    match walk(doc, parents)? {
        Value::Object(m) => {
            m.insert(last.clone(), value);
        }
        Value::Array(a) if last == "-" => a.push(value),
        Value::Array(a) => {
            let i: usize = last.parse().map_err(|_| format!("bad index {last}"))?;
            if replace {
                *a.get_mut(i).ok_or("index out of range")? = value;
            } else if i <= a.len() {
                a.insert(i, value);
            } else {
                return Err("index out of range".into());
            }
        }
        _ => return Err("not a container".into()),
    }
    Ok(())
}

fn json_remove(doc: &mut serde_json::Value, ptr: &str) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    let parts = pointer_parts(ptr);
    let (last, parents) = parts.split_last().ok_or("can't remove the root")?;
    match walk(doc, parents)? {
        Value::Object(m) => m.remove(last).ok_or_else(|| "no such key".into()),
        Value::Array(a) => {
            let i: usize = last.parse().map_err(|_| "bad index".to_string())?;
            (i < a.len()).then(|| a.remove(i)).ok_or_else(|| "index out of range".into())
        }
        _ => Err("not a container".into()),
    }
}

fn json_patch_op(doc: &mut serde_json::Value, op: &serde_json::Value) -> Result<(), String> {
    let kind = op.get("op").and_then(|v| v.as_str()).ok_or("no op")?;
    let path = op.get("path").and_then(|v| v.as_str()).ok_or("no path")?;
    let value = || op.get("value").cloned().ok_or("no value");
    match kind {
        "add" => json_add(doc, path, value()?, false),
        "replace" => json_add(doc, path, value()?, true),
        "remove" => json_remove(doc, path).map(|_| ()),
        "move" | "copy" => {
            let from = op.get("from").and_then(|v| v.as_str()).ok_or("no from")?;
            let v = if kind == "move" { json_remove(doc, from)? } else { doc.pointer(from).cloned().ok_or("no such path")? };
            json_add(doc, path, v, false)
        }
        "test" => Ok(()),
        _ => Err(format!("unknown op {kind}")),
    }
}

/// Where `path` would be in the overlay folder.
fn overlay(path: &Path) -> Option<PathBuf> {
    let guard = OVERLAY.read().unwrap();
    let (root, dir) = guard.as_ref()?;
    let rel = path.strip_prefix(root).ok()?;
    Some(dir.join(rel))
}

fn base_is_file(path: &Path) -> bool {
    match in_pack(path) {
        Some((pack, key)) => pack.by_path.contains_key(&key),
        None => path.is_file(),
    }
}

/// The mounted pack and the key for `path`, if `path` lies inside it.
fn in_pack(path: &Path) -> Option<(Arc<Pack>, String)> {
    let pack = MOUNT.read().unwrap().clone()?;
    let key = pack.key(path)?;
    Some((pack, key))
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    let data = match mod_file(path) {
        Some(m) => std::fs::read(m)?,
        None => read_base(path)?,
    };
    Ok(apply_mod_edits(path, data))
}

/// The file without any mod's changes.
fn read_base(path: &Path) -> io::Result<Vec<u8>> {
    if !base_is_file(path) {
        if let Some(o) = overlay(path).filter(|o| o.is_file()) {
            return std::fs::read(o);
        }
    }
    match in_pack(path) {
        Some((pack, key)) => pack.read_entry(&key).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
        None => std::fs::read(path),
    }
}

pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|_| bad("file is not UTF-8"))
}

pub fn is_file(path: &Path) -> bool {
    base_is_file(path) || overlay(path).is_some_and(|o| o.is_file()) || mod_file(path).is_some()
}

/// The mod supplying the file at `path`, if one replaces or adds it.
pub fn from_mod(path: &Path) -> Option<PathBuf> {
    let (rel, mods) = mod_rel(path)?;
    mods.iter().rev().find(|m| m.join(&rel).is_file()).cloned()
}

pub fn is_dir(path: &Path) -> bool {
    let base = match in_pack(path) {
        Some((pack, key)) => pack.dirs.contains_key(&key),
        None => path.is_dir(),
    };
    base || overlay(path).is_some_and(|o| o.is_dir()) || mod_rel(path).is_some_and(|(rel, mods)| mods.iter().any(|m| m.join(&rel).is_dir()))
}

pub fn exists(path: &Path) -> bool {
    is_file(path) || is_dir(path)
}

/// A directory listing entry.
#[derive(Debug, Clone)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

pub fn read_dir(path: &Path) -> io::Result<Vec<DirEntry>> {
    let base = read_dir_base(path);
    let Some((rel, mods)) = mod_rel(path) else { return base };
    // Every layer's entries, by name (case-insensitively, like the pack).
    let found_base = base.is_ok();
    let mut out = base.unwrap_or_default();
    let mut found = found_base;
    let mut seen: std::collections::HashSet<String> = out.iter().map(|e| e.name.to_lowercase()).collect();
    for m in &mods {
        let Ok(rd) = std::fs::read_dir(m.join(&rel)) else { continue };
        found = true;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if rel.as_os_str().is_empty() && name.starts_with('_') {
                // `_merge`, `_append`, `_polymod_meta.json`: not assets.
                continue;
            }
            if seen.insert(name.to_lowercase()) {
                out.push(DirEntry { path: path.join(&name), is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false), name });
            }
        }
    }
    if found {
        Ok(out)
    } else {
        Err(io::ErrorKind::NotFound.into())
    }
}

fn read_dir_base(path: &Path) -> io::Result<Vec<DirEntry>> {
    match in_pack(path) {
        Some((pack, key)) => {
            let children = pack.dirs.get(&key).ok_or(io::ErrorKind::NotFound)?;
            Ok(children
                .values()
                .map(|(name, is_dir)| DirEntry { name: name.clone(), path: path.join(name), is_dir: *is_dir })
                .collect())
        }
        None => {
            let mut out = Vec::new();
            for e in std::fs::read_dir(path)? {
                let e = e?;
                out.push(DirEntry {
                    name: e.file_name().to_string_lossy().into_owned(),
                    path: e.path(),
                    is_dir: e.file_type().map(|t| t.is_dir()).unwrap_or(false),
                });
            }
            Ok(out)
        }
    }
}

/// Streams files into a new pack.
pub struct PackWriter {
    out: io::BufWriter<File>,
    pos: u64,
    entries: Vec<PackEntry>,
}

impl PackWriter {
    pub fn create(path: &Path) -> io::Result<Self> {
        use std::io::Write;
        let mut out = io::BufWriter::new(File::create(path)?);
        out.write_all(&[0u8; HEADER_LEN as usize])?;
        Ok(Self { out, pos: HEADER_LEN, entries: Vec::new() })
    }

    /// Add a file; `compress` zstd-compresses it (kept raw if that doesn't help).
    pub fn add(&mut self, rel: &str, data: &[u8], compress: bool) -> io::Result<()> {
        use std::io::Write;
        let packed = if compress { zstd::bulk::compress(data, 19).ok().filter(|c| c.len() < data.len()) } else { None };
        let (codec, bytes) = match &packed {
            Some(c) => (CODEC_ZSTD, c.as_slice()),
            None => (CODEC_RAW, data),
        };
        self.out.write_all(bytes)?;
        self.entries.push(PackEntry {
            path: rel.replace('\\', "/"),
            codec,
            offset: self.pos,
            stored_len: bytes.len() as u64,
            raw_len: data.len() as u64,
        });
        self.pos += bytes.len() as u64;
        Ok(())
    }

    /// Bytes written so far.
    pub fn written(&self) -> u64 {
        self.pos
    }

    /// Write the index and header.
    pub fn finish(mut self) -> io::Result<u64> {
        use std::io::{Seek, SeekFrom, Write};
        let mut index = Vec::new();
        index.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        for e in &self.entries {
            let p = e.path.as_bytes();
            let len = u16::try_from(p.len()).map_err(|_| bad("path too long for pack"))?;
            index.extend_from_slice(&len.to_le_bytes());
            index.extend_from_slice(p);
            index.push(e.codec);
            index.extend_from_slice(&e.offset.to_le_bytes());
            index.extend_from_slice(&e.stored_len.to_le_bytes());
            index.extend_from_slice(&e.raw_len.to_le_bytes());
        }
        let index_offset = self.pos;
        self.out.write_all(&index)?;
        self.out.seek(SeekFrom::Start(0))?;
        self.out.write_all(MAGIC)?;
        self.out.write_all(&index_offset.to_le_bytes())?;
        self.out.write_all(&(index.len() as u64).to_le_bytes())?;
        self.out.flush()?;
        Ok(index_offset + index.len() as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = std::env::temp_dir().join(format!("rmpak-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let pak = dir.join("t.pak");
        let mut w = PackWriter::create(&pak).unwrap();
        let text = "{\"a\": 1}".repeat(200);
        w.add("data/songs/Bopeebo/meta.json", text.as_bytes(), true).unwrap();
        w.add("images/x.png", &[1, 2, 3], false).unwrap();
        w.finish().unwrap();

        let pack = Pack::open(&pak).unwrap();
        assert_eq!(pack.read_entry("DATA/songs/bopeebo/META.json").unwrap(), text.as_bytes());
        assert_eq!(pack.entries()[0].codec, CODEC_ZSTD);
        assert_eq!(pack.read_entry("images/x.png").unwrap(), vec![1, 2, 3]);
        assert_eq!(pack.key(&pak.join("data").join("..").join("images")), Some("images".into()));

        mount(&pak).unwrap();
        assert!(is_dir(&pak.join("data").join("songs")));
        assert!(is_file(&pak.join("images").join("x.png")));
        assert!(!is_file(&pak.join("images").join("y.png")));
        let names: Vec<String> = read_dir(&pak.join("data").join("songs")).unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["Bopeebo"]);
        unmount();
        std::fs::remove_dir_all(&dir).ok();
    }
}
