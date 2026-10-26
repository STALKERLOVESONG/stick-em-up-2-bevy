//! Integration test: the texts of the real `Skyline` level once `ControlTypeDependentActivator`
//! has switched on the keyboard-and-mouse variant of each tutorial text.

use std::path::PathBuf;

use unity_data::level::load_level;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn skyline_tutorial_texts_are_the_keyboard_variants_and_explodable() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let level = load_level(&root, "Assets/Game/Levels/Skyline/Skyline.unity").unwrap();
    for t in &level.texts {
        eprintln!("{:?} explodable={} shadower={:?} at {:?}: {:?}", t.text.name, t.explodable, t.shadower, t.text.world.t, t.text.text);
    }
    let tutorial: Vec<_> = level.texts.iter().filter(|t| t.explodable).collect();
    assert!(!tutorial.is_empty());
    // Only the keyboard-and-mouse children are switched on.
    assert!(tutorial.iter().all(|t| t.text.name == "KeyboardAndMouseProcessor"), "{:?}", tutorial.iter().map(|t| &t.text.name).collect::<Vec<_>>());
    // Each casts a shadow (`TextShadower`) and has text and a font.
    assert!(tutorial.iter().all(|t| t.shadower.is_some() && !t.text.text.is_empty() && !t.text.font.is_empty()));
}
