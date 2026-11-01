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
