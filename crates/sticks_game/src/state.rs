use bevy::prelude::*;

use crate::input::InputDevice;

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

/// A player slot with controls (`ControlAssignment.ControlTypesForPlayer(i)` not empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPlayer {
    /// `Player.PlayerIndex`: the control-assignment column.
    pub index: usize,
    /// The column's available inputs, in `LobbyInput.All` order (the processors
    /// `PlayerSpawner.Spawn` hands the player's role inputs).
    pub devices: Vec<InputDevice>,
    /// Played on another machine of a network game (`PlayerSlotAssignments.SlotAssignedToAnotherClient`):
    /// no local inputs; the avatar follows what its owner sends.
    pub remote: bool,
}

/// What the lobby decided (`LevelLoader.Mode`, `Difficulty`, `PlayerCount`) and who plays with
/// what. Started from the command line: one keyboard-and-mouse campaign player.
/// TODO(UNCERTAIN): `LobbyDifficulty`'s default level; 0 (the middle of -1..=1) is assumed.
#[derive(Resource, Debug, Clone)]
pub struct Session {
    pub mode: GameMode,
    /// -1, 0 or 1 (`LevelLoader.Difficulty`).
    pub difficulty: i32,
    pub player_count: usize,
    /// The players with controls, by index.
    pub players: Vec<SessionPlayer>,
    /// `LobbyTeams` as the lobby left them (Team Versus).
    pub teams: sticks_sim::lobby::LobbyTeams,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            mode: GameMode::Campaign,
            difficulty: 0,
            player_count: 1,
            players: vec![SessionPlayer { index: 0, devices: vec![InputDevice::KeyboardMouse], remote: false }],
            teams: sticks_sim::lobby::LobbyTeams::default(),
        }
    }
}

impl Session {
    /// `ControlAssignment.ControlTypesForPlayer(i).Count > 0`, or (network games) the slot is
    /// played on another machine.
    pub fn has_controls(&self, index: usize) -> bool {
        self.players.iter().any(|p| p.index == index && (p.remote || !p.devices.is_empty()))
    }

    /// The slot is played on another machine.
    pub fn is_remote(&self, index: usize) -> bool {
        self.players.iter().any(|p| p.index == index && p.remote)
    }

    /// The player's devices (none for a slot without controls).
    pub fn devices(&self, index: usize) -> &[InputDevice] {
        self.players.iter().find(|p| p.index == index).map_or(&[], |p| p.devices.as_slice())
    }

    /// `AllClientControlTypes`: the control types of every player, sorted and comma-joined.
    pub fn control_types_name(&self) -> String {
        sticks_sim::devices::control_types_name(self.players.iter().flat_map(|p| p.devices.iter().map(|d| d.kind())))
    }

    /// The control-type names in use (`LevelTips.StartLevel`; a Steam controller counts as a
    /// gamepad).
    pub fn control_types_in_use(&self) -> Vec<&'static str> {
        let mut out: Vec<&'static str> = self.players.iter().flat_map(|p| p.devices.iter().map(|d| d.kind().control_type())).collect();
        out.sort_unstable();
        out.dedup();
        out
    }
}
