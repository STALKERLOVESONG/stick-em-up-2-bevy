//! Integration test: the enemies placed in the real exported `Skyline` level. Skipped when the
//! export is not available (set `STICK_EXPORT_ROOT` to the `ExportedProject` directory).
//!
//! Expected values were read from `Skyline.unity` by hand (the controller child "Enemy" of each
//! `Man` root carries `EnemyCharacterInput` and its behaviours).

use std::path::PathBuf;

use unity_data::level::{EnemySpawn, TargetQualifier, load_level};

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

fn near(a: [f32; 3], b: [f32; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
}

#[test]
fn skyline_enemies() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level(&root, "Assets/Game/Levels/Skyline/Skyline.unity").unwrap();
    assert_eq!(level.enemies.len(), 21);
    let count = |pred: &dyn Fn(&EnemySpawn) -> bool| level.enemies.iter().filter(|e| pred(e)).count();
    // Three behaviour sets: stand and shoot, duck and shoot, shoot or close in.
    assert_eq!(count(&|e| e.fire_and_move_if_necessary && e.move_to_first_target && e.fire_at_first_target && !e.duck), 6);
    assert_eq!(count(&|e| e.duck && e.fire_at_first_target && !e.move_to_first_target), 6);
    assert_eq!(count(&|e| !e.duck && e.fire_at_first_target && !e.move_to_first_target && !e.fire_and_move_if_necessary), 9);
    assert_eq!(count(&|e| e.emerge.is_some()), 0);
    assert_eq!(count(&|e| e.selected_weapon.as_deref() == Some("Rocket Launcher")), 20);

    for e in &level.enemies {
        assert!(near(e.controller_offset, [0.0, 1.8, 0.0]), "{}: {:?}", e.name, e.controller_offset);
        assert_eq!(e.starting_health, Some(15.0));
        assert_eq!(e.deactivate_off_screen, Some(3.0));
        assert_eq!(e.body_color, Some([120, 120, 120, 255]));
        assert_eq!(e.helmet_color, Some([0.3529412, 0.3529412, 0.3529412, 1.0]));
        let health = e.health_scaling.as_ref().unwrap();
        assert_eq!((health.amount.as_str(), health.affected_by_player_count), ("50%", true));
        let rate = e.firing_rate_scaling.as_ref().unwrap();
        assert_eq!((rate.amount.as_str(), rate.affected_by_player_count), ("-15%", true));
        let targeter = e.targeter.as_ref().unwrap();
        assert_eq!(targeter.distance_sorters, 1);
        assert_eq!(targeter.qualifiers.len(), 2);
        assert!(targeter.qualifiers.contains(&TargetQualifier::MaximumDistance(100.0)));
        assert!(targeter.qualifiers.contains(&TargetQualifier::LineOfSight(1)));
        let rocket = e.launcher_modifiers.iter().find(|m| m.weapon == "Rocket Launcher").unwrap();
        assert_eq!((rocket.firing_rate_time, rocket.projectile.as_deref()), (1.3, None));
        let shuriken = e.launcher_modifiers.iter().find(|m| m.weapon == "Shuriken").unwrap();
        assert_eq!(shuriken.projectile.as_deref(), Some("51f93a7afe30f4344a26f34375d5dcfe"));
    }

    // The one rifleman stands near the start.
    let weak: Vec<&EnemySpawn> = level.enemies.iter().filter(|e| e.name == "Enemy Still Weak").collect();
    assert_eq!(weak.len(), 1);
    let weak = weak[0];
    assert!(near(weak.position, [211.0, 0.0, -0.1]), "{:?}", weak.position);
    assert_eq!(weak.selected_weapon.as_deref(), Some("Assault Rifle"));
    let rifle = weak.launcher_modifiers.iter().find(|m| m.weapon == "Assault Rifle").unwrap();
    assert_eq!(rifle.firing_rate_time, 1.0);

    // A ducking rocketeer.
    assert!(level.enemies.iter().any(|e| e.name == "Enemy Ducking" && e.duck && near(e.position, [315.0, 2.0, -0.1])));
}
