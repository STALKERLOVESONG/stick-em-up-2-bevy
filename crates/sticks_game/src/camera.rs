//! The level camera: a perspective camera driven by the `CameraPositioner` rig.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::camera::{CameraPosition, CameraRig, PositionerSettings};

use crate::enemy::Enemy;
use crate::level::ActiveLevel;
use crate::life::Mortal;
use crate::player::{Avatar, FrameClock};
use unity_compat::Vec2 as SimVec2;
use crate::state::AppState;
use crate::FrameSet;

#[derive(Component)]
pub struct LevelCamera;

/// The positioner rig state (smoothing velocities, influences, settings).
#[derive(Component, Deref, DerefMut)]
pub struct Rig(pub CameraRig);

/// Height of the `Target` child of `Man.prefab` (a `CameraPositionInfluencer`) above the feet.
pub const CAMERA_TARGET_HEIGHT: f32 = 1.8;

/// The `Respawner`'s own `CameraPositionInfluencer`: while an avatar is dead the camera keeps
/// framing the place it died.
#[derive(Component, Default)]
pub struct CameraPlaceholder {
    /// Influence position while the placeholder is enabled.
    pub position: Option<SimVec2>,
    /// On the frame of a respawn the placeholder still contributes one last influence
    /// (`Respawner.Respawn` calls `Update()` before disabling it).
    /// TODO(UNCERTAIN): whether the respawned avatar's own influencer also runs that frame
    /// (assumed yes, so this frame spans the death spot and the respawn spot).
    pub respawn_frame_influence: Option<SimVec2>,
}

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), spawn_camera)
            .add_systems(Update, (change_minimum_distance, camera_late_update).chain().in_set(FrameSet::Camera))
            .add_systems(Update, sync_camera_transform.in_set(FrameSet::Present));
    }
}

fn spawn_camera(mut commands: Commands, level: Res<ActiveLevel>) {
    let Some(setup) = &level.0.camera else {
        error!("level {} has no camera", level.0.name);
        return;
    };
    let [x, y, z] = setup.position;
    let [r, g, b, _] = setup.background;
    commands.insert_resource(ClearColor(Color::srgb(r, g, b)));
    let settings = setup.positioner.clone().unwrap_or_else(PositionerSettings::default);
    let rig = CameraRig::new(settings, setup.field_of_view, CameraPosition { x, y, z });
    commands.spawn((
        LevelCamera,
        Rig(rig),
        Camera3d::default(),
        // The original writes its shader output straight to the screen: no tone mapping.
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Perspective(PerspectiveProjection {
            fov: setup.field_of_view.to_radians(),
            near: setup.near,
            far: setup.far,
            ..default()
        }),
        // Game z is negated for Bevy (see `level::spawn_level_geometry`).
        Transform::from_xyz(x, y, -z),
    ));
}

/// `CameraPositioner.LateUpdate`: every active avatar's `Target`, every enabled respawner
/// placeholder and the "Camera Influencer" of every flown helicopter is a
/// `CameraPositionInfluencer`.
fn camera_late_update(
    clock: Res<FrameClock>,
    windows: Query<&Window, With<PrimaryWindow>>,
    // TODO(UNCERTAIN): whether enemy characters carry a `CameraPositionInfluencer` is not recovered;
    // they are assumed not to.
    mut avatars: Query<(&Avatar, &Mortal, &mut CameraPlaceholder), Without<Enemy>>,
    helicopters: Query<&crate::helicopter::Helicopter>,
    cars: Query<&crate::cars::Car>,
    mut rigs: Query<&mut Rig>,
) {
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    let delta = clock.stepper.frame().delta;
    for mut rig in &mut rigs {
        for (avatar, mortal, mut placeholder) in &mut avatars {
            if mortal.avatar_active() {
                let p = avatar.position();
                rig.add_influence(SimVec2::new(p.x, p.y + CAMERA_TARGET_HEIGHT));
                // The respawner's placeholder only stands in while the avatar is gone. A remote
                // player's avatar comes back from the network without this machine seeing its
                // `Respawned` event, which left the placeholder where it died (in the abyss) and
                // dragged the camera out.
                placeholder.position = None;
            }
            if let Some(p) = placeholder.position {
                rig.add_influence(p);
            }
            if let Some(p) = placeholder.respawn_frame_influence.take() {
                rig.add_influence(p);
            }
        }
        // A flown helicopter's "Camera Influencer" (the pilot's avatar is deactivated meanwhile).
        for h in &helicopters {
            if let Some(p) = h.camera_influence() {
                rig.add_influence(p);
            }
        }
        // A driven car's "Camera Influencer" and `Rigidbody2DPathCameraInfluencer`.
        for car in &cars {
            if let Some(p) = car.camera_influence() {
                rig.add_influence(p);
            }
            if let Some(p) = car.path_influence() {
                rig.add_distance_only_influence(p);
            }
        }
        rig.late_update(aspect, delta);
    }
}

/// `Changer<MinimumCameraDistanceChanger>.Update` (Japan, Pirates, Skyline, Skyline 2): the
/// last changer (in x order) that a player has passed sets the camera's minimum distance; a
/// changer stays passed until every player is more than 10 units behind it (hysteresis); with
/// none passed the level's own minimum distance applies. `EndOfLevelCameraMover` disables them
/// all (recognised here by its framing: no edge margin and no minimum width).
fn change_minimum_distance(
    level: Res<crate::level::ActiveLevel>,
    avatars: Query<(&Avatar, &Mortal), With<crate::player::ControlledBy>>,
    helicopters: Query<&crate::helicopter::Helicopter>,
    mut rigs: Query<&mut Rig, With<LevelCamera>>,
    mut state: Local<Option<(String, f32, Vec<bool>)>>,
) {
    let changers = &level.0.distance_changers;
    if changers.is_empty() {
        return;
    }
    let Ok(mut rig) = rigs.single_mut() else { return };
    if state.as_ref().is_none_or(|(name, _, _)| *name != level.0.name) {
        *state = Some((level.0.name.clone(), rig.settings.minimum_distance, vec![false; changers.len()]));
    }
    let Some((_, default, passed)) = state.as_mut() else { return };
    if rig.settings.screen_edge_margin == 0.0 && rig.settings.minimum_on_screen_width == 0.0 {
        return;
    }
    // `Player.Controlling`: the player's own `Man`, or the vehicle it flies.
    let xs: Vec<f32> = avatars
        .iter()
        .filter(|(_, m)| m.avatar_active())
        .map(|(a, _)| a.position().x)
        .chain(helicopters.iter().filter(|h| h.pilot().is_some()).map(|h| h.body.position.x))
        .collect();
    let mut chosen = None;
    for (i, &(x, distance)) in changers.iter().enumerate() {
        let ahead = xs.iter().any(|&p| p > x);
        let all_behind = !xs.iter().any(|&p| p > x - 10.0);
        if ahead || (passed[i] && !all_behind) {
            chosen = Some(distance);
            passed[i] = true;
        } else {
            passed[i] = false;
        }
    }
    rig.settings.minimum_distance = chosen.unwrap_or(*default);
}

fn sync_camera_transform(mut cameras: Query<(&Rig, &mut Transform), With<LevelCamera>>) {
    for (rig, mut transform) in &mut cameras {
        let p = rig.position;
        transform.translation = Vec3::new(p.x, p.y, -p.z);
    }
}
