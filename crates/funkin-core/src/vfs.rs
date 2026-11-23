//! A tiny virtual file system: game files are read from a mounted `.pak` (one
//! file holding every asset) when the path lies inside it, else from disk.
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

/// The mounted pack and the key for `path`, if `path` lies inside it.
fn in_pack(path: &Path) -> Option<(Arc<Pack>, String)> {
    let pack = MOUNT.read().unwrap().clone()?;
    let key = pack.key(path)?;
    Some((pack, key))
}

pub fn read(path: &Path) -> io::Result<Vec<u8>> {
    match in_pack(path) {
        Some((pack, key)) => pack.read_entry(&key).map_err(|e| io::Error::new(e.kind(), format!("{}: {e}", path.display()))),
        None => std::fs::read(path),
    }
}

pub fn read_to_string(path: &Path) -> io::Result<String> {
    String::from_utf8(read(path)?).map_err(|_| bad("file is not UTF-8"))
}

pub fn is_file(path: &Path) -> bool {
    match in_pack(path) {
        Some((pack, key)) => pack.by_path.contains_key(&key),
        None => path.is_file(),
    }
}

pub fn is_dir(path: &Path) -> bool {
    match in_pack(path) {
        Some((pack, key)) => pack.dirs.contains_key(&key),
        None => path.is_dir(),
    }
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
