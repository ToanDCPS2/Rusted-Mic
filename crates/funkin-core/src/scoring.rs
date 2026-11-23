//! Port of `funkin/play/scoring/Scoring.hx` and the tally portion of `Highscore.hx`.

use serde::{Deserialize, Serialize};

use crate::constants::*;

/// Which system to use when scoring and judging notes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum ScoringSystem {
    /// Week 6 and older. Step function.
    Legacy,
    /// Week 7. Tighter windows than Legacy. Step function.
    Week7,
    /// "Points Based On Timing" v1 — sigmoid. Used by current versions.
    #[default]
    Pbot1,
}

/// A note judgement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Judgement {
    Sick,
    Good,
    Bad,
    Shit,
    Miss,
}

impl Judgement {
    /// Health gained for this judgement (from `PlayState.goodNoteHit`).
    pub fn health_change(self) -> f64 {
        match self {
            Judgement::Sick => HEALTH_SICK_BONUS,
            Judgement::Good => HEALTH_GOOD_BONUS,
            Judgement::Bad => HEALTH_BAD_BONUS,
            Judgement::Shit => HEALTH_SHIT_BONUS,
            Judgement::Miss => HEALTH_MISS_PENALTY,
        }
    }

    /// Whether this judgement resets the combo.
    pub fn is_combo_break(self) -> bool {
        match self {
            Judgement::Sick => JUDGEMENT_SICK_COMBO_BREAK,
            Judgement::Good => JUDGEMENT_GOOD_COMBO_BREAK,
            Judgement::Bad => JUDGEMENT_BAD_COMBO_BREAK,
            Judgement::Shit => JUDGEMENT_SHIT_COMBO_BREAK,
            Judgement::Miss => true,
        }
    }

    /// Asset name used for the rating popup sprite (`sick`, `good`, ...).
    pub fn as_str(self) -> &'static str {
        match self {
            Judgement::Sick => "sick",
            Judgement::Good => "good",
            Judgement::Bad => "bad",
            Judgement::Shit => "shit",
            Judgement::Miss => "miss",
        }
    }
}

// --- PBOT1 ------------------------------------------------------------------

pub const PBOT1_MAX_SCORE: i32 = 500;
pub const PBOT1_SCORING_OFFSET: f64 = 54.99;
pub const PBOT1_SCORING_SLOPE: f64 = 0.080;
pub const PBOT1_MIN_SCORE: f64 = 9.0;
pub const PBOT1_MISS_SCORE: i32 = -100;
pub const PBOT1_PERFECT_THRESHOLD: f64 = 5.0;
pub const PBOT1_MISS_THRESHOLD: f64 = 160.0;
pub const PBOT1_KILLER_THRESHOLD: f64 = 12.5;
pub const PBOT1_SICK_THRESHOLD: f64 = 45.0;
pub const PBOT1_GOOD_THRESHOLD: f64 = 90.0;
pub const PBOT1_BAD_THRESHOLD: f64 = 135.0;
pub const PBOT1_SHIT_THRESHOLD: f64 = 160.0;

// --- LEGACY / WEEK7 -----------------------------------------------------------

/// 10 frames at 60fps (~166.67ms).
pub const LEGACY_HIT_WINDOW: f64 = (10.0 / 60.0) * 1000.0;
pub const LEGACY_SICK_THRESHOLD: f64 = 0.2;
pub const LEGACY_GOOD_THRESHOLD: f64 = 0.75;
pub const LEGACY_BAD_THRESHOLD: f64 = 0.9;
pub const LEGACY_SHIT_THRESHOLD: f64 = 1.0;
pub const LEGACY_SICK_SCORE: i32 = 350;
pub const LEGACY_GOOD_SCORE: i32 = 200;
pub const LEGACY_BAD_SCORE: i32 = 100;
pub const LEGACY_SHIT_SCORE: i32 = 50;
pub const LEGACY_MISS_SCORE: i32 = -10;

pub const WEEK7_HIT_WINDOW: f64 = LEGACY_HIT_WINDOW;
pub const WEEK7_BAD_THRESHOLD: f64 = 0.8;
pub const WEEK7_GOOD_THRESHOLD: f64 = 0.55;
pub const WEEK7_SICK_THRESHOLD: f64 = 0.2;
pub const WEEK7_MISS_SCORE: i32 = -10;
pub const WEEK7_SHIT_SCORE: i32 = 50;
pub const WEEK7_BAD_SCORE: i32 = 100;
pub const WEEK7_GOOD_SCORE: i32 = 200;
pub const WEEK7_SICK_SCORE: i32 = 350;

/// Score for a hit `ms_timing` ms away from the note (sign ignored).
pub fn score_note(ms_timing: f64, system: ScoringSystem) -> i32 {
    let t = ms_timing.abs();
    match system {
        ScoringSystem::Pbot1 => {
            if t > PBOT1_MISS_THRESHOLD {
                PBOT1_MISS_SCORE
            } else if t < PBOT1_PERFECT_THRESHOLD {
                PBOT1_MAX_SCORE
            } else {
                let factor =
                    1.0 - 1.0 / (1.0 + (-PBOT1_SCORING_SLOPE * (t - PBOT1_SCORING_OFFSET)).exp());
                // `Std.int` truncates toward zero.
                (PBOT1_MAX_SCORE as f64 * factor + PBOT1_MIN_SCORE) as i32
            }
        }
        ScoringSystem::Legacy => {
            let w = LEGACY_HIT_WINDOW;
            if t < w * LEGACY_SICK_THRESHOLD {
                LEGACY_SICK_SCORE
            } else if t < w * LEGACY_GOOD_THRESHOLD {
                LEGACY_GOOD_SCORE
            } else if t < w * LEGACY_BAD_THRESHOLD {
                LEGACY_BAD_SCORE
            } else if t < w * LEGACY_SHIT_THRESHOLD {
                LEGACY_SHIT_SCORE
            } else {
                0
            }
        }
        ScoringSystem::Week7 => {
            let w = WEEK7_HIT_WINDOW;
            if t < w * WEEK7_SICK_THRESHOLD {
                WEEK7_SICK_SCORE
            } else if t < w * WEEK7_GOOD_THRESHOLD {
                WEEK7_GOOD_SCORE
            } else if t < w * WEEK7_BAD_THRESHOLD {
                WEEK7_BAD_SCORE
            } else if t < w {
                WEEK7_SHIT_SCORE
            } else {
                0
            }
        }
    }
}

/// Judgement for a hit `ms_timing` ms away from the note.
pub fn judge_note(ms_timing: f64, system: ScoringSystem) -> Judgement {
    let t = ms_timing.abs();
    let (sick, good, bad, shit) = match system {
        ScoringSystem::Pbot1 => (
            PBOT1_SICK_THRESHOLD,
            PBOT1_GOOD_THRESHOLD,
            PBOT1_BAD_THRESHOLD,
            PBOT1_SHIT_THRESHOLD,
        ),
        ScoringSystem::Legacy => (
            LEGACY_HIT_WINDOW * LEGACY_SICK_THRESHOLD,
            LEGACY_HIT_WINDOW * LEGACY_GOOD_THRESHOLD,
            LEGACY_HIT_WINDOW * LEGACY_BAD_THRESHOLD,
            LEGACY_HIT_WINDOW * LEGACY_SHIT_THRESHOLD,
        ),
        ScoringSystem::Week7 => (
            WEEK7_HIT_WINDOW * WEEK7_SICK_THRESHOLD,
            WEEK7_HIT_WINDOW * WEEK7_GOOD_THRESHOLD,
            WEEK7_HIT_WINDOW * WEEK7_BAD_THRESHOLD,
            WEEK7_HIT_WINDOW,
        ),
    };
    if t <= sick {
        Judgement::Sick
    } else if t <= good {
        Judgement::Good
    } else if t <= bad {
        Judgement::Bad
    } else if t <= shit {
        Judgement::Shit
    } else {
        Judgement::Miss
    }
}

pub fn miss_score(system: ScoringSystem) -> i32 {
    match system {
        ScoringSystem::Pbot1 => PBOT1_MISS_SCORE,
        ScoringSystem::Legacy => LEGACY_MISS_SCORE,
        ScoringSystem::Week7 => WEEK7_MISS_SCORE,
    }
}

/// Port of `SaveScoreTallyData` / `Highscore.tallies`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tallies {
    pub sick: u32,
    pub good: u32,
    pub bad: u32,
    pub shit: u32,
    pub missed: u32,
    pub combo: u32,
    pub max_combo: u32,
    pub total_notes_hit: u32,
    /// Number of scoreable player notes in the chart.
    pub total_notes: u32,
}

impl Tallies {
    pub fn record(&mut self, j: Judgement) {
        match j {
            Judgement::Sick => self.sick += 1,
            Judgement::Good => self.good += 1,
            Judgement::Bad => self.bad += 1,
            Judgement::Shit => self.shit += 1,
            Judgement::Miss => self.missed += 1,
        }
    }
}

/// `(sick + good - missed) / totalNotes`, clamped to `[0, 1]`.
/// This is the "clear %" shown on the results screen.
pub fn tally_completion(t: &Tallies) -> f64 {
    if t.total_notes == 0 {
        return 0.0;
    }
    ((t.sick as f64 + t.good as f64 - t.missed as f64) / t.total_notes as f64).clamp(0.0, 1.0)
}

/// Port of `ScoringRank`. Ordered worst → best so `Ord` comparisons match the original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ScoringRank {
    Shit,
    Good,
    Great,
    Excellent,
    Perfect,
    PerfectGold,
}

impl ScoringRank {
    /// Delay before the results music starts, in seconds.
    pub fn music_delay(self) -> f64 {
        match self {
            ScoringRank::PerfectGold | ScoringRank::Perfect => 95.0 / 24.0,
            ScoringRank::Excellent => 0.0,
            ScoringRank::Great => 5.0 / 24.0,
            ScoringRank::Good => 3.0 / 24.0,
            ScoringRank::Shit => 2.0 / 24.0,
        }
    }
}

/// Port of `Scoring.calculateRank`.
pub fn calculate_rank(t: &Tallies) -> Option<ScoringRank> {
    if t.total_notes == 0 {
        return None;
    }
    if t.sick == t.total_notes {
        return Some(ScoringRank::PerfectGold);
    }
    let c = tally_completion(t);
    Some(if c == RANK_PERFECT_THRESHOLD {
        ScoringRank::Perfect
    } else if c >= RANK_EXCELLENT_THRESHOLD {
        ScoringRank::Excellent
    } else if c >= RANK_GREAT_THRESHOLD {
        ScoringRank::Great
    } else if c >= RANK_GOOD_THRESHOLD {
        ScoringRank::Good
    } else {
        ScoringRank::Shit
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pbot1_curve() {
        assert_eq!(score_note(0.0, ScoringSystem::Pbot1), 500);
        assert_eq!(score_note(-4.9, ScoringSystem::Pbot1), 500);
        assert_eq!(score_note(161.0, ScoringSystem::Pbot1), -100);
        // Monotonically decreasing between thresholds.
        let mut prev = i32::MAX;
        for ms in 5..=160 {
            let s = score_note(ms as f64, ScoringSystem::Pbot1);
            assert!(s <= prev, "score increased at {ms}ms");
            prev = s;
        }
        // At the sigmoid midpoint the factor is 0.5.
        assert_eq!(score_note(54.99, ScoringSystem::Pbot1), 259);
    }

    #[test]
    fn pbot1_judgements() {
        assert_eq!(judge_note(45.0, ScoringSystem::Pbot1), Judgement::Sick);
        assert_eq!(judge_note(-45.1, ScoringSystem::Pbot1), Judgement::Good);
        assert_eq!(judge_note(90.0, ScoringSystem::Pbot1), Judgement::Good);
        assert_eq!(judge_note(135.0, ScoringSystem::Pbot1), Judgement::Bad);
        assert_eq!(judge_note(160.0, ScoringSystem::Pbot1), Judgement::Shit);
        assert_eq!(judge_note(160.1, ScoringSystem::Pbot1), Judgement::Miss);
    }

    #[test]
    fn ranks() {
        let mut t = Tallies {
            total_notes: 10,
            sick: 10,
            ..Default::default()
        };
        assert_eq!(calculate_rank(&t), Some(ScoringRank::PerfectGold));
        t.sick = 5;
        t.good = 5;
        assert_eq!(calculate_rank(&t), Some(ScoringRank::Perfect));
        t.good = 4;
        t.missed = 1;
        assert_eq!(calculate_rank(&t), Some(ScoringRank::Great));
        assert!(ScoringRank::PerfectGold > ScoringRank::Perfect);
        assert_eq!(calculate_rank(&Tallies::default()), None);
    }
}
