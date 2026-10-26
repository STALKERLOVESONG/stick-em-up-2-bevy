//! Projectile impact data and the particle effects reached from gameplay, read from the export.

use std::path::PathBuf;

use unity_data::particles::{ShapeKind, load_particle_effect};
use unity_data::projectile::{ContactExplosion, SpriteSource, load_projectile};

fn export_root() -> Option<PathBuf> {
    let root = std::env::var_os("STICK_EXPORT_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
    root.join("Assets/Game").is_dir().then_some(root)
}

const STAPLE: &str = "7c54e243c8a53704f9f6df8d51039435";
const ROCKET: &str = "ac1358cd551724848bf6412aaa657856";
const SPEAR: &str = "c9d91cde5b802074dbd3f52e240c4a2b";
const SHURIKEN: &str = "e9c617a6b59eb5c4daf13a69b5b3f53e";
const FIREWORK: &str = "573d59d6aefe64a4abbfb3e6a0bea912";
const ROCKET_EXPLOSION: &str = "2ad0a82ef216cff439702832a4ed2cf7";
const ROCKET_TRAIL: &str = "72ceaca1eb034044ba7563a4e1695222";
const FIREWORK_EXPLOSION: &str = "eb558736763b92c4b9505f5e333b3d2a";

#[test]
fn staples_stick_and_fade_after_two_seconds() {
    let Some(root) = export_root() else { return };
    let staple = load_projectile(&root.join("Assets"), STAPLE).unwrap();
    assert_eq!(staple.contact, ContactExplosion::FirstContact);
    let explosion = staple.explosion.expect("ExplodeSpritesAndLines");
    assert!(explosion.sticks);
    assert_eq!(explosion.fade_wait_time, 2.0);
    assert!(staple.despawns_on_explosion_finished && !staple.despawns_on_explode);
    assert!(staple.despawns_on_expiration && !staple.explodes_on_expiration);
    assert_eq!(staple.explosion_sound.as_deref(), Some("Ricochet"));
    assert_eq!(staple.sprites.len(), 1);
    assert_eq!(staple.sprites[0].source, SpriteSource::Named);
    assert_eq!(staple.angular_drag, 0.05);
}

#[test]
fn rockets_burst_into_particles_and_leave_a_trail() {
    let Some(root) = export_root() else { return };
    let rocket = load_projectile(&root.join("Assets"), ROCKET).unwrap();
    assert!(rocket.explosion.is_none(), "no sprite explosion: the rocket just disappears");
    assert!(rocket.despawns_on_explode);
    assert_eq!(rocket.spawns_on_explode.as_deref(), Some(ROCKET_EXPLOSION));
    assert_eq!(rocket.explosion_sound.as_deref(), Some("Small Explosion"));
    assert_eq!(rocket.lifetime_sound.as_deref(), Some("Rocket-Propelled Grenade Launcher RPG-7"));
    assert_eq!(rocket.lifetime_particles.as_ref().map(|(g, _)| g.as_str()), Some(ROCKET_TRAIL));
}

#[test]
fn spears_stick_unless_what_they_hit_was_destroyed_and_shurikens_ricochet() {
    let Some(root) = export_root() else { return };
    let spear = load_projectile(&root.join("Assets"), SPEAR).unwrap();
    assert_eq!(spear.contact, ContactExplosion::UnlessOtherIsDestroyed);
    assert!(spear.explosion.unwrap().sticks);
    assert!(spear.rotates_to_face_velocity && !spear.propelled);
    let by_charge = spear.speed_by_charge.expect("SetProjectileSpeedByCharge");
    assert!(spear.chargeable);
    assert_eq!((by_charge.maximum_charge_time, by_charge.minimum_charge_time), (1.6, 0.25));
    assert_eq!(spear.sprites.len(), 1, "the shadow copy is not a part of its own");
    assert!(matches!(spear.sprites[0].source, SpriteSource::Fixed { sprite_id: 120, .. }));

    let shuriken = load_projectile(&root.join("Assets"), SHURIKEN).unwrap();
    assert!(!shuriken.explosion.unwrap().sticks);
    assert_eq!(shuriken.launch_angular_velocity, 360.0);
    assert_eq!(shuriken.sprites.len(), 4);
    assert!(shuriken.sprites.iter().all(|s| s.shadow_plane_z == -0.2 && s.color == [1.0, 1.0, 0.0, 1.0]));

    let firework = load_projectile(&root.join("Assets"), FIREWORK).unwrap();
    assert!(firework.explodes_on_expiration && !firework.despawns_on_expiration);
    assert_eq!(firework.lifetime, Some(1.32));
    assert_eq!(firework.spawns_on_explode.as_deref(), Some(FIREWORK_EXPLOSION));
}

#[test]
fn the_rocket_explosion_is_a_burst_of_ten_short_lines() {
    let Some(root) = export_root() else { return };
    let fx = load_particle_effect(&root.join("Assets"), ROCKET_EXPLOSION).unwrap().expect("particle system");
    let s = &fx.system;
    assert!(!s.looping && s.local_space);
    assert_eq!(s.duration, 0.1);
    assert_eq!(s.emission.bursts, vec![(0.0, 10)]);
    assert_eq!(s.emission.rate.evaluate(0.0, 0.0), 0.0);
    assert_eq!(s.start_lifetime.evaluate(0.0, 0.5), 0.4);
    assert_eq!(s.start_speed.evaluate(0.0, 0.5), 15.0);
    assert_eq!(s.start_size.evaluate(0.0, 0.5), 0.2);
    assert_eq!(s.shape.kind, ShapeKind::CircleEdge);
    assert_eq!(s.shape.radius, 0.1);
    assert_eq!(s.max_particles, 10);
    // Mid grey: the vertex-lit shader doubles it to white.
    assert_eq!(s.start_color.evaluate(0.0, 0.0), [128, 128, 128, 255]);
    let lines = fx.lines.expect("ParticleSystemLines");
    assert_eq!((lines.line_thickness, lines.fade_time), (0.3, 0.4));
    assert!(fx.rotation_follows_velocity && fx.despawn_on_death);
}

#[test]
fn the_rocket_trail_is_world_space_brown_dots() {
    let Some(root) = export_root() else { return };
    let fx = load_particle_effect(&root.join("Assets"), ROCKET_TRAIL).unwrap().unwrap();
    let s = &fx.system;
    assert!(s.looping && !s.local_space);
    assert_eq!(s.emission.rate.evaluate(0.0, 0.0), 30.0);
    assert!(s.emission.bursts.is_empty());
    // Two constants: 0 .. 0.5.
    assert_eq!(s.start_speed.evaluate(0.0, 0.0), 0.0);
    assert_eq!(s.start_speed.evaluate(0.0, 1.0), 0.5);
    assert_eq!(s.start_size.evaluate(0.0, 0.3), 0.0, "size 0: drawn as dots");
    assert_eq!(s.start_color.evaluate(0.0, 0.0), [0x80, 0x40, 0x00, 0xFF]);
    assert_eq!(s.max_particles, 28);
    assert!((fx.lines.unwrap().fade_time - 2.0 / 3.0).abs() < 1e-6);
}

#[test]
fn firework_explosions_are_a_hundred_white_to_red_streaks() {
    let Some(root) = export_root() else { return };
    let fx = load_particle_effect(&root.join("Assets"), FIREWORK_EXPLOSION).unwrap().unwrap();
    let s = &fx.system;
    assert_eq!(s.emission.bursts, vec![(0.0, 100)]);
    assert_eq!(s.shape.kind, ShapeKind::Circle);
    assert_eq!(s.start_color.evaluate(0.0, 0.0), [255, 255, 255, 255]);
    assert_eq!(s.start_color.evaluate(0.0, 1.0), [255, 0, 0, 255]);
    assert!((s.start_size.evaluate(0.0, 0.0) - 0.01).abs() < 1e-6 && s.start_size.evaluate(0.0, 1.0) == 0.6);
}
