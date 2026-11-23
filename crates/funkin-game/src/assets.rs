//! Path resolution for the user's imported assets, mirroring `Paths.hx` libraries:
//! `default`/`preload` → `assets/`, any other library → `assets/<library>/`.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::audio::Pcm;
use crate::gfx::atlas::Atlas;
use crate::gfx::text::Font;
use crate::gfx::{Gfx, TexId};

pub struct Assets {
    pub root: PathBuf,
}

impl Assets {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn lib_dir(&self, lib: &str) -> PathBuf {
        match lib {
            "" | "default" | "preload" => self.root.clone(),
            other => self.root.join(other),
        }
    }

    /// Resolve `"lib:key"` (or bare `key`, searched in `shared` then `default`).
    fn resolve(&self, spec: &str, sub: &str, ext: &str) -> PathBuf {
        let (lib, key) = spec.split_once(':').unwrap_or(("", spec));
        let try_lib = |l: &str| self.lib_dir(l).join(sub).join(format!("{key}.{ext}"));
        if lib.is_empty() {
            let shared = try_lib("shared");
            if funkin_core::vfs::exists(&shared) {
                return shared;
            }
        }
        try_lib(lib)
    }

    /// `Paths.image` with `Paths.currentLevel`: a bare key is looked up in the
    /// level library (a stage `directory`), then `shared`, then `default`.
    /// An empty `ext` returns the folder path (Animate atlases).
    pub fn level_image(&self, spec: &str, level: Option<&str>, ext: &str) -> PathBuf {
        let (lib, key) = spec.split_once(':').unwrap_or(("", spec));
        let file = if ext.is_empty() { key.to_owned() } else { format!("{key}.{ext}") };
        let at = |l: &str| self.lib_dir(l).join("images").join(&file);
        if !lib.is_empty() {
            return at(lib);
        }
        for l in level.into_iter().chain(["shared"]) {
            let c = at(l);
            if funkin_core::vfs::exists(&c) {
                return c;
            }
        }
        at("")
    }

    pub fn image_path(&self, spec: &str) -> PathBuf {
        self.resolve(spec, "images", "png")
    }

    pub fn sound_path(&self, spec: &str) -> PathBuf {
        self.resolve(spec, "sounds", "ogg")
    }

    pub fn texture(&self, gfx: &mut Gfx, spec: &str) -> Result<TexId, String> {
        gfx.load_texture(&self.image_path(spec))
    }

    pub fn atlas(&self, gfx: &mut Gfx, spec: &str) -> Result<Atlas, String> {
        Atlas::load(gfx, &self.image_path(spec).with_extension(""))
    }

    /// An `AtlasText` font from `images/fonts/<name>`.
    pub fn font(&self, gfx: &mut Gfx, name: &str, upper: bool) -> Result<Font, String> {
        Font::load(gfx, &self.image_path(&format!("default:fonts/{name}")).with_extension(""), upper)
    }

    /// A TrueType font extracted at import: `<data>/fonts/<file>` (next to the
    /// copied assets, or in `%LOCALAPPDATA%\Rusted Mic\fonts`), or `assets/fonts/<file>`.
    pub fn ttf_path(&self, file: &str) -> Option<PathBuf> {
        let mut dirs = vec![self.root.join("fonts")];
        if let Some(p) = self.root.parent() {
            dirs.push(p.join("fonts"));
        }
        if let Some(l) = std::env::var_os("LOCALAPPDATA") {
            dirs.push(PathBuf::from(l).join("Rusted Mic").join("fonts"));
        }
        dirs.into_iter().map(|d| d.join(file)).find(|p| funkin_core::vfs::is_file(p))
    }

    /// `Paths.music(id/id)`.
    pub fn music_path(&self, id: &str) -> PathBuf {
        let p = self.root.join("music").join(id).join(format!("{id}.ogg"));
        if funkin_core::vfs::exists(&p) {
            return p;
        }
        let shared = self.root.join("shared").join("music").join(id).join(format!("{id}.ogg"));
        if funkin_core::vfs::exists(&shared) { shared } else { p }
    }

    pub fn sound(&self, spec: &str) -> Option<Arc<Pcm>> {
        let p = self.sound_path(spec);
        match Pcm::load_ogg(&p) {
            Ok(pcm) => Some(Arc::new(pcm)),
            Err(e) => {
                log::warn!("sound: {e}");
                None
            }
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
