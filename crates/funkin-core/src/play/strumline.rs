//! Port of the logic in `funkin/play/notes/Strumline.hx` (no rendering).

use std::ops::Range;

use super::note::*;
use crate::constants::*;
use crate::song::SongNoteData;

/// One side's notes (player or opponent).
#[derive(Debug, Clone)]
pub struct Strumline {
    pub is_player: bool,
    /// All notes for this side, sorted by strum time.
    pub notes: Vec<PlayNote>,
    /// Index of the first note that may still change state.
    cursor: usize,
    keys_held: [bool; KEY_COUNT],
    ghost_tap_timer: f64,
}

/// Something that happened to a hold note during [`Strumline::update_holds`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldUpdate {
    /// The player let go before the trail ended.
    Released(usize),
    /// The trail finished while held.
    Finished(usize),
}

impl Strumline {
    pub fn new(
        is_player: bool,
        notes: impl IntoIterator<Item = (SongNoteData, NoteKindFlags)>,
    ) -> Self {
        let mut notes: Vec<PlayNote> = notes
            .into_iter()
            .map(|(d, f)| PlayNote::new(d, f))
            .collect();
        notes.sort_by(|a, b| a.strum_time().total_cmp(&b.strum_time()));
        Self {
            is_player,
            notes,
            cursor: 0,
            keys_held: [false; KEY_COUNT],
            ghost_tap_timer: 0.0,
        }
    }

    /// Indices of notes that can still change state and have entered the
    /// lookahead (`song_position + HIT_WINDOW_MS`).
    pub fn active_range(&self, song_position: f64) -> Range<usize> {
        let limit = song_position + HIT_WINDOW_MS;
        let end = self.cursor
            + self.notes[self.cursor..]
                .iter()
                .take_while(|n| n.strum_time() <= limit)
                .count();
        self.cursor..end
    }

    /// Index of the first note that may still change state.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Skip past notes that are fully resolved. O(1) amortized per frame.
    pub fn advance_cursor(&mut self) {
        while self.cursor < self.notes.len() && self.notes[self.cursor].is_resolved() {
            self.cursor += 1;
        }
    }

    pub fn is_key_held(&self, dir: NoteDirection) -> bool {
        self.keys_held[dir.index()]
    }
    pub fn press_key(&mut self, dir: NoteDirection) {
        self.keys_held[dir.index()] = true;
    }
    pub fn release_key(&mut self, dir: NoteDirection) {
        self.keys_held[dir.index()] = false;
    }

    /// Port of `Strumline.hitNote`.
    pub fn hit_note(&mut self, idx: usize, song_position: f64) {
        let note = &mut self.notes[idx];
        note.has_been_hit = true;
        note.may_hit = false;
        let strum = note.strum_time();
        if let Some(h) = &mut note.hold {
            h.hit_note = true;
            h.missed_note = false;
            h.sustain_length = h
                .full_sustain_length
                .min(strum + h.full_sustain_length - song_position);
        }
        self.ghost_tap_timer = GHOST_TAP_DELAY;
    }

    /// Port of `getNotesMayHit`, restricted to the active range.
    pub fn notes_may_hit(&self, song_position: f64) -> Vec<usize> {
        self.active_range(song_position)
            .filter(|&i| {
                let n = &self.notes[i];
                !n.has_been_hit && !n.handled_miss && n.may_hit
            })
            .collect()
    }

    /// Port of `mayGhostTap` (only relevant when ghost tapping is enabled).
    pub fn may_ghost_tap(&self, song_position: f64) -> bool {
        if !self.notes_may_hit(song_position).is_empty() {
            return false;
        }
        let holds_busy = self.active_range(song_position).any(|i| {
            self.notes[i]
                .hold
                .as_ref()
                .is_some_and(|h| h.alive && (h.hit_note || h.missed_note))
        });
        !holds_busy && self.ghost_tap_timer <= 0.0
    }

    /// Port of `updateGhostTapTimer`. The timer only runs while no notes are on screen.
    pub fn update_ghost_tap_timer(
        &mut self,
        elapsed: f64,
        song_position: f64,
        render_distance_ms: f64,
    ) {
        let on_screen = self.notes[self.cursor..]
            .iter()
            .take_while(|n| n.strum_time() <= song_position + render_distance_ms)
            .any(|n| !n.is_resolved());
        if on_screen {
            return;
        }
        self.ghost_tap_timer = (self.ghost_tap_timer - elapsed).max(0.0);
    }

    /// Port of the hold-note section of `Strumline.updateNotes`.
    /// `controlled` is false for botplay: holds then play out without a key held.
    pub fn update_holds(&mut self, song_position: f64, controlled: bool, out: &mut Vec<HoldUpdate>) {
        for i in self.active_range(song_position) {
            let strum = self.notes[i].strum_time();
            let dir = self.notes[i].direction;
            let held = self.is_key_held(dir);
            let is_player = self.is_player;
            let Some(h) = self.notes[i].hold.as_mut() else {
                continue;
            };
            if !h.alive {
                continue;
            }
            if song_position > strum && h.hit_note && !h.missed_note {
                if is_player && controlled && !held {
                    h.missed_note = true;
                    out.push(HoldUpdate::Released(i));
                    continue;
                }
                h.sustain_length = (strum + h.full_sustain_length) - song_position;
                if h.sustain_length <= 0.0 {
                    h.sustain_length = 0.0;
                    h.alive = false;
                    out.push(HoldUpdate::Finished(i));
                }
            }
        }
    }

    /// Port of `handleSkippedNotes`: mark everything before `song_position`
    /// as resolved without penalties (used when seeking / restarting mid-song).
    pub fn skip_to(&mut self, song_position: f64) {
        for n in &mut self.notes {
            if !n.has_been_hit && n.strum_time() + HIT_WINDOW_MS < song_position {
                n.handled_miss = true;
                n.has_missed = true;
                if let Some(h) = &mut n.hold {
                    h.alive = false;
                    h.handled_miss = true;
                }
            }
        }
        self.ghost_tap_timer = 0.0;
        self.advance_cursor();
    }
}
