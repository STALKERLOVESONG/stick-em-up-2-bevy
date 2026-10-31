//! Integration test: the helicopters of the real `Skyline 2` level.

use std::path::PathBuf;

use unity_data::level::load_level;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn skyline_2_has_player_and_enemy_helicopters() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level(&root, "Assets/Game/Levels/Skyline/Skyline 2.unity").unwrap();
    for h in &level.helicopters {
        eprintln!(
            "{} at {:?}: body {:?}, {} collider paths ({:?} pts), health {} {:?}, {} guns, rotors {:?}, pilot {:?}, {} passengers, enemy {:?}",
            h.name,
            h.world.t,
            h.body,
            h.collider.len(),
            h.collider.iter().map(Vec::len).collect::<Vec<_>>(),
            h.starting_health,
            h.health_scaling,
            h.guns.len(),
            h.rotors.iter().map(|r| (r.top, r.angular_velocity, r.radius)).collect::<Vec<_>>(),
            h.pilot.as_ref().map(|p| &p.animation_state),
            h.passengers.len(),
            h.enemy.as_ref().map(|e| (e.move_to_first_target_height, e.fire_at_first_target, e.body_color, e.gunner_count.as_ref().map(|g| g.0), e.settings.targeter.as_ref().map(|t| t.qualifiers.len()))),
        );
    }
    let enemies: Vec<_> = level.helicopters.iter().filter(|h| h.enemy.is_some()).collect();
    let players: Vec<_> = level.helicopters.iter().filter(|h| h.enemy.is_none()).collect();
    assert_eq!(enemies.len(), 7);
    assert_eq!(players.len(), 3);
    // "Enemy Helicopter Grounded" sits with stopped rotors.
    assert!(enemies.iter().any(|h| h.name == "Enemy Helicopter Grounded" && h.rotors.iter().all(|r| r.angular_velocity == 0.0)));
    for h in &level.helicopters {
        // Enemy helicopters weigh 1000, the player's 1.
        assert_eq!(h.body.mass, if h.enemy.is_some() { 1000.0 } else { 1.0 });
        assert_eq!(h.body.gravity_scale, 0.0);
        assert!((h.body.angular_drag - 0.05).abs() < 1e-6);
        assert!(!h.collider.is_empty());
        assert_eq!(h.rotors.len(), 2);
        assert!(h.pilot.as_ref().is_some_and(|p| p.animation_state == "Driving"));
    }
    let e = enemies[0];
    assert_eq!(e.starting_health, 75.0);
    assert_eq!(e.guns.len(), 3);
    assert!(e.guns.iter().all(|g| (g.firing_rate_time - 0.4).abs() < 1e-6 && g.fully_automatic));
    let enemy = e.enemy.as_ref().unwrap();
    assert!(enemy.move_to_first_target_height && enemy.fire_at_first_target);
    assert_eq!(enemy.settings.deactivate_off_screen, Some(14.0));
    assert_eq!(players[0].starting_health, 150.0);

    // Boarding: players' helicopters keep their `ControlSlot` (radius 10), their "Target" and
    // their respawner; `RemovePlayerOnlyParts` strips all three from enemies.
    for h in &players {
        assert!(h.control_slot && h.respawns);
        assert_eq!(h.control_radius, 10.0);
        let target = h.target.expect("player helicopter Target");
        assert_eq!(h.nodes[target].name, "Target");
        let d = h.disembarkment_point.expect("Disembarkment Point");
        assert!((h.nodes[d].local_position[1] - 1.43).abs() < 1e-5);
        // The camera influencer sits 10 ahead of the root, on the "Offset" (y 2.732).
        let c = h.camera_influencer.expect("Camera Influencer");
        let world = h.world_of(c, &h.world, &|_| None);
        assert!((world.t[0] - h.world.t[0] - 10.0).abs() < 1e-3, "{:?}", world.t);
        assert!((world.t[1] - h.world.t[1] - 2.732_114_8).abs() < 1e-3, "{:?}", world.t);
        // The single-player gun is one of the gunner seats; every gun fires an assault-rifle sound.
        let gun = h.single_player_gun.expect("singlePlayerGunnerControl");
        assert!(h.guns[gun].control_slot);
        assert!(h.guns.iter().all(|g| g.firing_sound == "Assault Rifle SG 552"), "{:?}", h.guns.iter().map(|g| &g.firing_sound).collect::<Vec<_>>());
        // The engine: heli_loop, looping, fully 3D, log rolloff 1..500, the "Helicopter" group.
        let engine = h.engine_sound.as_ref().expect("engine AudioSource");
        assert_eq!(engine.clip, "11a33c9a95eee8f448431b0ef5fd67c4");
        assert!(engine.looping && engine.play_on_awake);
        assert_eq!((engine.min_distance, engine.max_distance, engine.rolloff, engine.spatial_blend), (1.0, 500.0, 0, 1.0));
        assert_eq!(engine.mixer_group, Some(24_300_104));
    }
    let silent: Vec<&str> = enemies.iter().filter(|h| h.engine_sound.is_none()).map(|h| h.name.as_str()).collect();
    eprintln!("enemy helicopters without an engine AudioSource: {silent:?}");
    assert!(silent.len() < enemies.len());
    for h in &enemies {
        assert!(!h.control_slot && !h.respawns && h.target.is_none());
        assert!(h.guns.iter().all(|g| !g.control_slot));
    }
}

/// `MinimumPlayers` on the Skyline helicopter's second and third guns (3 and 4 players): a
/// one-player game keeps only the pilot's gun.
#[test]
fn skyline_helicopter_guns_follow_minimum_players() {
    let Some(root) = export_root() else { return };
    let active_guns = |players: usize| {
        let conditions = unity_data::level::AwakeConditions { player_count: players, difficulty: 0 };
        let level = unity_data::level::load_level_for(&root, "Assets/Game/Levels/Skyline/Skyline.unity", Some(conditions)).unwrap();
        let h = &level.helicopters[0];
        h.guns
            .iter()
            .filter(|g| {
                let mut current = Some(g.node);
                while let Some(c) = current {
                    if !h.nodes[c].active_self {
                        return false;
                    }
                    current = h.nodes[c].parent;
                }
                true
            })
            .count()
    };
    assert_eq!(active_guns(1), 1);
    assert_eq!(active_guns(3), 2);
    assert_eq!(active_guns(4), 3);
}

/// `MinimumDifficulty` enemies of Japan only exist on the harder settings.
#[test]
fn japan_enemies_follow_minimum_difficulty() {
    let Some(root) = export_root() else { return };
    let count = |difficulty: i32| {
        let conditions = unity_data::level::AwakeConditions { player_count: 1, difficulty };
        unity_data::level::load_level_for(&root, "Assets/Game/Levels/Japan/Japan.unity", Some(conditions)).unwrap().enemies.len()
    };
    let (easy, hard) = (count(-1), count(1));
    assert!(hard > easy, "beginner {easy}, expert {hard}");
}
