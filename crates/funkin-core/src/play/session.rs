//! Port of the gameplay loop in `PlayState.hx`: input queue, note processing,
//! score/health/combo bookkeeping, hold notes, ghost misses and death.

use super::note::*;
use super::strumline::*;
use crate::conductor::{Conductor, ConductorHits};
use crate::constants::*;
use crate::scoring::{self, Judgement, ScoringSystem, Tallies};
use crate::song::{SongEventData, SongNoteData, SongTimeChange};

/// Which strumline something happened on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Player,
    Opponent,
}

/// Session options (the subset of `Preferences` / PlayState flags that affect logic).
#[derive(Debug, Clone)]
pub struct PlayConfig {
    pub scoring: ScoringSystem,
    /// Desktop builds of Funkin' ship with ghost tapping **off**
    /// (`FEATURE_GHOST_TAPPING` defaults to mobile only).
    pub ghost_tapping: bool,
    /// Practice mode: no death.
    pub practice: bool,
    /// Bot play: the player's notes are hit automatically and never scored.
    pub botplay: bool,
    /// Scroll speed, used only to compute on-screen distance for the ghost-tap timer.
    pub scroll_speed: f64,
    /// Visible playfield height in pixels (720 in the original).
    pub view_height: f64,
}

impl Default for PlayConfig {
    fn default() -> Self {
        Self {
            scoring: ScoringSystem::Pbot1,
            ghost_tapping: false,
            practice: false,
            botplay: false,
            scroll_speed: 1.0,
            view_height: 720.0,
        }
    }
}

/// Things the frontend should react to (animations, sounds, popups, UI).
#[derive(Debug, Clone, PartialEq)]
pub enum PlayEvent {
    NoteHit {
        side: Side,
        index: usize,
        direction: NoteDirection,
        /// `None` for opponent / botplay hits.
        judgement: Option<Judgement>,
        /// Signed timing error in ms (positive = late).
        timing_ms: i32,
        score: i32,
        combo: u32,
    },
    NoteMiss {
        index: usize,
        direction: NoteDirection,
    },
    /// A key was pressed with nothing to hit (penalized unless ghost tapping).
    GhostMiss {
        direction: NoteDirection,
        has_possible_notes: bool,
        penalized: bool,
    },
    HoldDropped {
        index: usize,
        direction: NoteDirection,
        remaining_ms: f64,
        penalized: bool,
    },
    HoldFinished {
        side: Side,
        index: usize,
        direction: NoteDirection,
    },
    /// A chart event (camera focus, play animation, ...) reached its time.
    ChartEvent(usize),
    Conductor(ConductorHits),
    /// Health hit zero outside practice mode.
    Died,
}

#[derive(Debug, Clone, Copy)]
enum Input {
    Press(NoteDirection, f64),
    Release(NoteDirection),
}

/// A single song attempt.
#[derive(Debug, Clone)]
pub struct PlaySession {
    pub config: PlayConfig,
    pub conductor: Conductor,
    pub player: Strumline,
    pub opponent: Strumline,
    pub events: Vec<SongEventData>,
    next_event: usize,
    pub health: f64,
    pub song_score: f64,
    pub tallies: Tallies,
    pub is_dead: bool,
    inputs: Vec<Input>,
    last_position: Option<f64>,
}

impl PlaySession {
    /// Build a session from chart data. `kind_flags` lets note kinds override
    /// behaviour (the original does this in scripts); pass `|_| Default::default()`.
    pub fn new(
        config: PlayConfig,
        time_changes: &[SongTimeChange],
        notes: &[SongNoteData],
        events: &[SongEventData],
        kind_flags: impl Fn(&SongNoteData) -> NoteKindFlags,
    ) -> Self {
        let mut conductor = Conductor::new();
        conductor.map_time_changes(time_changes);

        let (player, opponent): (Vec<_>, Vec<_>) = notes
            .iter()
            .filter(|n| (0..2).contains(&n.strumline_index()))
            .map(|n| (n.clone(), kind_flags(n)))
            .partition(|(n, _)| n.must_hit());

        let total_notes = player.iter().filter(|(_, f)| f.scoreable).count() as u32;
        let mut events = events.to_vec();
        events.sort_by(|a, b| a.time.total_cmp(&b.time));

        Self {
            config,
            conductor,
            player: Strumline::new(true, player),
            opponent: Strumline::new(false, opponent),
            events,
            next_event: 0,
            health: HEALTH_STARTING,
            song_score: 0.0,
            tallies: Tallies {
                total_notes,
                ..Default::default()
            },
            is_dead: false,
            inputs: Vec::new(),
            last_position: None,
        }
    }

    /// Integer score as displayed (`Std.int(songScore)`).
    pub fn score(&self) -> i64 {
        self.song_score as i64
    }

    /// Queue a key press that happened at `song_time_ms` (offsets already applied,
    /// i.e. in the same time base as [`Conductor::song_position`]).
    pub fn press(&mut self, dir: NoteDirection, song_time_ms: f64) {
        self.inputs.push(Input::Press(dir, song_time_ms));
    }

    pub fn release(&mut self, dir: NoteDirection) {
        self.inputs.push(Input::Release(dir));
    }

    /// Start mid-song without penalizing earlier notes.
    pub fn skip_to(&mut self, song_position: f64) {
        self.player.skip_to(song_position);
        self.opponent.skip_to(song_position);
        while self.next_event < self.events.len()
            && self.events[self.next_event].time < song_position
        {
            self.next_event += 1;
        }
    }

    /// Advance to `song_position` (ms, offsets applied). Returns events in order.
    pub fn update(&mut self, song_position: f64, out: &mut Vec<PlayEvent>) {
        let elapsed = self
            .last_position
            .map(|p| ((song_position - p) / MS_PER_SEC).max(0.0))
            .unwrap_or(0.0);
        self.last_position = Some(song_position);

        let hits = self.conductor.update(song_position, false);
        if hits.step || hits.beat || hits.measure {
            out.push(PlayEvent::Conductor(hits));
        }
        let pos = self.conductor.song_position;

        while self.next_event < self.events.len() && self.events[self.next_event].time <= pos {
            out.push(PlayEvent::ChartEvent(self.next_event));
            self.next_event += 1;
        }

        // Strumline.update → updateNotes (hold trails, early releases).
        let mut holds = Vec::new();
        self.player.update_holds(pos, !self.config.botplay, &mut holds);
        for h in holds.drain(..) {
            if let HoldUpdate::Finished(i) = h {
                let direction = self.player.notes[i].direction;
                out.push(PlayEvent::HoldFinished {
                    side: Side::Player,
                    index: i,
                    direction,
                });
            }
        }
        self.opponent.update_holds(pos, false, &mut holds);
        for h in holds.drain(..) {
            if let HoldUpdate::Finished(i) = h {
                let direction = self.opponent.notes[i].direction;
                out.push(PlayEvent::HoldFinished {
                    side: Side::Opponent,
                    index: i,
                    direction,
                });
            }
        }

        // Refresh hit windows so presses this frame see current state. Notes past
        // their window are left for `process_notes`, which owns miss handling.
        if !self.config.botplay {
            for i in self.player.active_range(pos) {
                let n = &mut self.player.notes[i];
                if pos <= n.strum_time() + HIT_WINDOW_MS {
                    n.process_window(pos, true);
                }
            }
        }

        self.process_input_queue(pos, out);
        self.process_notes(pos, elapsed, out);

        let render_distance =
            self.config.view_height / PIXELS_PER_MS / self.config.scroll_speed.max(0.01);
        self.player
            .update_ghost_tap_timer(elapsed, pos, render_distance);

        self.player.advance_cursor();
        self.opponent.advance_cursor();

        self.health = self.health.clamp(HEALTH_MIN, HEALTH_MAX);
        if self.health <= HEALTH_MIN && !self.config.practice && !self.is_dead {
            self.is_dead = true;
            out.push(PlayEvent::Died);
        }
    }

    /// Port of `processInputQueue`.
    fn process_input_queue(&mut self, pos: f64, out: &mut Vec<PlayEvent>) {
        if self.inputs.is_empty() {
            return;
        }
        let notes_in_range = self.player.notes_may_hit(pos);
        let mut by_dir: [Vec<usize>; KEY_COUNT] = Default::default();
        for &i in &notes_in_range {
            by_dir[self.player.notes[i].direction.index()].push(i);
        }

        // Presses are handled before releases, as in the original.
        let inputs = std::mem::take(&mut self.inputs);
        for input in &inputs {
            let Input::Press(dir, time) = *input else {
                continue;
            };
            self.player.press_key(dir);
            if self.config.botplay {
                continue;
            }
            let candidates = &mut by_dir[dir.index()];
            if candidates.is_empty() {
                let penalize = !self.config.ghost_tapping || !self.player.may_ghost_tap(pos);
                if penalize {
                    self.health += HEALTH_GHOST_MISS_PENALTY;
                    self.song_score += SCORE_GHOST_MISS_PENALTY;
                }
                out.push(PlayEvent::GhostMiss {
                    direction: dir,
                    has_possible_notes: !notes_in_range.is_empty(),
                    penalized: penalize,
                });
                continue;
            }
            let pick = candidates
                .iter()
                .position(|&i| !self.player.notes[i].flags.low_priority)
                .unwrap_or(0);
            let idx = candidates.remove(pick);
            self.good_note_hit(idx, time, pos, out);
        }
        for input in &inputs {
            if let Input::Release(dir) = *input {
                self.player.release_key(dir);
            }
        }
    }

    /// Port of `goodNoteHit`.
    fn good_note_hit(&mut self, idx: usize, input_time: f64, pos: f64, out: &mut Vec<PlayEvent>) {
        let note_time = self.player.notes[idx].strum_time();
        // `Std.int(songPosition - noteTime - inputLatencyMs)`: truncates toward zero.
        let diff = (input_time - note_time) as i32;
        let score = scoring::score_note(diff as f64, self.config.scoring);
        let judgement = scoring::judge_note(diff as f64, self.config.scoring);

        self.player.hit_note(idx, pos);
        let note = &self.player.notes[idx];
        let (direction, scoreable) = (note.direction, note.flags.scoreable);
        if scoreable {
            self.tallies.total_notes_hit += 1;
            self.apply_score(
                score as f64,
                Some(judgement),
                judgement.health_change(),
                judgement.is_combo_break(),
            );
        }
        out.push(PlayEvent::NoteHit {
            side: Side::Player,
            index: idx,
            direction,
            judgement: Some(judgement),
            timing_ms: diff,
            score,
            combo: self.tallies.combo,
        });
    }

    /// Port of `applyScore`.
    fn apply_score(
        &mut self,
        score: f64,
        judgement: Option<Judgement>,
        health_change: f64,
        combo_break: bool,
    ) {
        if let Some(j) = judgement {
            self.tallies.record(j);
        }
        self.health += health_change;
        if combo_break {
            self.tallies.combo = 0;
        } else {
            self.tallies.combo += 1;
            self.tallies.max_combo = self.tallies.max_combo.max(self.tallies.combo);
        }
        self.song_score += score;
    }

    /// Port of `processNotes`.
    fn process_notes(&mut self, pos: f64, elapsed: f64, out: &mut Vec<PlayEvent>) {
        // Opponent: always auto-hit at the strum time.
        for i in self.opponent.active_range(pos) {
            let r = self.opponent.notes[i].process_window(pos, false);
            if r.botplay_hit {
                self.opponent.hit_note(i, pos);
                let direction = self.opponent.notes[i].direction;
                out.push(PlayEvent::NoteHit {
                    side: Side::Opponent,
                    index: i,
                    direction,
                    judgement: None,
                    timing_ms: 0,
                    score: 0,
                    combo: 0,
                });
            }
            let n = &mut self.opponent.notes[i];
            if n.has_missed {
                n.handled_miss = true;
            }
            if let Some(h) = &mut n.hold {
                if h.missed_note && !h.handled_miss {
                    h.handled_miss = true;
                }
            }
        }

        // Player notes.
        let botplay = self.config.botplay;
        for i in self.player.active_range(pos) {
            let r = self.player.notes[i].process_window(pos, !botplay);
            if r.botplay_hit {
                self.player.hit_note(i, pos);
                let direction = self.player.notes[i].direction;
                out.push(PlayEvent::NoteHit {
                    side: Side::Player,
                    index: i,
                    direction,
                    judgement: None,
                    timing_ms: 0,
                    score: 0,
                    combo: self.tallies.combo,
                });
            }
            if !r.cont {
                continue;
            }
            let n = &mut self.player.notes[i];
            if n.has_missed && !n.handled_miss {
                n.handled_miss = true;
                let (direction, scoreable) = (n.direction, n.flags.scoreable);
                if !botplay && scoreable {
                    // onNoteMiss
                    let miss = scoring::miss_score(self.config.scoring) as f64;
                    self.apply_score(miss, Some(Judgement::Miss), HEALTH_MISS_PENALTY, true);
                }
                out.push(PlayEvent::NoteMiss {
                    index: i,
                    direction,
                });
            }
        }

        // Player hold notes.
        for i in self.player.active_range(pos) {
            let n = &mut self.player.notes[i];
            let (direction, scoreable) = (n.direction, n.flags.scoreable);
            let Some(h) = n.hold.as_mut() else { continue };
            if !h.alive {
                continue;
            }
            if h.hit_note && !h.missed_note && h.sustain_length > 0.0 && !botplay && scoreable {
                self.health += HEALTH_HOLD_BONUS_PER_SECOND * elapsed;
                self.song_score += SCORE_HOLD_BONUS_PER_SECOND * elapsed;
            }
            if h.missed_note && !h.handled_miss {
                h.handled_miss = true;
                let remaining = h.sustain_length;
                let hit = h.hit_note;
                let mut penalized = false;
                if !botplay && scoreable && remaining > HOLD_DROP_PENALTY_THRESHOLD_MS {
                    let remaining_sec = remaining / MS_PER_SEC;
                    let uncapped = remaining_sec * HEALTH_HOLD_DROP_PENALTY_PER_SECOND;
                    // If the head was missed, don't penalize again on top of that.
                    let max =
                        HEALTH_HOLD_DROP_PENALTY_MAX - if hit { -HEALTH_MISS_PENALTY } else { 0.0 };
                    let health_change = uncapped.clamp(max.min(0.0), 0.0);
                    let score_change = SCORE_HOLD_DROP_PENALTY_PER_SECOND * remaining_sec;
                    self.apply_score(score_change, None, health_change, true);
                    penalized = true;
                }
                out.push(PlayEvent::HoldDropped {
                    index: i,
                    direction,
                    remaining_ms: remaining,
                    penalized,
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(t: f64, d: i32, l: f64) -> SongNoteData {
        SongNoteData {
            time: t,
            data: d,
            length: l,
            kind: None,
            params: vec![],
        }
    }

    fn session(notes: &[SongNoteData]) -> PlaySession {
        PlaySession::new(
            PlayConfig::default(),
            &[SongTimeChange::new(0.0, 120.0)],
            notes,
            &[],
            |_| NoteKindFlags::default(),
        )
    }

    fn run(s: &mut PlaySession, until: f64) -> Vec<PlayEvent> {
        let mut out = Vec::new();
        let mut t = s.last_position.unwrap_or(0.0);
        while t <= until {
            s.update(t, &mut out);
            t += 1000.0 / 240.0;
        }
        out
    }

    #[test]
    fn botplay_hits_everything() {
        let mut s = session(&[note(500.0, 0, 0.0), note(1000.0, 2, 1000.0), note(2500.0, 3, 0.0)]);
        s.config.botplay = true;
        let ev = run(&mut s, 3000.0);
        let hits = ev
            .iter()
            .filter(|e| matches!(e, PlayEvent::NoteHit { side: Side::Player, .. }))
            .count();
        assert_eq!(hits, 3);
        assert!(ev.iter().any(|e| matches!(e, PlayEvent::HoldFinished { side: Side::Player, .. })));
        assert!(!ev.iter().any(|e| matches!(e, PlayEvent::NoteMiss { .. } | PlayEvent::HoldDropped { .. })));
        assert_eq!(s.tallies.missed, 0);
    }

    #[test]
    fn perfect_hit() {
        let mut s = session(&[note(1000.0, 0, 0.0), note(1000.0, 4, 0.0)]);
        run(&mut s, 999.0);
        s.press(NoteDirection::Left, 1000.0);
        let ev = run(&mut s, 1200.0);
        assert!(ev.iter().any(|e| matches!(
            e,
            PlayEvent::NoteHit {
                side: Side::Player,
                judgement: Some(Judgement::Sick),
                score: 500,
                ..
            }
        )));
        assert!(ev.iter().any(|e| matches!(
            e,
            PlayEvent::NoteHit {
                side: Side::Opponent,
                ..
            }
        )));
        assert_eq!(s.score(), 500);
        assert_eq!(s.tallies.sick, 1);
        assert_eq!(s.tallies.combo, 1);
        assert!((s.health - (HEALTH_STARTING + HEALTH_SICK_BONUS)).abs() < 1e-9);
    }

    #[test]
    fn miss_and_ghost() {
        let mut s = session(&[note(500.0, 1, 0.0)]);
        s.press(NoteDirection::Right, 0.0);
        let ev = run(&mut s, 800.0);
        assert!(ev.iter().any(|e| matches!(
            e,
            PlayEvent::GhostMiss {
                penalized: true,
                ..
            }
        )));
        assert!(ev.iter().any(|e| matches!(e, PlayEvent::NoteMiss { .. })));
        assert_eq!(s.tallies.missed, 1);
        assert_eq!(s.score(), -110);
        let expected = HEALTH_STARTING + HEALTH_GHOST_MISS_PENALTY + HEALTH_MISS_PENALTY;
        assert!((s.health - expected).abs() < 1e-9);
    }

    #[test]
    fn hold_note_full_and_dropped() {
        // Full hold: 1000ms long → +250 score per second held.
        let mut s = session(&[note(1000.0, 2, 1000.0)]);
        run(&mut s, 999.0);
        s.press(NoteDirection::Up, 1000.0);
        let ev = run(&mut s, 2100.0);
        assert!(ev.iter().any(|e| matches!(
            e,
            PlayEvent::HoldFinished {
                side: Side::Player,
                ..
            }
        )));
        assert!((s.score() - 750).abs() <= 2, "score was {}", s.score());

        // Dropped halfway: -125/s for the remaining ~500ms and a combo break.
        let mut s = session(&[note(1000.0, 2, 1000.0)]);
        run(&mut s, 999.0);
        s.press(NoteDirection::Up, 1000.0);
        run(&mut s, 1500.0);
        s.release(NoteDirection::Up);
        let ev = run(&mut s, 2100.0);
        assert!(ev.iter().any(|e| matches!(
            e,
            PlayEvent::HoldDropped {
                penalized: true,
                ..
            }
        )));
        assert_eq!(s.tallies.combo, 0);
        assert!(
            (s.score() - (500 + 125 - 62)).abs() <= 3,
            "score was {}",
            s.score()
        );
    }

    #[test]
    fn death() {
        let notes: Vec<_> = (0..20)
            .map(|i| note(200.0 + i as f64 * 50.0, 0, 0.0))
            .collect();
        let mut s = session(&notes);
        let ev = run(&mut s, 3000.0);
        assert!(s.is_dead);
        assert_eq!(
            ev.iter().filter(|e| matches!(e, PlayEvent::Died)).count(),
            1
        );
    }
}
