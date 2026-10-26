//! Weapon, projectile and allowed-weapon data recovered from the real export.

use std::path::PathBuf;

use unity_data::level::load_level;
use unity_data::projectile::load_projectile;
use unity_data::rig::load_man_rig;

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn the_man_carries_six_weapons_in_authored_order() {
    let Some(root) = export_root() else { return };
    let rig = load_man_rig(&root.join("Assets")).unwrap();
    let names: Vec<_> = rig.weapons.iter().map(|w| w.name.as_str()).collect();
    assert_eq!(names, ["Assault Rifle", "Rocket Launcher", "Shuriken", "Pistol", "Sword", "Spear", "No Weapon"]);

    let rifle = &rig.weapons[0];
    assert_eq!(rifle.firing_rate_time, 0.075);
    assert!(rifle.fully_automatic);
    assert_eq!(rifle.firing_sound, "Assault Rifle SG 552");
    assert!(rifle.left_hand.is_some() && rifle.right_hand.is_some());
    assert_eq!(rig.bones[rifle.launch_point.unwrap()].name, "Launch Point");
    assert_eq!(rig.bones[rifle.launch_point.unwrap()].parent, Some(rifle.bone));
    // The launcher measures its aim from its own weapon bone.
    assert_eq!(rifle.aim_pivot, Some(rifle.bone));
    assert_eq!(rifle.sprite_bones.len(), 1);
    assert_eq!(rig.bones[rifle.sprite_bones[0]].flat_sprite.as_ref().unwrap().sprite_name, "AR-15");

    let rocket = &rig.weapons[1];
    assert_eq!(rocket.firing_rate_time, 0.5);
    let pistol = rig.weapons.iter().find(|w| w.name == "Pistol").unwrap();
    assert_eq!(pistol.firing_rate_time, 0.75);
    let shuriken = rig.weapons.iter().find(|w| w.name == "Shuriken").unwrap();
    assert!(!shuriken.fully_automatic);
    assert_eq!(shuriken.firing_rate_time, 0.0);
    // "No Weapon" has hands but nothing to fire.
    let none = rig.weapons.last().unwrap();
    assert!(none.launch_point.is_none() && none.left_hand.is_some());
}

#[test]
fn the_assault_rifle_fires_staples() {
    let Some(root) = export_root() else { return };
    let assets = root.join("Assets");
    let rig = load_man_rig(&assets).unwrap();
    let staple = load_projectile(&assets, &rig.weapons[0].projectile).unwrap();
    assert_eq!(staple.name, "Staple");
    assert_eq!(staple.damage, 2.5);
    assert_eq!(staple.speed, 25.0);
    assert!(staple.propelled && staple.unstoppable);
    assert_eq!(staple.radius, 0.05);
    assert_eq!(staple.lifetime, Some(2.0));
    assert!(staple.explodes_on_collision);
    assert!(staple.damage_scaling.is_none(), "staples are not difficulty scaled");
    assert_eq!(staple.spawn_z, -0.1);
    assert_eq!(staple.sprites.len(), 1);
    assert_eq!(staple.sprites[0].sprite_name, "Staple");
    assert_eq!(staple.sprites[0].scale, [0.35, 0.35, 1.0]);

    let rocket = load_projectile(&assets, &rig.weapons[1].projectile).unwrap();
    assert_eq!((rocket.damage, rocket.speed, rocket.radius), (30.0, 15.0, 0.3));
    assert_eq!(rocket.damage_scaling.as_ref().unwrap().amount, "50%");
    assert!(rocket.spawns_on_explode.is_some());
    assert_eq!(rocket.sprites.len(), 2);
}

#[test]
fn the_platforms_level_keeps_the_rifle_and_the_rocket_launcher() {
    let Some(root) = export_root() else { return };
    let level = load_level(&root, "Assets/Game/Levels/Platforms/Platforms.unity").unwrap();
    let allowed = level.allowed_weapons.expect("Platforms has AllowedWeapons");
    assert!(allowed.allows("Assault Rifle", false) && allowed.allows("Rocket Launcher", false));
    assert!(!allowed.allows("Pistol", false));
    assert!(!allowed.allows("Sword", false));
}

#[test]
fn the_swords_swish_is_not_a_projectile() {
    let Some(root) = export_root() else { return };
    let assets = root.join("Assets");
    let rig = load_man_rig(&assets).unwrap();
    let sword = rig.weapons.iter().find(|w| w.name == "Sword").unwrap();
    assert!(unity_data::projectile::try_load_projectile(&assets, &sword.projectile).unwrap().is_none());
    assert!(load_projectile(&assets, &sword.projectile).is_err());
}
