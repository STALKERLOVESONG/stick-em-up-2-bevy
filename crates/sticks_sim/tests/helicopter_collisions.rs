//! Integration test: the Skyline 2 player helicopter, flown into the level, stays out of the
//! static colliders (H7, `sticks_sim::contact`).

mod common;

use sticks_sim::Vec2;
use sticks_sim::contact::{DynamicBody, Polygon, WarmStart, mass_properties};
use sticks_sim::helicopter::HelicopterControl;
use sticks_sim::physics::{merge_convex, triangulate};
use sticks_sim::rigid_body::RigidBody;
use sticks_sim::setup::{build_world, physics_rules};

#[test]
fn the_skyline_2_helicopter_lands_instead_of_sinking_into_the_roof() {
    let Some(root) = common::export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = unity_data::level::load_level(&root, "Assets/Game/Levels/Skyline/Skyline 2.unity").expect("level");
    let Ok(text) = std::fs::read_to_string(root.join("ProjectSettings/Physics2DSettings.asset")) else {
        eprintln!("Physics2DSettings.asset missing; skipping");
        return;
    };
    let rules = physics_rules(&unity_data::settings::parse_physics2d(&text).expect("settings"));
    let world = build_world(&level);
    let spec = level.helicopters.iter().find(|h| h.enemy.is_none()).expect("player helicopter");

    let scale = spec.world.lossy_scale();
    let pieces: Vec<Polygon> = spec
        .collider
        .iter()
        .flat_map(|path| merge_convex(triangulate(&path.iter().map(|p| Vec2::new(p[0], p[1])).collect::<Vec<_>>())))
        .map(|piece| Polygon::ccw(piece.iter().map(|p| Vec2::new(p.x * scale[0], p.y * scale[1])).collect()))
        .collect();
    assert!(!pieces.is_empty());
    let start = Vec2::new(spec.world.t[0], spec.world.t[1]);
    let mut body = RigidBody::new(start, 0.0, spec.body.mass, spec.body.linear_drag, spec.body.angular_drag, spec.body.gravity_scale);
    let (center, inertia) = mass_properties(&pieces, spec.body.mass);
    body.local_center = center;
    body.inertia = inertia;
    body.inv_inertia = 1.0 / inertia;
    let lowest_local = pieces.iter().flat_map(|p| p.vertices.iter()).map(|v| v.y).fold(f32::MAX, f32::min);

    // What lies under the helicopter's hull, between its skids.
    let under = world.raycast_down(Vec2::new(start.x, start.y + lowest_local), 1);
    eprintln!("helicopter at {start:?}, hull bottom {lowest_local}, colliders below: {under:?}");

    let mut bodies = vec![DynamicBody { key: 1, body, pieces, layer: spec.layer }];
    let mut control = HelicopterControl::default();
    let mut warm = WarmStart::default();
    let dt = 0.02;
    let mut min_y = f32::MAX;
    let mut touched = false;
    for _ in 0..400 {
        control.fixed_update(&mut bodies[0].body, Some(Vec2::new(0.0, -1.0)), dt);
        let touches = sticks_sim::contact::step(&mut bodies, &world, &rules, dt, rules.velocity_iterations, rules.position_iterations, &mut warm);
        touched |= !touches.is_empty();
        min_y = min_y.min(bodies[0].body.position.y);
    }
    let b = &bodies[0].body;
    eprintln!("after 8 s of full down: {:?}, angle {} rad, lowest y {min_y}", b.position, b.angle);
    assert!(touched, "the hull never met the level");
    // Without contacts it would have flown 8 s × 8 units/s ≈ 60 units down.
    assert!(b.position.y > start.y - 40.0, "sank through the level: {:?}", b.position);
    assert!(b.linear_velocity.y.abs() < 1.0, "still moving down: {:?}", b.linear_velocity);
}
