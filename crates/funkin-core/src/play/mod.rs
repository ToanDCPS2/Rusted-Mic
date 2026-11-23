//! Gameplay simulation: a headless, deterministic port of the note-processing
//! parts of `PlayState.hx`, `Strumline.hx` and `GRhythmUtil.hx`.
//!
//! The frontend owns time. Each frame it:
//! 1. forwards key presses/releases with the **song time at which they happened**
//!    ([`PlaySession::press`] / [`PlaySession::release`]),
//! 2. calls [`PlaySession::update`] with the current song position,
//! 3. drains the returned [`PlayEvent`]s to drive animations, sounds and UI.
//!
//! ## Deliberate differences from the original
//! * Input timing: the original subtracts measured input latency from the frame's
//!   song position. Here inputs carry their own song timestamp, which is
//!   equivalent but exact.
//! * Hit-window state (`mayHit`) is evaluated at the current position *before*
//!   inputs are processed. The original uses the previous frame's state, so it
//!   could ignore a press that landed in the first frame of a window.
//!   Results are otherwise identical, and they no longer depend on frame rate.

mod note;
mod session;
mod strumline;

pub use note::*;
pub use session::*;
pub use strumline::*;
