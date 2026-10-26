//! Integration tests for the character visual data (rig, humanoid avatar, sprite atlas) read
//! from the real export. Skipped when the export is not available.

use std::path::PathBuf;

use unity_data::humanoid::parse_avatar;
use unity_data::rig::load_man_rig;
use unity_data::sprites::load_collection_with_sprites;

fn assets_dir() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    let assets = root.join("Assets");
    assets.join("Game").is_dir().then_some(assets)
}

#[test]
fn man_rig_has_the_recovered_bone_hierarchy_and_visual_components() {
    let Some(assets) = assets_dir() else { return };
    let rig = load_man_rig(&assets).unwrap();
    assert_eq!(rig.bones[rig.root].name, "Man");
    let hips = rig.bone_by_path("Armature/Hips").unwrap();
    assert!((rig.bones[hips].position[2] - 1.6279181).abs() < 1e-6);
    // Every limb/spine bone with a `LinesToChildren` is body coloured; the helmet is a FlatSprite.
    let helmet = rig.bone_by_path("Armature/Hips/Waist/Spine1/Spine2/Neck/Head/Head Rotation/Head Start/Helmet").unwrap();
    assert_eq!(rig.colors.helmet, helmet);
    assert_eq!(rig.bones[helmet].flat_sprite.as_ref().unwrap().sprite_name, "Helmet");
    assert_eq!(rig.colors.body_color, [239, 66, 66, 255]);
    assert_eq!(rig.colors.face_color, [254, 233, 193, 255]);
    let head_start = rig.bone_by_path("Armature/Hips/Waist/Spine1/Spine2/Neck/Head/Head Rotation/Head Start").unwrap();
    assert_eq!(rig.colors.face, head_start);
    let lines = rig.bones[hips].lines.as_ref().unwrap();
    assert!((lines.width - 0.3).abs() < 1e-6);
    assert!(!lines.skip_children.is_empty());
    assert!(rig.colors.body.contains(&hips));
}

#[test]
fn human_avatar_maps_every_muscle_bone_to_a_rig_bone() {
    let Some(assets) = assets_dir() else { return };
    let rig = load_man_rig(&assets).unwrap();
    let text = std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap();
    let avatar = parse_avatar(&text).unwrap();
    assert!((avatar.scale - 1.7563329).abs() < 1e-6);
    assert_eq!(avatar.nodes.len(), 21);
    assert_eq!(avatar.axes.len(), 19);
    // Hips (HumanBodyBones 0) is human node 1 → Armature/Hips.
    assert_eq!(avatar.bone_node[0], Some(1));
    let bound = avatar.bind_to_rig(&rig);
    assert_eq!(bound.len(), 21);
    assert_eq!(bound[1], rig.bone_by_path("Armature/Hips"));
    assert_eq!(bound[2], rig.bone_by_path("Armature/Hips/Waist"));
    // Every human bone, including the unmapped upper-chest node 4, binds to a rig bone.
    for (node, bone) in bound.iter().enumerate().skip(1) {
        assert!(bone.is_some(), "human node {node} is not in the rig");
    }
    assert_eq!(bound[4], rig.bone_by_path("Armature/Hips/Waist/Spine1/Spine2"));
}

#[test]
fn line_and_helmet_sprites_come_from_the_scene_atlas() {
    let Some(assets) = assets_dir() else { return };
    let scene = assets.join("Game/Levels/Platforms/Platforms.unity");
    let library = load_collection_with_sprites(&assets, &scene, &["Line", "Helmet"]).unwrap();
    let line = library.collection.sprite("Line").unwrap();
    assert_eq!(line.uvs.len(), 4);
    assert!((line.uvs[0][0] - 0.4409182).abs() < 1e-6);
    let texture = library.texture_of(line).expect("Line has an atlas texture");
    assert!(texture.ends_with("atlas0_2.png"));
}
