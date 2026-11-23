//! Things drawn over every screen: the debug display (`FunkinDebugDisplay`),
//! the volume tray (`FunkinSoundTray`) and screenshots (`ScreenshotPlugin`).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Instant;

use crate::assets::Assets;
use crate::audio::{Audio, Pcm};
use crate::gfx::ttf::TtfFont;
use crate::gfx::{Gfx, TexId, VIEW_W, WHITE};
use crate::menus::{smooth_lerp, Tween};
use crate::settings::{DebugDisplay, Settings};

/// Resident memory of this process (`MemoryUtil.getTaskMemory`), in bytes.
fn task_memory() -> Option<u64> {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows_sys::Win32::System::Threading::GetCurrentProcess;
        let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        if K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
            return Some(c.WorkingSetSize as u64);
        }
        None
    }
    #[cfg(not(windows))]
    None
}

/// `FlxStringUtil.formatBytes(...).toLowerCase()`.
fn format_bytes(b: f64) -> String {
    let units = ["b", "kb", "mb", "gb", "tb"];
    let mut v = b;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    format!("{:.2}{}", (v * 100.0).round() / 100.0, units[i])
}

const GRAPH_LEN: usize = 216;

pub struct DebugOverlay {
    font: Option<TtfFont>,
    times: VecDeque<Instant>,
    since_update: f32,
    fps: u32,
    fps_peak: u32,
    mem: f64,
    mem_peak: f64,
    fps_hist: VecDeque<f32>,
    mem_hist: VecDeque<f32>,
}

impl DebugOverlay {
    pub fn new() -> Self {
        // 'Monsterrat' isn't shipped with the game: OpenFL falls back to a system sans.
        let font = ["C:\\Windows\\Fonts\\segoeui.ttf", "C:\\Windows\\Fonts\\arial.ttf"]
            .iter()
            .find_map(|p| TtfFont::load(std::path::Path::new(p)).ok());
        Self {
            font,
            times: VecDeque::new(),
            since_update: 1.0,
            fps: 0,
            fps_peak: 0,
            mem: 0.0,
            mem_peak: 0.0,
            fps_hist: VecDeque::new(),
            mem_hist: VecDeque::new(),
        }
    }

    pub fn frame(&mut self, dt: f32) {
        let now = Instant::now();
        self.times.push_back(now);
        while self.times.front().is_some_and(|t| now.duration_since(*t).as_secs_f32() > 1.0) {
            self.times.pop_front();
        }
        self.since_update += dt;
        if self.since_update < 0.1 {
            return;
        }
        self.since_update = 0.0;
        self.fps = self.times.len() as u32;
        self.fps_peak = self.fps_peak.max(self.fps);
        if let Some(m) = task_memory() {
            self.mem = m as f64;
            self.mem_peak = self.mem_peak.max(self.mem);
        }
        for (h, v) in [(&mut self.fps_hist, self.fps as f32), (&mut self.mem_hist, self.mem as f32)] {
            h.push_back(v);
            if h.len() > GRAPH_LEN {
                h.pop_front();
            }
        }
    }

    pub fn draw(&self, gfx: &mut Gfx, mode: DebugDisplay, bg: f32) {
        if mode == DebugDisplay::Off {
            return;
        }
        let Some(font) = &self.font else { return };
        let advanced = mode == DebugDisplay::Advanced;
        let (x, y) = (10.0, 10.0);
        // Task memory only (no GC numbers in Rust): 234 wide, 0.2 / 0.7 of 201 tall.
        let (iw, ih) = (234.0, 201.0 * if advanced { 0.7 } else { 0.2 });
        let to_rgb = |c: u32, a: f32| [((c >> 16) & 255) as f32 / 255.0, ((c >> 8) & 255) as f32 / 255.0, (c & 255) as f32 / 255.0, a];
        gfx.rect(x, y, iw + 6.0, ih + 6.0, to_rgb(0x3D3F41, bg));
        gfx.rect(x + 3.0, y + 3.0, iw, ih, to_rgb(0x2C2F30, bg));
        let mem = format!("TASK MEM: {} / {}", format_bytes(self.mem), format_bytes(self.mem_peak));
        let line = |gfx: &mut Gfx, s: &str, ly: f32| {
            font.draw(gfx, s, x + 8.0, y + ly, 13.0, WHITE);
        };
        if !advanced {
            line(gfx, &format!("FPS: {}", self.fps), 8.0);
            line(gfx, &mem, 24.0);
            return;
        }
        let avg = if self.fps_hist.is_empty() { 0.0 } else { self.fps_hist.iter().sum::<f32>() / self.fps_hist.len() as f32 };
        let mut sorted: Vec<f32> = self.fps_hist.iter().copied().collect();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let low = sorted.get(sorted.len() / 100).copied().unwrap_or(0.0);
        line(gfx, &format!("FPS: {}", self.fps), 8.0);
        line(gfx, &format!("AVG FPS: {}", avg.floor()), 24.0);
        line(gfx, &format!("1% LOW FPS: {}", low.floor()), 40.0);
        self.graph(gfx, &self.fps_hist, self.fps_peak as f32, x + 8.0, y + 57.0);
        line(gfx, &mem, 92.0);
        self.graph(gfx, &self.mem_hist, self.mem_peak as f32, x + 8.0, y + 110.0);
    }

    fn graph(&self, gfx: &mut Gfx, h: &VecDeque<f32>, max: f32, x: f32, y: f32) {
        let (w, gh) = (GRAPH_LEN as f32, 25.0);
        gfx.rect(x, y + gh, w, 1.0, [1.0, 1.0, 1.0, 0.5]);
        gfx.rect(x, y, 1.0, gh, [1.0, 1.0, 1.0, 0.5]);
        let max = max.max(1.0);
        for (i, v) in h.iter().enumerate() {
            let bh = (v / max).clamp(0.0, 1.0) * gh;
            gfx.rect(x + 1.0 + i as f32, y + gh - bh, 1.0, 1.5, WHITE);
        }
    }
}

/// `FunkinSoundTray`: the volume box sliding down from the top.
pub struct SoundTray {
    box_tex: Option<TexId>,
    bars: Vec<TexId>,
    y: f32,
    target_y: f32,
    alpha: f32,
    target_alpha: f32,
    timer: f32,
    up: Option<Arc<Pcm>>,
    down: Option<Arc<Pcm>>,
    max: Option<Arc<Pcm>>,
}

const TRAY_SCALE: f32 = 0.3;

impl SoundTray {
    pub fn new(gfx: &mut Gfx, assets: &Assets) -> Self {
        let box_tex = assets.texture(gfx, "default:soundtray/volumebox").ok();
        let bars = (1..=10).filter_map(|i| assets.texture(gfx, &format!("default:soundtray/bars_{i}")).ok()).collect();
        let h = box_tex.map(|t| gfx.texture_size(t).1 * TRAY_SCALE).unwrap_or(50.0);
        Self {
            box_tex,
            bars,
            y: -h - 10.0,
            target_y: -h - 10.0,
            alpha: 0.0,
            target_alpha: 0.0,
            timer: 0.0,
            up: assets.sound("default:soundtray/Volup"),
            down: assets.sound("default:soundtray/Voldown"),
            max: assets.sound("default:soundtray/VolMAX"),
        }
    }

    /// Volume changed (`showIncrement` / `showDecrement`).
    pub fn show(&mut self, audio: &Audio, settings: &Settings, up: bool) {
        self.timer = 1.0;
        self.target_y = 10.0;
        self.target_alpha = 1.0;
        let s = if settings.volume == 10 { &self.max } else if up { &self.up } else { &self.down };
        if let Some(s) = s {
            audio.play_sfx(s.clone(), 1.0);
        }
    }

    pub fn update(&mut self, gfx: &Gfx, dt: f32, settings: &Settings) {
        let h = self.box_tex.map(|t| gfx.texture_size(t).1 * TRAY_SCALE).unwrap_or(50.0);
        let audible = !settings.muted && settings.volume > 0;
        if audible {
            if self.timer > 0.0 {
                self.timer -= dt;
                if self.timer <= 0.0 {
                    self.target_y = -h - 10.0;
                    self.target_alpha = 0.0;
                }
            }
        } else {
            // Muted: the tray stays down.
            self.timer = 1.0;
            self.target_y = 10.0;
            self.target_alpha = 1.0;
        }
        self.y = smooth_lerp(self.y, self.target_y, dt, 0.768);
        self.alpha = smooth_lerp(self.alpha, self.target_alpha, dt, 0.307);
    }

    pub fn draw(&self, gfx: &mut Gfx, settings: &Settings) {
        let Some(b) = self.box_tex else { return };
        if self.alpha <= 0.01 {
            return;
        }
        let (w, _) = gfx.texture_size(b);
        let x = (VIEW_W - w * TRAY_SCALE) / 2.0;
        let a = [1.0, 1.0, 1.0, self.alpha];
        gfx.sprite(b, x, self.y, TRAY_SCALE, a);
        let level = if settings.muted { 0 } else { settings.volume.clamp(0, 10) as usize };
        if let Some(&back) = self.bars.last() {
            gfx.sprite(back, x + 9.0, self.y + 5.0, TRAY_SCALE, [1.0, 1.0, 1.0, self.alpha * 0.4]);
        }
        for (i, &bar) in self.bars.iter().enumerate() {
            if i < level {
                gfx.sprite(bar, x + 9.0, self.y + 5.0, TRAY_SCALE, a);
            }
        }
    }
}

/// `screenshots/` next to the executable.
pub fn screenshot_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_owned()))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("screenshots")
}

/// `DateUtil.generateTimestamp`, in local time.
fn timestamp() -> String {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut t = std::mem::zeroed();
        GetLocalTime(&mut t);
        return format!("{:04}-{:02}-{:02} {:02}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond);
    }
    #[allow(unreachable_code)]
    {
        let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        s.to_string()
    }
}

const PREVIEW_INITIAL_DELAY: f32 = 0.25;
const PREVIEW_FADE_IN: f32 = 0.3;
const PREVIEW_HOLD: f32 = 1.25;
const PREVIEW_FADE_OUT: f32 = 0.3;

pub struct Screenshots {
    sfx: Option<Arc<Pcm>>,
    /// A capture was requested for this frame.
    pub pending: bool,
    flash_at: Option<f32>,
    preview: Option<TexId>,
    preview_size: (f32, f32),
    preview_at: Option<f32>,
    /// Waiting for the save to finish before showing the preview.
    saving: Option<Receiver<bool>>,
    hide_mouse_until: Option<f32>,
    time: f32,
    /// The preview's rectangle on the canvas while it's visible (click opens the folder).
    pub preview_rect: Option<[f32; 4]>,
}

impl Screenshots {
    pub fn new(assets: &Assets) -> Self {
        Self {
            sfx: assets.sound("default:screenshot"),
            pending: false,
            flash_at: None,
            preview: None,
            preview_size: (0.0, 0.0),
            preview_at: None,
            saving: None,
            hide_mouse_until: None,
            time: 0.0,
            preview_rect: None,
        }
    }

    pub fn request(&mut self, gfx: &mut Gfx, settings: &Settings) {
        self.pending = true;
        gfx.request_capture();
        if settings.screenshot_hide_mouse {
            gfx.window().set_cursor_visible(false);
            self.hide_mouse_until = Some(self.time + 0.5);
        }
    }

    /// After present: pick up the captured frame and save it.
    pub fn after_present(&mut self, gfx: &mut Gfx, audio: &Audio, settings: &Settings) {
        if !std::mem::take(&mut self.pending) {
            return;
        }
        let Some((w, h, rgba)) = gfx.take_capture() else {
            log::warn!("screenshot: this GPU can't read back the window");
            return;
        };
        // Feedback: a quick flash (white only with flashing lights) and the shutter sound.
        if settings.flashing_lights {
            self.flash_at = Some(self.time);
        }
        if let Some(s) = &self.sfx {
            audio.play_sfx(s.clone(), 1.0);
        }
        // The preview thumbnail: a fifth of the screen.
        let (pw, ph) = (w / 5, h / 5);
        if pw > 0 && ph > 0 {
            let img = image::RgbaImage::from_raw(w, h, rgba.clone()).map(|i| image::imageops::thumbnail(&i, pw, ph));
            if let Some(t) = img {
                match self.preview {
                    Some(id) => gfx.replace_rgba(id, pw, ph, &t),
                    None => self.preview = Some(gfx.upload_rgba(pw, ph, &t)),
                }
                self.preview_size = (pw as f32, ph as f32);
            }
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let dir = screenshot_dir();
            let _ = std::fs::create_dir_all(&dir);
            let base = format!("screenshot-{}", timestamp());
            let mut path = dir.join(format!("{base}.png"));
            let mut n = 1;
            while path.exists() {
                n += 1;
                path = dir.join(format!("{base} ({n}).png"));
            }
            let ok = image::save_buffer(&path, &rgba, w, h, image::ColorType::Rgba8).map_err(|e| log::warn!("screenshot: {e}")).is_ok();
            if ok {
                log::info!("saved {}", path.display());
            }
            let _ = tx.send(ok);
        });
        if settings.screenshot_fancy_preview {
            if settings.screenshot_preview_on_save {
                self.saving = Some(rx);
                self.preview_at = None;
            } else {
                self.preview_at = Some(self.time);
            }
        }
    }

    pub fn update(&mut self, gfx: &Gfx, dt: f32) {
        self.time += dt;
        if let Some(rx) = &self.saving {
            if let Ok(ok) = rx.try_recv() {
                self.saving = None;
                if ok {
                    self.preview_at = Some(self.time);
                }
            }
        }
        if self.hide_mouse_until.is_some_and(|t| self.time >= t) {
            self.hide_mouse_until = None;
            gfx.window().set_cursor_visible(true);
        }
    }

    pub fn draw(&mut self, gfx: &mut Gfx) {
        if let Some(at) = self.flash_at {
            let a = 1.0 - (self.time - at) / 0.15;
            if a > 0.0 {
                gfx.rect(0.0, 0.0, VIEW_W, crate::gfx::VIEW_H, [1.0, 1.0, 1.0, a]);
            } else {
                self.flash_at = None;
            }
        }
        self.preview_rect = None;
        let (Some(at), Some(tex)) = (self.preview_at, self.preview) else { return };
        let e = self.time - at - PREVIEW_INITIAL_DELAY;
        if e < 0.0 {
            return;
        }
        let (alpha, dy) = if e < PREVIEW_FADE_IN {
            let k = Tween::new(0.0, 1.0, 0.0, PREVIEW_FADE_IN, "quartOut").at(e);
            (k, -10.0 * (1.0 - k))
        } else if e < PREVIEW_FADE_IN + PREVIEW_HOLD {
            (1.0, 0.0)
        } else if e < PREVIEW_FADE_IN + PREVIEW_HOLD + PREVIEW_FADE_OUT {
            let k = Tween::new(0.0, 1.0, 0.0, PREVIEW_FADE_OUT, "quartInOut").at(e - PREVIEW_FADE_IN - PREVIEW_HOLD);
            (1.0 - k, 10.0 * k)
        } else {
            self.preview_at = None;
            return;
        };
        // Sizes are in window pixels in the original; scale to the canvas.
        let k = VIEW_W / (self.preview_size.0 * 5.0).max(1.0);
        let (pw, ph) = (self.preview_size.0 * k, self.preview_size.1 * k);
        let (x, y) = (5.0, 5.0 + dy);
        gfx.rect(x, y, pw + 10.0, ph + 10.0, [1.0, 1.0, 1.0, alpha]);
        gfx.image(tex, [0.0, 0.0, self.preview_size.0, self.preview_size.1], x + 5.0, y + 5.0, pw, ph, [1.0, 1.0, 1.0, alpha]);
        self.preview_rect = Some([x, y, pw + 10.0, ph + 10.0]);
    }
}
