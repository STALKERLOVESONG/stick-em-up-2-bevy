//! Integration test: the real `Moon Race` level (difficulty-dependent track, gravity).

use std::path::PathBuf;

use unity_data::level::{AwakeConditions, load_level_for};

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn moon_race_draws_its_track_and_has_moon_gravity() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level_for(&root, "Assets/Game/Levels/Moon/Moon Race.unity", Some(AwakeConditions { player_count: 1, difficulty: 0 })).unwrap();
    // `GravityChanger`.
    assert_eq!(level.gravity, Some([0.0, -8.055]));
    // The Regular track's `Ground` is drawn by the `PolygonColliderRenderer` on its "Base Visual"
    // child, which names the parent's collider.
    let ground: Vec<_> = level.colliders.iter().filter(|c| c.name == "Ground" && c.surface.is_some()).collect();
    assert_eq!(ground.len(), 1, "{:?}", level.colliders.iter().map(|c| (&c.name, c.surface.is_some())).collect::<Vec<_>>());
    // One rover for one player (`MinimumPlayers` removes the other three).
    assert_eq!(level.cars.len(), 1);
    assert!(level.cars[0].driver.as_ref().is_some_and(|d| d.animation_state == "Driving"));
    assert!(level.time_limit.is_some());
}

#[test]
fn pirates_whales_are_dynamic_platforms() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = unity_data::level::load_level(&root, "Assets/Game/Levels/Pirates/Pirates.unity").unwrap();
    assert_eq!(level.whales.len(), 7);
    for w in &level.whales {
        assert_eq!((w.up_time, w.down_time, w.transition_time, w.up_y_offset), (4.0, 4.0, 1.0, 14.0));
        // Each whale's polygon collider is a dynamic collider of the level.
        assert!(level.colliders.iter().any(|c| c.object == w.object && c.dynamic && !c.trigger), "whale {}", w.object);
        // Its sprites ride on it.
        assert!(level.sprites.iter().any(|s| s.rigid_root == Some(w.object)));
    }
}

#[test]
fn corruption_2_boss_explodes_constantly() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = unity_data::level::load_level(&root, "Assets/Game/Levels/Corruption 2/Corruption 2.unity").unwrap();
    let constant: Vec<_> = level.enemies.iter().filter_map(|e| e.constant_explosion.map(|t| (e.name.as_str(), t))).collect();
    assert_eq!(constant.len(), 1, "{constant:?}");
    assert_eq!(constant[0].0, "Big Bad");
    assert!((constant[0].1 - 1.0 / 6.0).abs() < 1e-5);
}

#[test]
fn moon_has_a_lunar_module_with_passenger_seats() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level_for(&root, "Assets/Game/Levels/Moon/Moon.unity", Some(AwakeConditions { player_count: 1, difficulty: 0 })).unwrap();
    let lm = level.cars.iter().find(|c| c.rocket.is_some()).expect("the lunar module");
    assert_eq!(lm.name, "Lunar Module");
    let r = lm.rocket.unwrap();
    assert_eq!((r.max_acceleration, r.max_angular_velocity, r.max_angular_acceleration), (20.0, 60.0, 135.0));
    assert_eq!(lm.nodes[r.engine.unwrap()].name, "Rocket");
    assert_eq!(lm.passengers.len(), 3);
    assert!(lm.wheels.is_empty() && !lm.collider.is_empty() && !lm.explodes);
    // The rovers explode; the module does not.
    assert!(level.cars.iter().filter(|c| c.rocket.is_none()).all(|c| c.explodes));
}

#[test]
fn pirates_has_a_deck_cannon() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = unity_data::level::load_level(&root, "Assets/Game/Levels/Pirates/Pirates.unity").unwrap();
    assert_eq!(level.cannons.len(), 1);
    let c = &level.cannons[0];
    assert_eq!(c.launchers.len(), 1);
    let l = &c.launchers[0];
    assert!((l.firing_rate_time - 0.7).abs() < 1e-6 && l.fully_automatic);
    assert_eq!(l.projectile, "a1b0db6803474074ca87e06834a10645");
    // The launch point sits 0.42 + 1.63 along the pivot's x.
    let (_, at) = c.parts.iter().find(|(id, _)| *id == l.object).unwrap();
    assert!((at.t[0] - 2.05).abs() < 1e-3 && at.t[1].abs() < 1e-3, "{:?}", at.t);
    assert!(c.barrel.is_some());
    // The barrel's sprite (and its shadow copy) turn with the pivot.
    assert!(level.sprites.iter().filter(|s| c.parts.iter().any(|(id, _)| *id == s.object)).count() >= 2);
}

#[test]
fn skyline_player_helicopter_gunner_seats() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    for level in ["Skyline/Skyline", "Skyline/Skyline 2"] {
        let level = load_level_for(&root, &format!("Assets/Game/Levels/{level}.unity"), Some(AwakeConditions { player_count: 4, difficulty: 0 })).unwrap();
        for h in level.helicopters.iter().filter(|h| h.enemy.is_none()) {
            let seats: Vec<_> = h.guns.iter().filter(|g| g.control_slot).collect();
            eprintln!("{}: {} guns, {} seats, occupants {:?}, moves {:?}", h.name, h.guns.len(), seats.len(), seats.iter().map(|g| g.occupant.map(|o| h.nodes[o].name.clone())).collect::<Vec<_>>(), seats.iter().map(|g| g.moves_to_pilot).collect::<Vec<_>>());
            for g in seats {
                eprintln!("  seat radius {}", g.control_radius);
                assert!(g.occupant.is_some(), "{}: a gunner seat shows its gunner", h.name);
            }
        }
    }
}

#[test]
fn office_goal_props_and_the_ending_scene() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let office = load_level_for(&root, "Assets/Game/Levels/Office/Office.unity", Some(AwakeConditions { player_count: 1, difficulty: 0 })).unwrap();
    let goal_props = office.props.iter().filter(|p| p.destroy_all_for_goal).count();
    eprintln!("Office: {goal_props} DestroyAllForGoal props of {}", office.props.len());
    assert!(goal_props > 0);
    assert!(!office.hides_player_info);
    let complete = load_level_for(&root, "Assets/Game/Levels/Complete/Complete.unity", Some(AwakeConditions { player_count: 1, difficulty: 0 })).unwrap();
    assert!(complete.hides_player_info);
    let prompts: Vec<_> = complete.texts.iter().filter(|t| t.next_goal_prompt).collect();
    eprintln!("Complete: {} texts, {} spawn points, {} cannons; prompt layer {:?}", complete.texts.len(), complete.spawn_points.len(), complete.cannons.len(), prompts.iter().map(|t| t.layer).collect::<Vec<_>>());
    assert_eq!(prompts.len(), 1);
}

#[test]
fn rover_gunner_seat_holds_a_rocket_launcher() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level_for(&root, "Assets/Game/Levels/Moon/Moon.unity", Some(AwakeConditions { player_count: 2, difficulty: 0 })).unwrap();
    let rovers: Vec<_> = level.cars.iter().filter(|c| c.rocket.is_none()).collect();
    assert!(!rovers.is_empty());
    for car in rovers {
        eprintln!("{}: gunners {:?}", car.name, car.gunners.iter().map(|g| (&car.nodes[g.seat.node].name, &g.weapon, &g.seat.animation_state, g.seat.enabled, g.control_radius)).collect::<Vec<_>>());
        assert_eq!(car.gunners.len(), 1);
        assert_eq!(car.gunners[0].weapon, "Rocket Launcher");
        assert!(!car.gunners[0].seat.prefab.is_empty());
        eprintln!("  engine {:?}", car.engine_sound.as_ref().map(|e| (e.volume, e.pitch, e.looping, e.play_on_awake)));
    }
    for car in level.cars.iter().filter(|c| c.rocket.is_some()) {
        eprintln!("{}: engine {:?}", car.name, car.engine_sound.as_ref().map(|e| (e.volume, e.pitch, e.looping, e.play_on_awake)));
        let (node, exhaust) = car.exhaust.as_ref().expect("the lunar module's exhaust");
        eprintln!("  exhaust at {} local {} rate {:?} lines {:?} shape {:?}", car.nodes[*node].name, exhaust.system.local_space, exhaust.system.emission.rate.scalar, exhaust.lines, exhaust.system.shape.kind);
    }
}

#[test]
fn camera_offsets_changers_and_the_boss_colours() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let conditions = Some(AwakeConditions { player_count: 1, difficulty: 0 });
    let pirates = load_level_for(&root, "Assets/Game/Levels/Pirates/Pirates.unity", conditions).unwrap();
    assert_eq!(pirates.camera.as_ref().unwrap().parent_offset, [0.0, 5.0]);
    assert_eq!(pirates.offset_changers, vec![(253.0, 13.0), (450.0, 0.0)]);
    let japan = load_level_for(&root, "Assets/Game/Levels/Japan/Japan.unity", conditions).unwrap();
    assert_eq!(japan.camera.as_ref().unwrap().parent_offset, [0.0, 0.0]);
    let corruption = load_level_for(&root, "Assets/Game/Levels/Corruption 2/Corruption 2.unity", conditions).unwrap();
    assert_eq!(corruption.camera.as_ref().unwrap().parent_offset, [0.0, 6.0]);
    let boss: Vec<_> = corruption.enemies.iter().filter(|e| e.color_cycle.is_some()).map(|e| (e.name.clone(), e.color_cycle)).collect();
    assert_eq!(boss, vec![("Big Bad".to_owned(), Some(10.0))]);
    let moon = load_level_for(&root, "Assets/Game/Levels/Moon/Moon.unity", conditions).unwrap();
    assert!(moon.cars.iter().any(|c| c.rocket.is_some() && c.ground_raycast_camera));
    assert!(moon.cars.iter().all(|c| c.rocket.is_some() || !c.ground_raycast_camera));
    let dock = moon.dock.as_ref().expect("Moon's Ender docks the lunar module");
    let lunar = moon.cars.iter().find(|c| c.rocket.is_some()).unwrap();
    assert_eq!(dock.to_dock, lunar.root);
    eprintln!("dock joints {:?}", dock.joints);
    assert_eq!(dock.joints.len(), 2);
    assert!(moon.enders.iter().any(|e| e.ender.as_ref().is_some_and(|x| x.object == dock.ender)), "the Dock sits on a level ender's Ender");
}
