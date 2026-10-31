//! The versus levels' "Versus Game Ender" (against the real export; skipped without it).

use std::path::PathBuf;

use unity_data::level::load_level;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game/Levels").is_dir().then_some(root)
}

#[test]
fn versus_levels_end_with_the_versus_screen_and_the_loser_destroyer() {
    let Some(root) = export_root() else { return };
    for path in ["Assets/Game/Levels/Japan/Japan VS.unity", "Assets/Game/Levels/Skyline/Skyline VS.unity", "Assets/Game/Levels/Treetops/Treetops VS.unity"] {
        if !root.join(path).is_file() {
            continue;
        }
        let level = load_level(&root, path).unwrap();
        let ender = level.versus_ender.as_ref().unwrap_or_else(|| panic!("{path}: no Versus Game Ender"));
        let e = ender.ender.as_ref().expect("Ender object");
        assert!(e.versus_ui && e.loser_destroyer && e.time_slower, "{path}: {e:?}");
        assert!(!e.ui && !e.level_complete, "{path}: campaign scripts on the versus Ender");
        assert!(e.camera_minimum_distance.is_some());
        assert!(level.spawn_points.len() >= 4, "{path}: {} spawn points", level.spawn_points.len());
    }
    // Campaign levels have none.
    let japan = load_level(&root, "Assets/Game/Levels/Japan/Japan.unity").unwrap();
    assert!(japan.versus_ender.is_none());
}
