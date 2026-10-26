//! Integration test against the real exported `Lobby` scene. Skipped when the export is missing.

use std::path::PathBuf;

use unity_data::fonts::FontLibrary;
use unity_data::interactive::load_interactive_scene;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn lobby_screens_buttons_and_texts() {
    let Some(root) = export_root() else {
        eprintln!("export not found; skipping");
        return;
    };
    let lobby = load_interactive_scene(&root, "Assets/Game/Lobby/Lobby.unity").unwrap();

    // The screens `ActivateExclusively` switches between, all inactive in the scene.
    let screens: Vec<&str> = lobby.scripts_named("ActivateExclusively").map(|s| lobby.objects[s.object].name.as_str()).collect();
    for name in ["Splash", "Network Setup", "Control Assignment", "Game Summary", "Mode Select", "Level Select"] {
        assert!(screens.contains(&name), "{name} in {screens:?}");
    }
    for s in lobby.scripts_named("ActivateExclusively") {
        assert!(!lobby.objects[s.object].active_self, "{} starts inactive", lobby.objects[s.object].name);
    }

    // The title screen's offline button leads to control assignment.
    let setup = lobby.scripts_named("NetworkSetup").next().unwrap();
    let play = lobby.field_object(setup, "playOfflineButton").unwrap();
    assert_eq!(lobby.objects[play].name, "1 Play Offline");
    let next = lobby.field_object(setup, "nextScreen").unwrap();
    assert_eq!(lobby.objects[next].name, "Control Assignment");
    assert!(lobby.boxes.iter().any(|b| b.object == play), "buttons are clicked through a BoxCollider");

    // Button captions are text meshes with their fonts.
    let text = lobby.child(play, "Text").unwrap();
    let caption = lobby.texts.iter().find(|t| lobby.object_of(t.object) == Some(text)).unwrap();
    assert_eq!(caption.text, "Play Offline");
    let mut fonts = FontLibrary::new(&root.join("Assets"));
    let font = fonts.get(&caption.font).expect("caption font");
    assert!(font.line_height > 0.0 && !font.dict.is_empty() && font.texture.is_some());
    // Every caption has a shadow copy.
    assert!(lobby.texts.iter().any(|t| t.shadow_of.as_ref().is_some_and(|s| lobby.texts[s.0].object == caption.object)));

    // The camera rig and the light hanging under it.
    let camera = lobby.level.camera.as_ref().unwrap();
    assert_eq!(camera.positioner.as_ref().unwrap().minimum_distance, 23.7);
    let light = lobby.level.light_setup.as_ref().unwrap();
    assert!(light.camera_chain.is_some());

    // Level lists live in the Permanent System scene, not here.
    assert!(lobby.scripts_named("LobbyLevelSelect").next().is_none());
}

#[test]
#[ignore]
fn probe_font() {
    let root = export_root().unwrap();
    let lobby = load_interactive_scene(&root, "Assets/Game/Lobby/Lobby.unity").unwrap();
    let mut fonts = FontLibrary::new(&root.join("Assets"));
    let guids: std::collections::BTreeSet<String> = lobby.texts.iter().map(|t| t.font.clone()).collect();
    for g in guids {
        match fonts.get(&g) {
            Some(f) => eprintln!("{g} {}: lh {} dict {} chars {} tex {:?} mat {}", f.name, f.line_height, f.dict.len(), f.chars.len(), f.texture, f.material_guid),
            None => eprintln!("{g}: not loaded"),
        }
    }
}
