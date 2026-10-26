//! Integration test against the real exported `Platforms` level. Skipped when the export is not
//! available (set `STICK_EXPORT_ROOT` to the `ExportedProject` directory).

use std::path::PathBuf;

use unity_data::level::{Shape, load_level};

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn platforms_level_has_ground_spawns_and_camera() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level(&root, "Assets/Game/Levels/Platforms/Platforms.unity").unwrap();
    assert_eq!(level.name, "Platforms");
    // Four spawn points, in child order 0..4 under "Player Spawn Points".
    assert_eq!(level.spawn_points.len(), 4);
    assert!(level.spawn_points.iter().all(|s| s.facing_right));
    let sp0 = &level.spawn_points[0];
    println!("spawn points: {:?}", level.spawn_points);
    assert!(sp0.position[1] < -50.0);

    // Ground is a PolygonCollider2D on layer 0 contributing several independent solid paths.
    let ground: Vec<_> = level.colliders.iter().filter(|c| c.name == "Ground").collect();
    assert!(ground.len() >= 3, "ground paths: {}", ground.len());
    assert!(ground.iter().all(|c| matches!(c.shape, Shape::Polygon(_)) && !c.trigger && !c.dynamic));
    // Every ground path carries its PolygonColliderRenderer settings (local-space UVs).
    let surfaces: Vec<_> = ground.iter().filter_map(|c| c.surface.as_ref()).collect();
    assert!(!surfaces.is_empty(), "ground has render surfaces");
    for s in &surfaces {
        assert!(s.tiling[0] > 0.0 && !s.is_shadow && !s.material.is_empty());
    }
    // Abyss is an enabled box trigger that kills on enter.
    let abyss = level.colliders.iter().find(|c| c.name == "Abyss").expect("abyss");
    assert!(abyss.trigger);
    assert!(abyss.has_behaviour("DamageOtherOnTriggerEnter"), "{:?}", abyss.behaviours);
    // The "Background" polygon's collider is disabled: no physics, but its enabled
    // `PolygonColliderRenderer` still draws the path (it reads the collider's points regardless).
    let background: Vec<_> = level.colliders.iter().filter(|c| c.name == "Background").collect();
    assert!(!background.is_empty(), "the background is drawn");
    assert!(background.iter().all(|c| !c.enabled && c.surface.is_some()));
    assert!(level.colliders.iter().filter(|c| !c.enabled).all(|c| c.surface.is_some()), "disabled colliders are kept only to be drawn");

    let light = level.light.expect("the scene has exactly one light");
    println!("scene light at {light:?}");
    assert!(light[2] < 0.0, "the light is in front of the scene plane");

    let cam = level.camera.expect("camera");
    assert_eq!(cam.field_of_view, 60.0);
    let pos = cam.positioner.expect("positioner");
    assert_eq!(pos.screen_edge_margin, 18.0);
    assert_eq!(pos.lowest_on_screen_y, -105.0);
    assert_eq!(pos.minimum_distance, 16.0);
    assert_eq!(pos.maximum_distance, 75.0);
}

#[test]
fn skyline_tutorial_parses_quickly_and_has_geometry() {
    let Some(root) = export_root() else { return };
    let path = root.join("Assets/Game/Levels/Skyline/Skyline.unity");
    if !path.exists() {
        return;
    }
    let started = std::time::Instant::now();
    let level = load_level(&root, "Assets/Game/Levels/Skyline/Skyline.unity").unwrap();
    println!("Skyline: {} colliders, {} spawns, parsed in {:?}", level.colliders.len(), level.spawn_points.len(), started.elapsed());
    let mut tags = std::collections::BTreeMap::new();
    for c in &level.colliders {
        *tags.entry((c.layer, c.tag.clone(), c.trigger, c.dynamic)).or_insert(0usize) += 1;
    }
    println!("{tags:#?}");
    assert!(!level.colliders.is_empty());
    assert!(!level.spawn_points.is_empty());
}

#[test]
fn character_animation_data_matches_recovered_values() {
    let Some(root) = export_root() else { return };
    let assets = root.join("Assets");
    if !assets.join("AnimatorController/Character.controller").exists() {
        return;
    }
    let data = unity_data::anim::load_character_animation(&assets).unwrap();
    assert!((data.human_scale - 1.756_332_9).abs() < 1e-6);
    let c = &data.controller;
    let idle = c.state_index("Idle").unwrap();
    assert_eq!(c.default_state, idle);
    let run = &c.states[c.state_index("Run Forwards").unwrap()];
    assert_eq!(run.speed, 1.4);
    match &run.motion {
        unity_data::anim::Motion::Blend { parameter, children } => {
            assert_eq!(parameter, "Forward Velocity");
            assert_eq!(children.len(), 2);
            assert_eq!((children[0].threshold, children[0].time_scale), (0.0, 0.5));
            assert_eq!((children[1].threshold, children[1].time_scale), (1.0, 1.0));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(c.states[c.state_index("Falling").unwrap()].tag, "Airborne");
    // The Ducking clip animates "Collider Height Fraction" to a constant 0.81666666.
    let duck = &c.states[c.state_index("Ducking").unwrap()];
    let unity_data::anim::Motion::Clip(guid) = &duck.motion else { panic!() };
    let curve = data.clips[guid].parameter_curve("Collider Height Fraction").expect("curve");
    assert!((curve.first_value() - 0.816_666_66).abs() < 1e-6);
}
