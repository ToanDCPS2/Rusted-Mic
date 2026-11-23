//! Audio output with a sample-accurate song clock.
//!
//! Songs are decoded to PCM up front (OGG Vorbis via `lewton`) and mixed in
//! the cpal callback. The callback publishes how many frames it has rendered
//! and when, plus the device's reported output latency. From that, the game
//! thread computes the song position for any `Instant`, which is far more
//! precise than polling a player's position (what `FlxSound.time` does).

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// Decoded audio: interleaved stereo `f32` at `rate` Hz.
pub struct Pcm {
    pub samples: Vec<f32>,
    pub rate: u32,
}

impl Pcm {
    pub fn frames(&self) -> usize {
        self.samples.len() / 2
    }
    pub fn duration_ms(&self) -> f64 {
        self.frames() as f64 / self.rate as f64 * 1000.0
    }

    /// Decode an OGG Vorbis file.
    pub fn load_ogg(path: &Path) -> Result<Self, String> {
        let bytes = funkin_core::vfs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut reader = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(bytes))
            .map_err(|e| format!("{}: {e}", path.display()))?;
        let rate = reader.ident_hdr.audio_sample_rate;
        let channels = reader.ident_hdr.audio_channels as usize;
        let mut samples = Vec::new();
        while let Some(packet) = reader
            .read_dec_packet_itl()
            .map_err(|e| format!("{}: {e}", path.display()))?
        {
            for frame in packet.chunks(channels.max(1)) {
                let l = frame[0] as f32 / 32768.0;
                let r = if channels > 1 {
                    frame[1] as f32 / 32768.0
                } else {
                    l
                };
                samples.push(l);
                samples.push(r);
            }
        }
        Ok(Self { samples, rate })
    }
}

/// A track in the song mix with a live-adjustable volume.
pub struct Track {
    pub pcm: Pcm,
    volume: AtomicU32,
}

impl Track {
    pub fn new(pcm: Pcm) -> Arc<Self> {
        Arc::new(Self {
            pcm,
            volume: AtomicU32::new(1.0f32.to_bits()),
        })
    }
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.to_bits(), Ordering::Relaxed);
    }
    fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
}

enum Command {
    Play {
        tracks: Vec<Arc<Track>>,
        start_ms: f64,
        looped: Option<(f64, f64)>,
    },
    Stop,
    Sfx(Arc<Pcm>, f32),
    /// Freeze (or continue) the song tracks; sound effects and the overlay keep playing.
    Pause(bool),
    /// A second looping track over everything (the pause menu's music).
    Overlay(Option<(Arc<Track>, f64)>),
}

/// Clock state published by the audio thread.
struct Clock {
    /// Song position (device frames) at the start of the last callback. May be negative (pre-roll).
    frame: AtomicI64,
    /// When the last callback ran, in ns since `epoch()`.
    callback_ns: AtomicU64,
    /// Callback → DAC latency in ns.
    latency_ns: AtomicU64,
    playing: AtomicBool,
    paused: AtomicBool,
    /// Master gain (f32 bits).
    master: AtomicU32,
    /// How many `Play` commands the audio thread has applied; `frame` belongs
    /// to the latest once this matches [`Audio::play`]'s return value.
    generation: AtomicU64,
}

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

pub struct Audio {
    _stream: cpal::Stream,
    tx: Sender<Command>,
    clock: Arc<Clock>,
    rate: u32,
    /// `Play` commands sent so far.
    plays: AtomicU64,
}

struct Mixer {
    rx: Receiver<Command>,
    clock: Arc<Clock>,
    rate: u32,
    channels: usize,
    tracks: Vec<Arc<Track>>,
    frame: i64,
    /// Loop region `[start, end)` in ms; the clock keeps counting through it.
    looped: Option<(f64, f64)>,
    sfx: Vec<(Arc<Pcm>, f64, f32)>,
    paused: bool,
    overlay: Option<(Arc<Track>, f64)>,
    generation: u64,
}

impl Mixer {
    fn fill(&mut self, out: &mut [f32], info: &cpal::OutputCallbackInfo) {
        let now = Instant::now();
        while let Ok(cmd) = self.rx.try_recv() {
            match cmd {
                Command::Play { tracks, start_ms, looped } => {
                    self.tracks = tracks;
                    self.looped = looped.filter(|(a, b)| b > a);
                    self.frame = (start_ms / 1000.0 * self.rate as f64).round() as i64;
                    self.generation += 1;
                    self.clock.playing.store(true, Ordering::Release);
                }
                Command::Stop => {
                    self.tracks.clear();
                    self.clock.playing.store(false, Ordering::Release);
                }
                Command::Sfx(pcm, vol) => self.sfx.push((pcm, 0.0, vol)),
                Command::Pause(p) => {
                    self.paused = p;
                    self.clock.paused.store(p, Ordering::Release);
                }
                Command::Overlay(t) => self.overlay = t,
            }
        }
        let ts = info.timestamp();
        let latency = ts.playback.saturating_duration_since(ts.callback);
        self.clock
            .latency_ns
            .store(latency.as_nanos() as u64, Ordering::Relaxed);
        self.clock.frame.store(self.frame, Ordering::Relaxed);
        self.clock.callback_ns.store(
            now.duration_since(epoch()).as_nanos() as u64,
            Ordering::Release,
        );
        self.clock.generation.store(self.generation, Ordering::Release);

        let ch = self.channels.max(1);
        let rate = self.rate as f64;
        let master = f32::from_bits(self.clock.master.load(Ordering::Relaxed));
        let paused = self.paused;
        for frame in out.chunks_mut(ch) {
            let (mut l, mut r) = (0.0f32, 0.0f32);
            if self.frame >= 0 && !paused {
                let mut ms = self.frame as f64 / rate * 1000.0;
                if let Some((a, b)) = self.looped {
                    if ms >= b {
                        ms = a + (ms - a).rem_euclid(b - a);
                    }
                }
                for t in &self.tracks {
                    let src = ms / 1000.0 * t.pcm.rate as f64;
                    let (sl, sr) = sample(&t.pcm, src);
                    let v = t.volume();
                    l += sl * v;
                    r += sr * v;
                }
            }
            for (pcm, pos, vol) in &mut self.sfx {
                let (sl, sr) = sample(pcm, *pos);
                l += sl * *vol;
                r += sr * *vol;
                *pos += pcm.rate as f64 / rate;
            }
            if let Some((t, pos)) = &mut self.overlay {
                if *pos as usize >= t.pcm.frames() {
                    *pos = 0.0;
                }
                let (sl, sr) = sample(&t.pcm, *pos);
                let v = t.volume();
                l += sl * v;
                r += sr * v;
                *pos += t.pcm.rate as f64 / rate;
            }
            if !self.tracks.is_empty() && !paused {
                self.frame += 1;
            }
            frame[0] = (l * master).clamp(-1.0, 1.0);
            if ch > 1 {
                frame[1] = (r * master).clamp(-1.0, 1.0);
                for s in &mut frame[2..] {
                    *s = 0.0;
                }
            }
        }
        self.sfx
            .retain(|(pcm, pos, _)| (*pos as usize) < pcm.frames());
    }
}

/// Linear-interpolated stereo sample at fractional source frame `pos`.
#[inline]
fn sample(pcm: &Pcm, pos: f64) -> (f32, f32) {
    let i = pos as usize;
    let n = pcm.frames();
    if i + 1 >= n {
        return if i < n {
            (pcm.samples[i * 2], pcm.samples[i * 2 + 1])
        } else {
            (0.0, 0.0)
        };
    }
    let t = (pos - i as f64) as f32;
    let s = &pcm.samples[i * 2..i * 2 + 4];
    (s[0] + (s[2] - s[0]) * t, s[1] + (s[3] - s[1]) * t)
}

impl Audio {
    pub fn new() -> Result<Self, String> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no audio output device")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        let rate = supported.sample_rate();
        let channels = supported.channels() as usize;
        let config: cpal::StreamConfig = supported.into();

        let (tx, rx) = channel();
        let clock = Arc::new(Clock {
            frame: AtomicI64::new(0),
            callback_ns: AtomicU64::new(0),
            latency_ns: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            master: AtomicU32::new(1.0f32.to_bits()),
            generation: AtomicU64::new(0),
        });
        let _ = epoch();
        let mut mixer = Mixer {
            rx,
            clock: clock.clone(),
            rate,
            channels,
            tracks: Vec::new(),
            frame: 0,
            looped: None,
            sfx: Vec::new(),
            paused: false,
            overlay: None,
            generation: 0,
        };
        let stream = device
            .build_output_stream::<f32, _, _>(
                config,
                move |data: &mut [f32], info: &cpal::OutputCallbackInfo| mixer.fill(data, info),
                |e| log::error!("audio stream error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())?;
        stream.play().map_err(|e| e.to_string())?;
        Ok(Self {
            _stream: stream,
            tx,
            clock,
            rate,
            plays: AtomicU64::new(0),
        })
    }

    /// Start a song so that song time `start_ms` (negative = pre-roll silence) is
    /// at the head of the next buffer.
    /// Returns an id for [`Audio::started`].
    pub fn play(&self, tracks: Vec<Arc<Track>>, start_ms: f64) -> u64 {
        let _ = self.tx.send(Command::Play { tracks, start_ms, looped: None });
        self.plays.fetch_add(1, Ordering::AcqRel) + 1
    }

    /// Whether the audio thread has switched to the song [`Audio::play`]
    /// returned `id` for. Until then the clock still shows whatever played
    /// before (the menu music's position).
    pub fn started(&self, id: u64) -> bool {
        self.clock.generation.load(Ordering::Acquire) >= id
    }

    /// Start looping music: plays from `start_ms`, wrapping `[loop_start, loop_end)`.
    /// The clock (`song_time_at`) keeps increasing across loops.
    pub fn play_looped(&self, tracks: Vec<Arc<Track>>, start_ms: f64, loop_start: f64, loop_end: f64) {
        let _ = self.tx.send(Command::Play { tracks, start_ms, looped: Some((loop_start, loop_end)) });
        self.plays.fetch_add(1, Ordering::AcqRel);
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Command::Stop);
    }

    /// Freeze or continue the song (the clock stops with it).
    pub fn set_paused(&self, paused: bool) {
        // Publish immediately so the clock stops extrapolating this frame.
        self.clock.paused.store(paused, Ordering::Release);
        let _ = self.tx.send(Command::Pause(paused));
    }

    pub fn paused(&self) -> bool {
        self.clock.paused.load(Ordering::Acquire)
    }

    /// Loop `track` (from source frame `start`) over everything else; None stops it.
    pub fn set_overlay(&self, track: Option<(Arc<Track>, f64)>) {
        let _ = self.tx.send(Command::Overlay(track));
    }

    /// Master volume for everything (0..1, already curved).
    pub fn set_master(&self, gain: f32) {
        self.clock.master.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn play_sfx(&self, pcm: Arc<Pcm>, volume: f32) {
        let _ = self.tx.send(Command::Sfx(pcm, volume));
    }

    /// The song position (ms) that is audible at `at`.
    pub fn song_time_at(&self, at: Instant) -> f64 {
        let cb_ns = self.clock.callback_ns.load(Ordering::Acquire);
        let frame = self.clock.frame.load(Ordering::Relaxed);
        let latency_ms = self.clock.latency_ns.load(Ordering::Relaxed) as f64 / 1e6;
        let base_ms = frame as f64 / self.rate as f64 * 1000.0;
        let at_ns = at.saturating_duration_since(epoch()).as_nanos() as f64;
        // Never extrapolate further than ~100ms past the last callback (e.g. a stalled device).
        let since_ms = if self.clock.paused.load(Ordering::Acquire) {
            0.0
        } else {
            ((at_ns - cb_ns as f64) / 1e6).clamp(-100.0, 100.0)
        };
        base_ms + since_ms - latency_ms
    }
}
