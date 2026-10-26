//! Bridges recovered serialized data to the simulation types.

use unity_compat::Vec2;
use unity_data::level::{Level, Shape};
use unity_data::settings::Physics2dSettings;

use crate::physics::{PhysicsRules, StaticCollider, StaticShape, World};

/// `Physics2DSettings.asset` as simulation rules.
pub fn physics_rules(settings: &Physics2dSettings) -> PhysicsRules {
    PhysicsRules {
        gravity: Vec2::new(settings.gravity.0, settings.gravity.1),
        layer_matrix: settings.layer_collision_matrix,
        raycasts_hit_triggers: settings.raycasts_hit_triggers,
        box2d: settings.box2d,
    }
}

/// Static collision world of a level. A collider's id is its index in `level.colliders`, so
/// contacts and trigger reports can be mapped back to names and behaviours.
///
/// Colliders under a `Rigidbody2D` (moving platforms, props, vehicles) are left out until the
/// dynamic-body layer exists.
pub fn build_world(level: &Level) -> World {
    let items = level
        .colliders
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.dynamic && c.enabled)
        .map(|(index, c)| {
            let meta = StaticCollider {
                id: index as u32,
                layer: c.layer,
                walljumpable: c.tag == "Walljumpable",
                unsafe_ground: c.tag == "Unsafe Ground",
                one_way: c.tag == "One-way Platform",
                trigger: c.trigger,
                no_safe_standing: c.has_behaviour("HorizontalScroller2D") || c.has_behaviour("Projectile"),
                unsafe_respawn_area: c.has_behaviour("UnsafeRespawnArea"),
            };
            let v = |p: &[f32; 2]| Vec2::new(p[0], p[1]);
            let shape = match &c.shape {
                Shape::Polygon(pts) => StaticShape::Polygon(pts.iter().map(v).collect()),
                Shape::Circle { center, radius } => StaticShape::Circle { center: v(center), radius: *radius },
                Shape::Edge(pts) => StaticShape::Edge(pts.iter().map(v).collect()),
            };
            (meta, shape)
        })
        .collect();
    World::build(items)
}

/// Everything a session needs from disk to play one level.
pub struct LevelBundle {
    pub level: Level,
    pub rules: PhysicsRules,
    pub world: World,
    pub animation: std::sync::Arc<crate::animator::AnimatorModel>,
}

/// Load a level plus the global data it is played with (`level_asset` is relative to the export
/// root, e.g. `Assets/Game/Levels/Platforms/Platforms.unity`).
pub fn load_level_bundle(export_root: &std::path::Path, level_asset: &str) -> std::io::Result<LevelBundle> {
    let level = unity_data::level::load_level(export_root, level_asset)?;
    let settings_text = std::fs::read_to_string(export_root.join("ProjectSettings/Physics2DSettings.asset"))?;
    let settings = unity_data::settings::parse_physics2d(&settings_text)
        .ok_or_else(|| std::io::Error::other("Physics2DSettings.asset could not be parsed"))?;
    let rules = physics_rules(&settings);
    let world = build_world(&level);
    let animation = crate::animator::AnimatorModel::new(unity_data::anim::load_character_animation(&export_root.join("Assets"))?);
    Ok(LevelBundle { level, rules, world, animation })
}
