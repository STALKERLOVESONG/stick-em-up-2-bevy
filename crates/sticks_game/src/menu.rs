//! `Time.timeScale`, what the in-game screens ask for (restart, back to the Lobby, the next
//! level) and the clean-up when a game ends. The screens themselves are the Permanent System's
//! `In-Game UI` (`ingame_ui`); the game's menus are the Lobby scene (`lobby`).

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;

use crate::camera::LevelCamera;
use crate::hud::Hud;
use crate::level::LevelGeometry;
use crate::lobby::LobbyMemory;
use crate::player::{Avatar, Player};
use crate::state::AppState;
use crate::weapon::Projectile;

/// `Time.timeScale`: 0 while a `Pause` menu is open (the gameplay sets do not run), 0.4 in the
/// end-of-level slow motion. `debug_factor` is a development hook (not in the original) that
/// multiplies the game speed.
#[derive(Resource, Debug, Clone, Copy)]
pub struct TimeScale {
    pub scale: f32,
    pub debug_factor: f32,
}

impl Default for TimeScale {
    fn default() -> Self {
        TimeScale { scale: 1.0, debug_factor: 1.0 }
    }
}

/// Run condition for the gameplay sets: `FixedUpdate` does not run and `Update` sees a zero
/// delta while `Time.timeScale` is 0, so nothing moves.
pub fn not_paused(time_scale: Res<TimeScale>) -> bool {
    time_scale.scale > 0.0
}

/// What the in-game screens' buttons ask the level loader for.
#[derive(Message, Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadRequest {
    /// `RestartLevelButton` → `LevelLoader.StartGame()`: the selected level again.
    Restart,
    /// `ReturnToLobbyButton` → `LevelLoader.ReturnToLobby()`.
    ReturnToLobby,
    /// `LevelCompleteButton`: the last campaign level goes back to the first and loads the
    /// "Complete" scene; any other advances `LobbyLevelSelect.Index` and returns to the Lobby.
    LevelCompleted,
}

pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TimeScale>()
            .add_message::<LoadRequest>()
            .add_systems(OnExit(AppState::InGame), cleanup_game)
            .add_systems(Last, apply_time_scale)
            .add_systems(Update, load_requests.run_if(in_state(AppState::InGame)));
    }
}

/// `Time.timeScale` drives the virtual clock everything in `Update` reads (the next frame's
/// delta).
fn apply_time_scale(time_scale: Res<TimeScale>, mut time: ResMut<Time<Virtual>>) {
    let speed = time_scale.scale.max(0.0) * time_scale.debug_factor.max(0.0);
    if time.relative_speed() != speed {
        time.set_relative_speed(speed);
    }
}

/// `LevelLoader.RequestLoad`: one load at a time.
fn load_requests(
    mut requests: MessageReader<LoadRequest>,
    mut memory: ResMut<LobbyMemory>,
    mut next: ResMut<NextState<AppState>>,
    mut net: ResMut<crate::net::Net>,
    session: Res<crate::state::Session>,
    source: Res<crate::level::LevelSource>,
) {
    let Some(&request) = requests.read().last() else { return };
    // In a network game loads go through the host, which loads them everywhere
    // (`SendRequestToLoad`).
    if !net.is_offline() {
        use sticks_net::LoadKind;
        let kind = match request {
            LoadRequest::Restart => LoadKind::Normal,
            LoadRequest::ReturnToLobby => LoadKind::Lobby,
            LoadRequest::LevelCompleted => {
                // `LevelCompleteButton`: the shared level index moves on first.
                let count = memory.campaign_levels.len();
                if count > 0 && memory.level_index + 1 >= count {
                    memory.level_index = 0;
                    LoadKind::Complete
                } else {
                    memory.level_index += 1;
                    LoadKind::Lobby
                }
            }
        };
        if net.is_host() {
            let level = match kind {
                LoadKind::Normal => source.0.clone(),
                LoadKind::Lobby => crate::net::LOBBY_LEVEL.to_owned(),
                LoadKind::Complete => crate::net::COMPLETE_LEVEL.to_owned(),
            };
            net.host_load(level, session.mode, session.difficulty, session.player_count);
        } else {
            net.request_load(kind);
        }
        return;
    }
    match request {
        LoadRequest::Restart => next.set(AppState::Loading),
        LoadRequest::ReturnToLobby => next.set(AppState::Lobby),
        LoadRequest::LevelCompleted => {
            let count = memory.campaign_levels.len();
            if count > 0 && memory.level_index + 1 >= count {
                memory.level_index = 0;
                // TODO: `LevelLoader.GameComplete()` loads the "Complete" ending scene, which is
                // not reconstructed; the Lobby is shown instead.
                warn!("campaign complete: the ending scene is not reconstructed; back to the Lobby");
            } else {
                memory.level_index += 1;
            }
            next.set(AppState::Lobby);
        }
    }
}

/// Leaving a game removes everything it spawned; the next game builds it all again.
/// `EndOfLevelTimeSlower.OnDestroy` and `InGameMenuHandler.CloseAllMenusImmediately` leave
/// `Time.timeScale` at 1.
#[allow(clippy::type_complexity)]
fn cleanup_game(
    mut commands: Commands,
    mut time_scale: ResMut<TimeScale>,
    // `Allow<Disabled>`: enemies asleep off screen are disabled entities and must go too.
    doomed: Query<
        Entity,
        (
            Or<(With<LevelGeometry>, With<LevelCamera>, With<Player>, With<Avatar>, With<Projectile>, With<Hud>, With<crate::unity_render::QueueRun>, With<crate::helicopter::Helicopter>, With<crate::level_end::LevelEndEntity>, With<crate::weapon::MeleeSwing>)>,
            Allow<Disabled>,
        ),
    >,
) {
    time_scale.scale = 1.0;
    for entity in &doomed {
        commands.entity(entity).try_despawn();
    }
}
