//! Integration test: the level enders, level tips and enemy registration read from the real
//! exported levels. Skipped when the export is not available (set `STICK_EXPORT_ROOT`).
//!
//! Expected values were read from the scene files by hand ("Level Ender Position/Level Ender"
//! with its "Marker" and "Ender" children).

use std::path::PathBuf;

use unity_data::level::{EnderActivator, load_level};

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn skyline_ends_when_every_player_reaches_the_marker() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let Ok(level) = load_level(&root, "Assets/Game/Levels/Skyline/Skyline.unity") else {
        eprintln!("Skyline.unity not readable; skipping");
        return;
    };
    assert_eq!(level.enders.len(), 1);
    let ender = &level.enders[0];
    assert_eq!(ender.name, "Level Ender");
    assert!(ender.game_over_activator && ender.enemy_destroyer);
    let [EnderActivator::AllPlayersTrigger { trigger: Some(trigger) }] = ender.activators.as_slice() else { panic!("{:?}", ender.activators) };
    // The trigger is the marker's own `TriggeredByAllPlayers` (a radius-5 trigger circle).
    let marker = &ender.markers[0];
    assert_eq!(*trigger, marker.object);
    assert!(marker.active);
    assert_eq!(marker.outline.len(), 2, "the exclamation mark's bar and dot");
    assert_eq!(marker.color, [255; 4]);
    assert!((marker.line_thickness - 0.28).abs() < 1e-6);
    assert!((marker.position[0] - 986.75).abs() < 1e-3 && (marker.position[1] - 70.0).abs() < 1e-3);
    let circle = level.colliders.iter().find(|c| c.object == *trigger && c.trigger).expect("trigger circle");
    assert!(matches!(circle.shape, unity_data::level::Shape::Circle { radius, .. } if (radius - 5.0).abs() < 1e-4));
    let end = ender.ender.as_ref().expect("Ender");
    assert!(end.ui && end.time_slower && end.disable_game_overing && end.level_complete);
    assert_eq!(end.camera_minimum_distance, Some(14.0));
    assert_eq!(end.explode, Some(marker.object));
    assert_eq!(end.start_sounds, ["Applause"]);
    // Every placed enemy announces itself.
    assert!(level.enemies.iter().all(|e| e.registered));
    // Skyline has no `LevelTips`.
    assert!(level.tips.is_none());
}

#[test]
fn office_ends_when_every_enemy_is_gone() {
    let Some(root) = export_root() else { return };
    let Ok(level) = load_level(&root, "Assets/Game/Levels/Office/Office.unity") else {
        eprintln!("Office.unity not readable; skipping");
        return;
    };
    let ender = &level.enders[0];
    // Its `ActivateSomethingOnTriggered` is disabled (and the marker inactive): only the enemy
    // count ends Office.
    assert!(!ender.activators.iter().any(|a| matches!(a, EnderActivator::AllPlayersTrigger { trigger: Some(_) })), "{:?}", ender.activators);
    assert!(ender.activators.iter().any(|a| matches!(a, EnderActivator::DestroyAllEnemies { countdown_label: Some(_) })));
    assert!(ender.markers.iter().all(|m| !m.active));
    assert_eq!(level.enemies.iter().filter(|e| e.registered).count(), 24);
}

#[test]
fn pirates_tip_applies_to_every_control_type() {
    let Some(root) = export_root() else { return };
    let Ok(level) = load_level(&root, "Assets/Game/Levels/Pirates/Pirates.unity") else {
        eprintln!("Pirates.unity not readable; skipping");
        return;
    };
    let tips = level.tips.expect("LevelTips");
    assert_eq!(tips.len(), 1);
    assert!(tips[0].applicable_control_types.is_empty());
    assert_eq!(tips[0].message, "Press fire to swing your sword.\nYou can deflect bullets with it!");
}
