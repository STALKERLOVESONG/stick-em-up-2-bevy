//! Level loading and the static world: the parsed scene and its collision data. What the level
//! looks like is drawn by `unity_render` from the same parse.

use std::sync::Arc;

use bevy::prelude::*;
use sticks_sim::animator::AnimatorModel;
use sticks_sim::physics::{PhysicsRules, World as CollisionWorld};
use sticks_sim::setup::load_level_bundle_with;
use unity_data::level::AwakeConditions;
use unity_data::level::Level;

use crate::config::GameConfig;
use crate::state::{AppState, Session};

/// The scene file of the level to play, relative to the export root (what `LevelLoader` was
/// asked to load).
#[derive(Resource, Clone, Debug)]
pub struct LevelSource(pub String);

/// The parsed level scene (spawn points, camera setup, collider metadata).
#[derive(Resource)]
pub struct ActiveLevel(pub Level);

/// Static collision geometry and the physics rules it is simulated under.
#[derive(Resource)]
pub struct Collision {
    pub world: CollisionWorld,
    pub rules: PhysicsRules,
}

/// Animator controller and clips shared by every character.
#[derive(Resource)]
pub struct CharacterAnimation(pub Arc<AnimatorModel>);

/// Marker for entities that belong to the loaded level.
#[derive(Component)]
pub struct LevelGeometry;

pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, load_level_data.run_if(in_state(AppState::Loading)));
    }
}

/// Reads the level on the second frame of `Loading`, so the loading screen is on display while
/// the (blocking) parse runs.
fn load_level_data(mut commands: Commands, config: Res<GameConfig>, source: Res<LevelSource>, session: Res<Session>, mut next: ResMut<NextState<AppState>>, mut frames: Local<u8>) {
    // Two frames of the Level Loading scene first (the run entities spawned in the first appear
    // in the second).
    if *frames < 2 {
        *frames += 1;
        return;
    }
    *frames = 0;
    // `LevelLoader.PlayerCount` / `Difficulty` as the level's `Awake`s see them.
    let conditions = AwakeConditions { player_count: session.player_count, difficulty: session.difficulty };
    // `AllClientControlTypes` for the `ControlTypeDependentActivator` texts.
    let control_types = session.control_types_name();
    let bundle = load_level_bundle_with(&config.export_root, &source.0, Some(conditions), &control_types).unwrap_or_else(|e| {
        panic!("failed to load level {:?} from {}: {e}", source.0, config.export_root.display())
    });
    info!(
        "loaded {}: {} colliders ({} pieces), {} spawn points",
        bundle.level.name,
        bundle.world.collider_count(),
        bundle.world.piece_count(),
        bundle.level.spawn_points.len()
    );
    commands.insert_resource(ActiveLevel(bundle.level));
    commands.insert_resource(Collision { world: bundle.world, rules: bundle.rules });
    commands.insert_resource(CharacterAnimation(bundle.animation));
    next.set(AppState::InGame);
}
