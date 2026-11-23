//! # funkin-core
//!
//! A Rust port of the gameplay logic from the open-source Funkin' codebase
//! (<https://github.com/FunkinCrew/Funkin>, Apache-2.0).
//!
//! This crate is deliberately **engine-independent**: it has no rendering, audio
//! or windowing dependencies. Everything here is pure, deterministic logic that a
//! frontend drives by feeding it a song position (from the audio clock) and
//! timestamped input events.
//!
//! | Rust module          | Ported from (Haxe)                                   |
//! |----------------------|------------------------------------------------------|
//! | [`constants`]        | `funkin/util/Constants.hx`                           |
//! | [`conductor`]        | `funkin/Conductor.hx`                                |
//! | [`scoring`]          | `funkin/play/scoring/Scoring.hx`, `Highscore.hx`     |
//! | [`song`]             | `funkin/data/song/SongData.hx`, `SongRegistry.hx`, `play/song/Song.hx` |
//! | [`play`]             | `funkin/play/PlayState.hx`, `play/notes/Strumline.hx`, `util/GRhythmUtil.hx` |
//! | [`state`]            | The FlxState flow (Title → Menus → PlayState → Results) |
//!
//! No game assets are included in or referenced by this crate; data is loaded
//! at runtime from a user-supplied directory.

pub mod camera;
pub mod conductor;
pub mod constants;
pub mod play;
pub mod scoring;
pub mod song;
pub mod stage;
pub mod state;
pub mod vfs;

/// Port of `FlxMath.roundDecimal`: rounds `value` to `precision` decimal places,
/// using Haxe's `Math.round` semantics (round half towards positive infinity).
#[inline]
pub fn round_decimal(value: f64, precision: i32) -> f64 {
    let mult = 10f64.powi(precision);
    (value * mult + 0.5).floor() / mult
}
