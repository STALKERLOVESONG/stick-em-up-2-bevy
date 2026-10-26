use bevy::prelude::*;

/// Top-level application state: the `Lobby` scene (title, game set-up), `Loading` (the level's
/// serialized data is read), `InGame`.
#[derive(States, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AppState {
    #[default]
    Lobby,
    Loading,
    InGame,
}

/// `GameMode`: how the level is being played.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameMode {
    #[default]
    Campaign,
    Versus,
    TeamVersus,
}

/// What the lobby decided (`LevelLoader.Mode`, `Difficulty`, `PlayerCount`). The lobby does not
/// exist yet, so a single-player campaign game is assumed.
/// TODO(UNCERTAIN): `LobbyDifficulty`'s default level; 0 (the middle of -1..=1) is assumed.
#[derive(Resource, Debug, Clone)]
pub struct Session {
    pub mode: GameMode,
    /// -1, 0 or 1 (`LevelLoader.Difficulty`).
    pub difficulty: i32,
    pub player_count: usize,
    /// The local player's slot (`ControlAssignment`: the column the keyboard-and-mouse input
    /// sits in), which is its `Player.PlayerIndex`.
    pub player_index: usize,
    /// `LobbyTeams` as the lobby left them (Team Versus).
    pub teams: sticks_sim::lobby::LobbyTeams,
}

impl Default for Session {
    fn default() -> Self {
        Self { mode: GameMode::Campaign, difficulty: 0, player_count: 1, player_index: 0, teams: sticks_sim::lobby::LobbyTeams::default() }
    }
}
