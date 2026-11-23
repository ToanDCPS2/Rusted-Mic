//! High-level game flow, mirroring the FlxState transitions in Funkin':
//!
//! ```text
//! Setup (assets missing) ──► Title ──► MainMenu ─┬─► StoryMenu ─┐
//!                                                └─► Freeplay ──┴─► Play ──► Results
//!                                                                  │  ▲
//!                                                                  ▼  │ retry
//!                                                               GameOver
//! ```
//!
//! This is a pure state machine; the frontend renders whichever state is current.

use crate::scoring::{ScoringRank, Tallies};

/// What to play next.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayRequest {
    pub song_id: String,
    pub difficulty: String,
    pub variation: String,
    /// Remaining songs in a story-mode week (port of `PlayStatePlaylist`).
    pub playlist: Vec<String>,
    pub is_story: bool,
    pub level_id: Option<String>,
}

/// Results of one song (or a whole week in story mode).
#[derive(Debug, Clone, PartialEq)]
pub struct SongResult {
    pub request: PlayRequest,
    pub score: i64,
    pub tallies: Tallies,
    pub rank: Option<ScoringRank>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum GameState {
    /// First boot / assets missing. Gameplay is unreachable from here until
    /// a valid installation has been imported.
    Setup {
        reason: String,
    },
    Title,
    MainMenu,
    StoryMenu,
    Freeplay,
    Play(PlayRequest),
    GameOver(PlayRequest),
    Results(SongResult),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Transition {
    AssetsReady,
    Confirm,
    OpenStory,
    OpenFreeplay,
    Back,
    StartSong(PlayRequest),
    Died,
    Retry,
    SongFinished(SongResult),
}

impl GameState {
    /// Apply a transition. Invalid transitions leave the state unchanged and
    /// return `false`.
    pub fn apply(&mut self, t: Transition) -> bool {
        use GameState as S;
        use Transition as T;
        let next = match (&*self, t) {
            (S::Setup { .. }, T::AssetsReady) => S::Title,
            (S::Title, T::Confirm) => S::MainMenu,
            (S::MainMenu, T::OpenStory) => S::StoryMenu,
            (S::MainMenu, T::OpenFreeplay) => S::Freeplay,
            (S::MainMenu, T::Back) => S::Title,
            (S::StoryMenu | S::Freeplay, T::Back) => S::MainMenu,
            (S::StoryMenu | S::Freeplay, T::StartSong(r)) => S::Play(r),
            (S::Play(r), T::Died) => S::GameOver(r.clone()),
            (S::GameOver(r), T::Retry) => S::Play(r.clone()),
            (S::GameOver(r) | S::Play(r), T::Back) => {
                if r.is_story {
                    S::StoryMenu
                } else {
                    S::Freeplay
                }
            }
            (S::Play(_), T::SongFinished(res)) => {
                // Story mode continues through the playlist before showing results.
                let req = &res.request;
                if req.is_story && !req.playlist.is_empty() {
                    let mut next = req.clone();
                    next.song_id = next.playlist.remove(0);
                    S::Play(next)
                } else {
                    S::Results(res)
                }
            }
            (S::Results(r), T::Confirm | T::Back) => {
                if r.request.is_story {
                    S::StoryMenu
                } else {
                    S::Freeplay
                }
            }
            _ => return false,
        };
        *self = next;
        true
    }

    /// Whether this state requires imported assets.
    pub fn requires_assets(&self) -> bool {
        !matches!(self, GameState::Setup { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_gates_everything() {
        let mut s = GameState::Setup {
            reason: "no assets".into(),
        };
        assert!(!s.apply(Transition::Confirm));
        assert!(!s.apply(Transition::OpenFreeplay));
        assert!(s.apply(Transition::AssetsReady));
        assert_eq!(s, GameState::Title);
    }
}
