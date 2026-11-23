//! Runtime note state (port of the logic-relevant fields of `NoteSprite` / `SustainTrail`).

use crate::constants::HIT_WINDOW_MS;
use crate::song::SongNoteData;

/// Lanes in strumline order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum NoteDirection {
    Left = 0,
    Down = 1,
    Up = 2,
    Right = 3,
}

impl NoteDirection {
    pub const ALL: [NoteDirection; 4] = [Self::Left, Self::Down, Self::Up, Self::Right];

    pub fn from_index(i: usize) -> Option<Self> {
        Self::ALL.get(i).copied()
    }
    pub fn index(self) -> usize {
        self as usize
    }
    /// Lowercase name, as used by animation and asset names.
    pub fn name(self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Down => "down",
            Self::Up => "up",
            Self::Right => "right",
        }
    }
}

/// Behaviour overrides that note kinds (scripted in the original) can set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteKindFlags {
    /// Whether hitting/missing this note affects score, health and tallies.
    pub scoreable: bool,
    /// Low-priority notes are only targeted if no normal note is in range.
    pub low_priority: bool,
}

impl Default for NoteKindFlags {
    fn default() -> Self {
        Self {
            scoreable: true,
            low_priority: false,
        }
    }
}

/// Runtime state of a hold note's trail.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldState {
    pub full_sustain_length: f64,
    /// Remaining length in ms.
    pub sustain_length: f64,
    pub hit_note: bool,
    pub missed_note: bool,
    pub handled_miss: bool,
    /// False once the hold has finished or was cleaned up.
    pub alive: bool,
}

/// A note in play.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayNote {
    pub data: SongNoteData,
    pub direction: NoteDirection,
    pub flags: NoteKindFlags,
    pub has_been_hit: bool,
    pub has_missed: bool,
    pub handled_miss: bool,
    pub may_hit: bool,
    pub too_early: bool,
    pub hold: Option<HoldState>,
}

/// Result of [`PlayNote::process_window`] (port of `HitWindowRes`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowResult {
    pub botplay_hit: bool,
    pub cont: bool,
}

impl PlayNote {
    pub fn new(data: SongNoteData, flags: NoteKindFlags) -> Self {
        let direction =
            NoteDirection::from_index(data.direction()).expect("direction is always 0..4");
        let hold = data.is_hold().then_some(HoldState {
            full_sustain_length: data.length,
            sustain_length: data.length,
            hit_note: false,
            missed_note: false,
            handled_miss: false,
            alive: true,
        });
        Self {
            data,
            direction,
            flags,
            has_been_hit: false,
            has_missed: false,
            handled_miss: false,
            may_hit: false,
            too_early: false,
            hold,
        }
    }

    #[inline]
    pub fn strum_time(&self) -> f64 {
        self.data.time
    }

    /// True when nothing further can happen to this note (head and trail).
    pub fn is_resolved(&self) -> bool {
        (self.has_been_hit || self.handled_miss)
            && self
                .hold
                .as_ref()
                .is_none_or(|h| !h.alive || h.handled_miss)
    }

    /// Port of `GRhythmUtil.processWindow`.
    pub fn process_window(&mut self, song_position: f64, is_controlled: bool) -> WindowResult {
        let start = self.strum_time() - HIT_WINDOW_MS;
        let center = self.strum_time();
        let end = self.strum_time() + HIT_WINDOW_MS;

        if self.has_missed || self.has_been_hit {
            return WindowResult {
                botplay_hit: false,
                cont: false,
            };
        }
        if song_position > end {
            self.too_early = false;
            self.has_missed = true;
            self.may_hit = false;
            if let Some(h) = &mut self.hold {
                h.missed_note = true;
            }
            return WindowResult {
                botplay_hit: false,
                cont: true,
            };
        }
        if !is_controlled && song_position >= center {
            return WindowResult {
                botplay_hit: true,
                cont: true,
            };
        }
        if let Some(h) = &mut self.hold {
            h.missed_note = false;
        }
        if song_position >= start {
            self.too_early = false;
            self.has_missed = false;
            self.may_hit = true;
        } else {
            self.too_early = true;
            self.may_hit = false;
            self.has_missed = false;
        }
        WindowResult {
            botplay_hit: false,
            cont: true,
        }
    }
}
