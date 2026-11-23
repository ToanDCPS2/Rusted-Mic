//! Port of `funkin/Conductor.hx`.
//!
//! The conductor converts a song position (ms) into steps/beats/measures,
//! honouring BPM and time-signature changes, and reports step/beat/measure
//! boundaries crossed since the last update.
//!
//! Unlike the original, this conductor never reads the audio clock itself: the
//! frontend passes the position in. That keeps it deterministic and testable,
//! and lets the frontend choose the most accurate clock available (e.g. the
//! audio device's sample counter, interpolated between callbacks).

use crate::constants::*;
use crate::round_decimal;
use crate::song::SongTimeChange;

/// A time change with its beat position resolved (`beatTime` is always set).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeChange {
    pub time_stamp: f64,
    pub beat_time: f64,
    pub bpm: f64,
    pub num: u32,
    pub den: u32,
}

impl TimeChange {
    #[inline]
    pub fn beat_length_ms(&self) -> f64 {
        (SECS_PER_MIN / self.bpm) * MS_PER_SEC * (4.0 / self.den as f64)
    }
    #[inline]
    pub fn step_length_ms(&self) -> f64 {
        self.beat_length_ms() / STEPS_PER_BEAT
    }
    #[inline]
    pub fn steps_per_measure(&self) -> f64 {
        self.num as f64 * STEPS_PER_BEAT
    }
}

/// Boundaries crossed during an [`Conductor::update`] call
/// (`onStepHit` / `onBeatHit` / `onMeasureHit` in the original).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConductorHits {
    pub step: bool,
    pub beat: bool,
    pub measure: bool,
}

#[derive(Debug, Clone)]
pub struct Conductor {
    time_changes: Vec<TimeChange>,
    current: Option<usize>,
    bpm_override: Option<f64>,

    /// Current position in ms, offsets applied.
    pub song_position: f64,
    pub current_step: i64,
    pub current_beat: i64,
    pub current_measure: i64,
    pub current_step_time: f64,
    pub current_beat_time: f64,
    pub current_measure_time: f64,

    /// Per-song instrumental offset from metadata (ms).
    pub instrumental_offset: f64,
    /// Offset compensating for codec delay (ms).
    pub format_offset: f64,
    /// User-configured global offset (ms, `Preferences.globalOffset`).
    pub global_offset: f64,
}

impl Default for Conductor {
    fn default() -> Self {
        Self::new()
    }
}

impl Conductor {
    pub fn new() -> Self {
        Self {
            time_changes: Vec::new(),
            current: None,
            bpm_override: None,
            song_position: 0.0,
            current_step: 0,
            current_beat: 0,
            current_measure: 0,
            current_step_time: 0.0,
            current_beat_time: 0.0,
            current_measure_time: 0.0,
            instrumental_offset: 0.0,
            format_offset: 0.0,
            global_offset: 0.0,
        }
    }

    pub fn time_changes(&self) -> &[TimeChange] {
        &self.time_changes
    }

    pub fn current_time_change(&self) -> Option<&TimeChange> {
        self.current.map(|i| &self.time_changes[i])
    }

    pub fn force_bpm(&mut self, bpm: Option<f64>) {
        self.bpm_override = bpm;
    }

    pub fn bpm(&self) -> f64 {
        self.bpm_override
            .or_else(|| self.current_time_change().map(|t| t.bpm))
            .unwrap_or(DEFAULT_BPM)
    }

    pub fn starting_bpm(&self) -> f64 {
        self.bpm_override
            .or_else(|| self.time_changes.first().map(|t| t.bpm))
            .unwrap_or(DEFAULT_BPM)
    }

    pub fn time_signature_numerator(&self) -> u32 {
        self.current_time_change()
            .map(|t| t.num)
            .unwrap_or(DEFAULT_TIME_SIGNATURE_NUM)
    }

    pub fn time_signature_denominator(&self) -> u32 {
        self.current_time_change()
            .map(|t| t.den)
            .unwrap_or(DEFAULT_TIME_SIGNATURE_DEN)
    }

    pub fn beat_length_ms(&self) -> f64 {
        (SECS_PER_MIN / self.bpm()) * MS_PER_SEC * (4.0 / self.time_signature_denominator() as f64)
    }

    pub fn step_length_ms(&self) -> f64 {
        self.beat_length_ms() / STEPS_PER_BEAT
    }

    pub fn measure_length_ms(&self) -> f64 {
        self.beat_length_ms() * self.time_signature_numerator() as f64
    }

    pub fn steps_per_measure(&self) -> f64 {
        self.time_signature_numerator() as f64 * STEPS_PER_BEAT
    }

    /// `instrumentalOffset + formatOffset + globalOffset`.
    pub fn combined_offset(&self) -> f64 {
        self.instrumental_offset + self.format_offset + self.global_offset
    }

    /// Port of `mapTimeChanges`: sort, clamp and resolve each change's beat time.
    pub fn map_time_changes(&mut self, changes: &[SongTimeChange]) {
        let mut sorted: Vec<SongTimeChange> = changes.to_vec();
        sorted.sort_by(|a, b| a.time_stamp.total_cmp(&b.time_stamp));
        self.time_changes.clear();

        for c in sorted {
            let time_stamp = c.time_stamp.max(0.0);
            let beat_time = match self.time_changes.last() {
                Some(prev) if time_stamp > 0.0 => round_decimal(
                    prev.beat_time
                        + (time_stamp - prev.time_stamp) * prev.bpm / SECS_PER_MIN / MS_PER_SEC
                            * (prev.den as f64 / 4.0),
                    4,
                ),
                _ => 0.0,
            };
            self.time_changes.push(TimeChange {
                time_stamp,
                beat_time,
                bpm: c.bpm,
                num: c.time_signature_num.max(1),
                den: c.time_signature_den.max(1),
            });
        }
        if self.time_changes.is_empty() {
            log::warn!("Conductor mapped no time changes; using {DEFAULT_BPM} BPM");
        }
        let pos = self.song_position;
        self.update(pos, false);
    }

    /// Port of `update`. `song_pos` is the raw audio position in ms; when
    /// `apply_offsets` is true, [`Self::combined_offset`] is added to it.
    pub fn update(&mut self, song_pos: f64, apply_offsets: bool) -> ConductorHits {
        let song_pos = if apply_offsets {
            song_pos + self.combined_offset()
        } else {
            song_pos
        };

        let (old_step, old_beat, old_measure) =
            (self.current_step, self.current_beat, self.current_measure);
        self.song_position = song_pos;

        self.current = if self.time_changes.is_empty() {
            None
        } else {
            Some(0)
        };
        if self.song_position > 0.0 {
            for (i, tc) in self.time_changes.iter().enumerate() {
                if self.song_position >= tc.time_stamp {
                    self.current = Some(i);
                } else {
                    break;
                }
            }
        }

        match self.current_time_change().copied() {
            Some(tc) if self.song_position > 0.0 => {
                self.current_step_time = round_decimal(
                    tc.beat_time * STEPS_PER_BEAT
                        + (self.song_position - tc.time_stamp) / self.step_length_ms(),
                    6,
                );
                self.current_beat_time = self.current_step_time / STEPS_PER_BEAT;
                self.current_measure_time = self.time_in_measures(self.song_position);
            }
            _ => {
                self.current_step_time =
                    round_decimal(self.song_position / self.step_length_ms(), 4);
                self.current_beat_time = self.current_step_time / STEPS_PER_BEAT;
                self.current_measure_time = self.current_step_time / self.steps_per_measure();
            }
        }
        self.current_step = self.current_step_time.floor() as i64;
        self.current_beat = self.current_beat_time.floor() as i64;
        self.current_measure = self.current_measure_time.floor() as i64;

        ConductorHits {
            step: self.current_step != old_step,
            beat: self.current_beat != old_beat,
            measure: self.current_measure != old_measure,
        }
    }

    /// Index of the last time change at or before `ms`.
    fn change_at_ms(&self, ms: f64) -> usize {
        self.time_changes
            .iter()
            .rposition(|t| ms >= t.time_stamp)
            .unwrap_or(0)
    }

    /// Port of `getTimeInSteps`.
    pub fn time_in_steps(&self, ms: f64) -> f64 {
        if self.time_changes.is_empty() {
            return (ms / self.step_length_ms()).floor();
        }
        let ms = ms.max(0.0);
        let tc = &self.time_changes[self.change_at_ms(ms)];
        tc.beat_time * STEPS_PER_BEAT + (ms - tc.time_stamp) / tc.step_length_ms()
    }

    /// Port of `getStepTimeInMs`.
    pub fn step_time_in_ms(&self, step: f64) -> f64 {
        if self.time_changes.is_empty() {
            return step * self.step_length_ms();
        }
        let idx = self
            .time_changes
            .iter()
            .rposition(|t| step >= t.beat_time * STEPS_PER_BEAT)
            .unwrap_or(0);
        let tc = &self.time_changes[idx];
        tc.time_stamp + (step - tc.beat_time * STEPS_PER_BEAT) * tc.step_length_ms()
    }

    /// Port of `getBeatTimeInMs`.
    pub fn beat_time_in_ms(&self, beat: f64) -> f64 {
        self.step_time_in_ms(beat * STEPS_PER_BEAT)
    }

    /// Port of `getTimeInMeasures` (measures depend on each segment's time signature).
    pub fn time_in_measures(&self, ms: f64) -> f64 {
        if self.time_changes.is_empty() {
            return ms / self.step_length_ms() / self.steps_per_measure();
        }
        let ms = ms.max(0.0);
        let mut result = 0.0;
        let mut last = self.time_changes[0];
        for tc in &self.time_changes[1..] {
            if ms < tc.time_stamp {
                break;
            }
            result += (tc.time_stamp - last.time_stamp)
                / last.step_length_ms()
                / last.steps_per_measure();
            last = *tc;
        }
        result + (ms - last.time_stamp) / last.step_length_ms() / last.steps_per_measure()
    }

    /// Port of `getMeasureTimeInMs`.
    pub fn measure_time_in_ms(&self, measure: f64) -> f64 {
        if self.time_changes.is_empty() {
            return measure * self.step_length_ms() * self.steps_per_measure();
        }
        let measure = measure.max(0.0);
        let mut last = self.time_changes[0];
        let mut last_measure = 0.0;
        for tc in &self.time_changes[1..] {
            let m = last_measure
                + (tc.time_stamp - last.time_stamp)
                    / last.step_length_ms()
                    / last.steps_per_measure();
            if measure < m {
                break;
            }
            last = *tc;
            last_measure = m;
        }
        last.time_stamp
            + (measure - last_measure) * last.step_length_ms() * last.steps_per_measure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tc(t: f64, bpm: f64) -> SongTimeChange {
        SongTimeChange::new(t, bpm)
    }

    #[test]
    fn single_bpm() {
        let mut c = Conductor::new();
        c.map_time_changes(&[tc(0.0, 120.0)]);
        // 120 BPM: 500ms per beat, 125ms per step.
        assert_eq!(c.step_length_ms(), 125.0);
        let hits = c.update(1000.0, false);
        assert!(hits.beat && hits.step);
        assert_eq!(c.current_beat, 2);
        assert_eq!(c.current_step, 8);
        assert_eq!(c.current_measure, 0);
        assert_eq!(c.time_in_steps(1000.0), 8.0);
        assert_eq!(c.step_time_in_ms(8.0), 1000.0);
    }

    #[test]
    fn bpm_change() {
        let mut c = Conductor::new();
        // 120 BPM for 2s (4 beats), then 60 BPM.
        c.map_time_changes(&[tc(0.0, 120.0), tc(2000.0, 60.0)]);
        assert_eq!(c.time_changes()[1].beat_time, 4.0);
        c.update(3000.0, false);
        assert_eq!(c.bpm(), 60.0);
        assert_eq!(c.current_beat, 5);
        assert_eq!(c.beat_time_in_ms(5.0), 3000.0);
        assert!((c.time_in_measures(3000.0) - 1.25).abs() < 1e-9);
        assert!((c.measure_time_in_ms(1.25) - 3000.0).abs() < 1e-9);
    }

    #[test]
    fn offsets() {
        let mut c = Conductor::new();
        c.map_time_changes(&[tc(0.0, 100.0)]);
        c.global_offset = -20.0;
        c.instrumental_offset = 5.0;
        c.update(1000.0, true);
        assert_eq!(c.song_position, 985.0);
    }
}
