//! Diagnostics behind F1: collider outlines and a line of animator/physics state in the HUD.
//! Not part of the original game.
//!
//! The outlines are gizmos drawn by an overlay camera that copies the level camera (its own
//! render layer, no clear, after the level): in the level camera's pass Bevy sorts line gizmos
//! before the scene's transparent runs, which draw over them (`ZTest Always`).

use bevy::camera::visibility::RenderLayers;
use bevy::camera::ClearColorConfig;
use bevy::prelude::*;
use sticks_sim::character::DEFAULT_Z;

use crate::camera::LevelCamera;
use crate::enemy::Enemy;
use crate::level::ActiveLevel;
use crate::hud::DebugText;
use crate::life::Mortal;
use crate::player::{Avatar, ControlledBy};
use crate::state::AppState;
use unity_data::level::Shape;

/// F1 toggles the overlays.
#[derive(Resource, Default)]
struct ShowLevelColliders(bool);

/// The render layer only the overlay camera sees.
const OVERLAY_LAYER: usize = 2;

/// The camera that draws the F1 gizmos over the level.
#[derive(Component)]
struct OverlayCamera;

pub struct DebugPlugin;

impl Plugin for DebugPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ShowLevelColliders>()
            .add_systems(Startup, |mut store: ResMut<GizmoConfigStore>| {
                store.config_mut::<DefaultGizmoConfigGroup>().0.render_layers = RenderLayers::layer(OVERLAY_LAYER);
            })
            .add_systems(
                Update,
                (toggle_overlays, follow_level_camera, draw_character_colliders, draw_level_colliders, update_debug_text).run_if(in_state(AppState::InGame)),
            );
    }
}

fn toggle_overlays(keys: Res<ButtonInput<KeyCode>>, mut show: ResMut<ShowLevelColliders>) {
    if keys.just_pressed(KeyCode::F1) {
        show.0 = !show.0;
    }
}

/// Keep the overlay camera on the level camera (spawned with the first frame of a level; the
/// game's cleanup removes it with the HUD).
#[allow(clippy::type_complexity)]
fn follow_level_camera(
    mut commands: Commands,
    level: Query<(&Transform, &Projection), (With<LevelCamera>, Without<OverlayCamera>)>,
    mut overlay: Query<(&mut Transform, &mut Projection), (With<OverlayCamera>, Without<LevelCamera>)>,
) {
    let Ok((transform, projection)) = level.single() else { return };
    match overlay.single_mut() {
        Ok((mut t, mut p)) => {
            *t = *transform;
            *p = projection.clone();
        }
        Err(_) => {
            commands.spawn((
                Name::new("Debug overlay camera"),
                OverlayCamera,
                crate::hud::Hud,
                Camera3d::default(),
                Camera { order: 5, clear_color: ClearColorConfig::None, ..default() },
                bevy::core_pipeline::tonemapping::Tonemapping::None,
                projection.clone(),
                *transform,
                RenderLayers::layer(OVERLAY_LAYER),
            ));
        }
    }
}

/// The character's simulated colliders, drawn over its figure while F1 overlays are on.
fn draw_character_colliders(mut gizmos: Gizmos, show: Res<ShowLevelColliders>, players: Query<(&Avatar, &Mortal)>) {
    if !show.0 {
        return;
    }
    let z = -DEFAULT_Z;
    for (avatar, mortal) in &players {
        if !mortal.avatar_active() {
            continue;
        }
        let pos = avatar.position();
        for circle in &avatar.body.shape.circles {
            let c = pos + circle.offset;
            gizmos.circle(Isometry3d::from_translation(Vec3::new(c.x, c.y, z)), circle.radius, Color::srgb(0.2, 0.9, 0.3));
        }
        for b in &avatar.body.shape.boxes {
            let c = pos + b.offset;
            gizmos.rect(
                Isometry3d::from_translation(Vec3::new(c.x, c.y, z)),
                Vec2::new(b.half_extents.x * 2.0, b.half_extents.y * 2.0),
                Color::srgb(0.9, 0.9, 0.2),
            );
        }
    }
}

fn draw_level_colliders(mut gizmos: Gizmos, show: Res<ShowLevelColliders>, level: Res<ActiveLevel>) {
    if !show.0 {
        return;
    }
    for collider in level.0.colliders.iter().filter(|c| c.enabled) {
        let color = if collider.trigger { Color::srgb(1.0, 0.2, 0.2) } else { Color::srgb(0.3, 0.6, 1.0) };
        match &collider.shape {
            Shape::Polygon(pts) => {
                let strip = pts.iter().chain(pts.first()).map(|p| Vec3::new(p[0], p[1], 0.0));
                gizmos.linestrip(strip, color);
            }
            Shape::Edge(pts) => {
                gizmos.linestrip(pts.iter().map(|p| Vec3::new(p[0], p[1], 0.0)), color);
            }
            Shape::Circle { center, radius } => {
                gizmos.circle(Isometry3d::from_translation(Vec3::new(center[0], center[1], 0.0)), *radius, color);
            }
        }
    }
}

fn update_debug_text(
    show: Res<ShowLevelColliders>,
    mut texts: Query<(&mut Text, &mut Visibility), With<DebugText>>,
    avatars: Query<(&Avatar, &Mortal), With<ControlledBy>>,
    enemies: Query<&Mortal, With<Enemy>>,
) {
    let Ok((mut text, mut visibility)) = texts.single_mut() else { return };
    *visibility = if show.0 { Visibility::Inherited } else { Visibility::Hidden };
    let Ok((avatar, mortal)) = avatars.single() else { return };
    if !show.0 {
        return;
    }
    let model = avatar.animator.model();
    let state = model.state_name(avatar.animator.current_state());
    text.0 = format!(
        "{state}{} | x {:.1} y {:.1} | v ({:.2}, {:.2}) | {} | hp {:.0} | target hp {} | {:?}",
        avatar.animator.next_state().map(|s| format!(" -> {}", model.state_name(s))).unwrap_or_default(),
        avatar.position().x,
        avatar.position().y,
        avatar.body.velocity.x,
        avatar.body.velocity.y,
        if avatar.is_grounded() { "grounded" } else { "airborne" },
        mortal.health.remaining(),
        enemies.iter().map(|e| format!("{:.1}", e.health.remaining())).collect::<Vec<_>>().join("/"),
        mortal.stage(),
    );
}
