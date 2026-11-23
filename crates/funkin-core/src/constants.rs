//! Gameplay constants, ported 1:1 from `funkin/util/Constants.hx`.
//!
//! Only values that affect game logic are ported; purely visual constants
//! (colours, tween durations, ...) belong to the frontend.

// ---------------------------------------------------------------------------
// Defaults
// ---------------------------------------------------------------------------

pub const DEFAULT_DIFFICULTY: &str = "normal";
pub const DEFAULT_DIFFICULTY_LIST: [&str; 3] = ["easy", "normal", "hard"];
pub const DEFAULT_DIFFICULTY_LIST_ERECT: [&str; 2] = ["erect", "nightmare"];
pub const DEFAULT_CHARACTER: &str = "bf";
pub const DEFAULT_STAGE: &str = "mainStage";
pub const DEFAULT_SONG: &str = "tutorial";
pub const DEFAULT_VARIATION: &str = "default";
pub const DEFAULT_NOTE_STYLE: &str = "funkin";
pub const DEFAULT_BPM: f64 = 100.0;
pub const DEFAULT_SONGNAME: &str = "Unknown";
pub const DEFAULT_SCROLLSPEED: f64 = 1.0;
pub const DEFAULT_TIME_SIGNATURE_NUM: u32 = 4;
pub const DEFAULT_TIME_SIGNATURE_DEN: u32 = 4;
pub const DEFAULT_PREVIEW_START_TIME: f64 = 0.0;
pub const DEFAULT_PREVIEW_END_TIME: f64 = 0.2;
pub const DEFAULT_DIVISIONS: i32 = 96;

// ---------------------------------------------------------------------------
// Timing
// ---------------------------------------------------------------------------

/// Notes can be hit this many ms before or after their strum time.
pub const HIT_WINDOW_MS: f64 = 160.0;
pub const SECS_PER_MIN: f64 = 60.0;
pub const MS_PER_SEC: f64 = 1000.0;
pub const NS_PER_MS: f64 = 1_000_000.0;
pub const STEPS_PER_BEAT: f64 = 4.0;
/// All MP3 decoders introduce a playback delay of 528 samples (~12ms at 44.1kHz).
pub const MP3_DELAY_MS: f64 = 528.0 / 44100.0 * MS_PER_SEC;
/// Scroll distance per millisecond at scroll speed 1.0.
pub const PIXELS_PER_MS: f64 = 0.45;
/// After hitting a note, ghost tapping is disallowed for this many seconds.
pub const GHOST_TAP_DELAY: f64 = 3.0 / 8.0;

// ---------------------------------------------------------------------------
// Health
// ---------------------------------------------------------------------------

pub const HEALTH_MAX: f64 = 2.0;
pub const HEALTH_STARTING: f64 = HEALTH_MAX / 2.0;
pub const HEALTH_MIN: f64 = 0.0;
pub const HEALTH_KILLER_BONUS: f64 = 2.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_SICK_BONUS: f64 = 1.5 / 100.0 * HEALTH_MAX;
pub const HEALTH_GOOD_BONUS: f64 = 0.75 / 100.0 * HEALTH_MAX;
pub const HEALTH_BAD_BONUS: f64 = 0.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_SHIT_BONUS: f64 = -1.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_HOLD_BONUS_PER_SECOND: f64 = 6.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_MISS_PENALTY: f64 = -4.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_GHOST_MISS_PENALTY: f64 = -4.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_HOLD_DROP_PENALTY_PER_SECOND: f64 = 0.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_HOLD_DROP_PENALTY_MAX: f64 = 0.0 / 100.0 * HEALTH_MAX;
pub const HEALTH_MINE_PENALTY: f64 = -15.0 / 100.0 * HEALTH_MAX;

// ---------------------------------------------------------------------------
// Score
// ---------------------------------------------------------------------------

pub const SCORE_GHOST_MISS_PENALTY: f64 = -10.0;
pub const SCORE_HOLD_BONUS_PER_SECOND: f64 = 250.0;
pub const SCORE_HOLD_DROP_PENALTY_PER_SECOND: f64 = -125.0;
/// Hold notes dropped with less than this much remaining are not penalized.
pub const HOLD_DROP_PENALTY_THRESHOLD_MS: f64 = 160.0;

pub const JUDGEMENT_KILLER_COMBO_BREAK: bool = false;
pub const JUDGEMENT_SICK_COMBO_BREAK: bool = false;
pub const JUDGEMENT_GOOD_COMBO_BREAK: bool = false;
pub const JUDGEMENT_BAD_COMBO_BREAK: bool = true;
pub const JUDGEMENT_SHIT_COMBO_BREAK: bool = true;

// ---------------------------------------------------------------------------
// Ranks
// ---------------------------------------------------------------------------

pub const RANK_PERFECT_THRESHOLD: f64 = 1.00;
pub const RANK_EXCELLENT_THRESHOLD: f64 = 0.90;
pub const RANK_GREAT_THRESHOLD: f64 = 0.80;
pub const RANK_GOOD_THRESHOLD: f64 = 0.60;

// ---------------------------------------------------------------------------
// Strumlines
// ---------------------------------------------------------------------------

/// Number of lanes per strumline.
pub const KEY_COUNT: usize = 4;
