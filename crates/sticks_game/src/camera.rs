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
            .add_systems(Update, camera_late_update.in_set(FrameSet::Camera))
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
    mut rigs: Query<&mut Rig>,
) {
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    let delta = clock.stepper.frame().delta;
    for mut rig in &mut rigs {
        for (avatar, mortal, mut placeholder) in &mut avatars {
            if mortal.avatar_active() {
                let p = avatar.position();
                rig.add_influence(SimVec2::new(p.x, p.y + CAMERA_TARGET_HEIGHT));
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
        rig.late_update(aspect, delta);
    }
}

fn sync_camera_transform(mut cameras: Query<(&Rig, &mut Transform), With<LevelCamera>>) {
    for (rig, mut transform) in &mut cameras {
        let p = rig.position;
        transform.translation = Vec3::new(p.x, p.y, -p.z);
    }
}
