//! Japan's scenery and events (against the real export; skipped without it).

use std::path::PathBuf;

use unity_data::level::load_level;
use unity_data::scenery::BuiltinMesh;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game/Levels/Japan").is_dir().then_some(root)
}

#[test]
fn japan_sky_clouds_lamps_and_emerging_enemies() {
    let Some(root) = export_root() else { return };
    let level = load_level(&root, "Assets/Game/Levels/Japan/Japan.unity").unwrap();
    // The sky: a built-in plane under the "Sky" `FakeParallax`.
    let sky = level.meshes.iter().find(|m| m.name == "Plane").expect("sky plane");
    assert_eq!(sky.mesh, BuiltinMesh::Plane);
    let p = sky.parallax.expect("sky parallax");
    assert_eq!(p.fake_z, 1000.0);
    // The mountains' surface moves with the camera too.
    assert!(level.colliders.iter().any(|c| c.name == "Mountains" && c.surface.as_ref().is_some_and(|s| s.parallax.is_some())));
    // Clouds drift left.
    assert_eq!(level.scrollers.len(), 27);
    assert!(level.scrollers.iter().all(|s| s.velocity < 0.0 && s.left < s.right));
    assert!(level.sprites.iter().any(|s| s.rigid_root == Some(level.scrollers[0].object)));
    // Hanging lamps: 5 health, a collider and sprites each, layer 30 in the campaign.
    let lamps: Vec<_> = level.props.iter().filter(|p| p.name == "Hanging Lamp").collect();
    assert_eq!(lamps.len(), 27);
    assert!(lamps.iter().all(|l| l.starting_health == 5.0 && l.colliders.len() == 1 && l.sprites.len() == 8 && l.campaign_layer == Some(30)));
    assert_eq!(lamps[0].sound.as_deref(), Some("Small Explosion"));
    // Emerging enemies wait for their trigger.
    let emerging: Vec<_> = level.enemies.iter().filter_map(|e| e.emerge.as_ref()).collect();
    assert!(emerging.len() >= 15, "{} emerging enemies", emerging.len());
    for e in &emerging {
        let trigger = e.trigger_object.expect("activation trigger");
        assert!(level.colliders.iter().any(|c| c.object == trigger && c.trigger), "trigger {trigger} has no trigger collider");
    }
    assert!(emerging.iter().any(|e| e.unsubsume_on_completion == ["FireAndMoveIfNecessary"]));
    assert!(emerging.iter().any(|e| e.enable_on_completion == ["Duck"]));
}
