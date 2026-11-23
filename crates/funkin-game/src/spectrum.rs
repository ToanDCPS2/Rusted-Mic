//! A small spectrum analyser for the A-Bot speaker's visualizer, modelled on
//! `funkin.vis.dsp.SpectralAnalyzer` as `ABotVis` configures it: 7 log-spaced bars
//! from 10 Hz to 22 kHz, a 256-point FFT, levels mapped from -65..-25 dB to 0..1.

use crate::audio::Pcm;

const FFT_N: usize = 256;
const MIN_DB: f32 = -65.0;
const MAX_DB: f32 = -25.0;
const MIN_FREQ: f32 = 10.0;
const MAX_FREQ: f32 = 22000.0;
/// Largest change of a bar level per update (`maxDelta`).
const MAX_DELTA: f32 = 0.1;

pub struct Analyzer<const BARS: usize> {
    pub levels: [f32; BARS],
}

impl<const BARS: usize> Default for Analyzer<BARS> {
    fn default() -> Self {
        Self { levels: [0.0; BARS] }
    }
}

/// In-place radix-2 FFT.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (s, c) = (ang * k as f32).sin_cos();
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * c - im[b] * s;
                let ti = re[b] * s + im[b] * c;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
            }
        }
        len <<= 1;
    }
}

impl<const BARS: usize> Analyzer<BARS> {
    /// Analyse the audio playing at `pos_ms` and move the bars toward it.
    pub fn update(&mut self, pcm: &Pcm, pos_ms: f64) {
        let mut re = [0.0f32; FFT_N];
        let mut im = [0.0f32; FFT_N];
        let frames = pcm.frames();
        let start = (pos_ms / 1000.0 * pcm.rate as f64) as i64;
        if pos_ms >= 0.0 && (start as usize) < frames {
            for (i, r) in re.iter_mut().enumerate() {
                let f = start as usize + i;
                if f >= frames {
                    break;
                }
                let mono = (pcm.samples[f * 2] + pcm.samples[f * 2 + 1]) * 0.5;
                // Hann window.
                let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (FFT_N - 1) as f32).cos();
                *r = mono * w;
            }
        }
        fft(&mut re, &mut im);
        let bin_hz = pcm.rate as f32 / FFT_N as f32;
        let mag = |k: usize| (re[k] * re[k] + im[k] * im[k]).sqrt() * 4.0 / FFT_N as f32;
        for (bar, level) in self.levels.iter_mut().enumerate() {
            let ratio = MAX_FREQ / MIN_FREQ;
            let lo = MIN_FREQ * ratio.powf(bar as f32 / BARS as f32);
            let hi = MIN_FREQ * ratio.powf((bar + 1) as f32 / BARS as f32);
            let k0 = ((lo / bin_hz).floor() as usize).clamp(1, FFT_N / 2 - 1);
            let k1 = ((hi / bin_hz).ceil() as usize).clamp(k0, FFT_N / 2 - 1);
            let peak = (k0..=k1).map(mag).fold(0.0f32, f32::max);
            let db = 20.0 * peak.max(1e-9).log10();
            let target = ((db - MIN_DB) / (MAX_DB - MIN_DB)).clamp(0.0, 1.0);
            *level += (target - *level).clamp(-MAX_DELTA, MAX_DELTA);
        }
    }
}
