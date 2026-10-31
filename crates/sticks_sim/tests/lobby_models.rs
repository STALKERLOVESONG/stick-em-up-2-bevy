//! The Lobby's posed models drop onto their screen's ground and stand there (against the real
//! `Lobby.unity`). Skipped when the export is missing.

mod common;

use sticks_sim::lobby_models::LobbyModel;
use sticks_sim::setup::build_world;
use sticks_sim::visual::CharacterPalette;
use sticks_sim::Vec2;
use unity_data::interactive::load_interactive_scene;

const DT: f32 = 1.0 / 60.0;

#[test]
fn summary_and_customisation_models_land_on_their_ground() {
    let Some(root) = common::export_root() else { return };
    if !root.join("Assets/Game").is_dir() {
        return;
    }
    let scene = load_interactive_scene(&root, "Assets/Game/Lobby/Lobby.unity").unwrap();
    let world = build_world(&scene.level);
    let settings = unity_data::settings::parse_physics2d(&std::fs::read_to_string(root.join("ProjectSettings/Physics2DSettings.asset")).unwrap()).unwrap();
    let rules = sticks_sim::setup::physics_rules(&settings);
    let animation = sticks_sim::animator::AnimatorModel::new(unity_data::anim::load_character_animation(&root.join("Assets")).unwrap());
    let palette = CharacterPalette { body: [120, 120, 120, 255], face: [200, 150, 120, 255], helmet: [0.35, 0.35, 0.35, 1.0] };
    for (name, ground_top) in [("Summary Model 0", 14.271515 - 6.9), ("Body Model 0", f32::NAN)] {
        let object = scene.root(name).unwrap_or_else(|| panic!("{name}"));
        let p = scene.objects[object].world.t;
        let mut model = LobbyModel::new(animation.clone(), &rules, Vec2::new(p[0], p[1]), palette);
        for frame in 1..=180u64 {
            model.fixed_step(&world, &rules, DT, frame);
            model.frame_update(DT);
        }
        let at = model.character.position();
        assert!(model.character.is_grounded(), "{name} did not land: {at:?} from {p:?}");
        assert!(at.y < p[1] && p[1] - at.y < 3.0, "{name} fell to {at:?} from {p:?}");
        if !ground_top.is_nan() {
            assert!((at.y - ground_top).abs() < 0.05, "{name} stands at {at:?}, platform top {ground_top}");
        }
        assert!((at.x - p[0]).abs() < 0.05, "{name} drifted: {at:?}");
        // Posed holding the Lobby's only allowed weapon.
        let assets = root.join("Assets");
        let rig = unity_data::rig::load_man_rig(&assets).unwrap();
        let weapon = sticks_sim::lobby_models::equipped_weapon(&rig, scene.level.allowed_weapons.as_ref());
        assert_eq!(weapon.map(|w| rig.weapons[w].name.as_str()), Some("Shuriken"));
        let avatar = unity_data::humanoid::parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap()).unwrap();
        let model_rig = sticks_sim::pose::CharacterRigModel::new(avatar, rig).unwrap();
        let visual = model.visual(&model_rig, weapon, None);
        assert!(!visual.lines.is_empty());
    }
}
