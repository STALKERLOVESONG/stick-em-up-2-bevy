//! Enemy AI end to end on the real `Skyline` level: the enemies' recovered settings, the real
//! `Man` rig and projectile prefabs, the level's collision for line of sight and shots.
//!
//! One frame follows the game's order: fixed step (characters, then projectiles), the
//! `CharacterInput` copy of the enemy's processor, the enemy's behaviours, `CharacterControl2D`
//! `Update` (animator, aim, weapon switch) and the launcher.
//!
//! Expected values come from the decompiled behaviours and the serialized scene, not from
//! recordings of the original executable.

mod common;

use sticks_sim::ai::{Ballistics, Brain, Candidate, EnemyInput, Targeter};
use sticks_sim::aim::{AimedWeapon, PosedCharacter};
use sticks_sim::character::{Character, CharacterInput, DEFAULT_Z, StandingColliders};
use sticks_sim::life::{Health, LifeCycle};
use sticks_sim::pose::CharacterRigModel;
use sticks_sim::setup::{LevelBundle, load_level_bundle};
use sticks_sim::weapons::{Hit, Launcher, Loadout, Shot, ShotTarget, corrected_aim};
use sticks_sim::Vec2;
use unity_compat::Vec3;
use unity_data::humanoid::parse_avatar;
use unity_data::level::EnemySpawn;
use unity_data::projectile::{ProjectileSpec, load_projectile};
use unity_data::rig::load_man_rig;

const DT: f32 = 1.0 / 60.0;
/// Height of the `Target` child (players) and of the enemy controller object above the feet.
const TARGET_HEIGHT: f32 = 1.8;

struct Fixture {
    bundle: LevelBundle,
    model: CharacterRigModel,
    /// Projectile prefab of each rig weapon (`None` for melee effects).
    projectiles: Vec<Option<ProjectileSpec>>,
}

fn fixture() -> Option<Fixture> {
    let Some(root) = common::export_root() else {
        eprintln!("enemy AI tests skipped: export not available (set STICK_EXPORT_ROOT)");
        return None;
    };
    let assets = root.join("Assets");
    let bundle = load_level_bundle(&root, "Assets/Game/Levels/Skyline/Skyline.unity").unwrap();
    let rig = load_man_rig(&assets).unwrap();
    let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap()).unwrap();
    let projectiles = rig.weapons.iter().map(|w| load_projectile(&assets, &w.projectile).ok()).collect();
    let model = CharacterRigModel::new(avatar, rig).unwrap();
    Some(Fixture { bundle, model, projectiles })
}

impl Fixture {
    fn weapon(&self, name: &str) -> usize {
        self.model.rig().weapons.iter().position(|w| w.name == name).unwrap()
    }

    fn enemy_spawn(&self, name: &str, behaviour: impl Fn(&EnemySpawn) -> bool) -> &EnemySpawn {
        self.bundle.level.enemies.iter().find(|e| e.name == name && behaviour(e)).unwrap()
    }
}

/// A recovered enemy with its weapon, standing where the scene put it.
struct Enemy {
    ch: Character,
    input: EnemyInput,
    controls: CharacterInput,
    brain: Brain,
    targeter: Targeter,
    loadout: Loadout,
    launchers: Vec<Launcher>,
    corrected: Vec2,
    posed: PosedCharacter,
}

impl Enemy {
    fn new(f: &Fixture, spawn: &EnemySpawn) -> Enemy {
        let position = Vec2::new(spawn.position[0], spawn.position[1]);
        let mut ch = Character::new(f.bundle.animation.clone(), &f.bundle.rules, StandingColliders::MAN, position);
        ch.body.layer = 9;
        ch.set_facing_right(true);
        let weapons = &f.model.rig().weapons;
        let names: Vec<&str> = weapons.iter().map(|w| w.name.as_str()).collect();
        let allowed = f.bundle.level.allowed_weapons.clone().unwrap();
        // `ProjectileLauncherModifier`s (campaign, one player, difficulty 0: the -15 % scaling
        // multiplies by zero steps and changes nothing).
        let launchers = weapons
            .iter()
            .enumerate()
            .map(|(i, w)| match spawn.launcher_modifiers.iter().rev().find(|m| m.weapon == w.name && m.firing_rate_time >= 0.0) {
                Some(m) => Launcher::new(m.firing_rate_time, true, false),
                None => Launcher::new(w.firing_rate_time, w.fully_automatic, f.projectiles[i].as_ref().is_some_and(|p| p.chargeable)),
            })
            .collect();
        let input = EnemyInput {
            duck: spawn.duck,
            selected_weapon: spawn.selected_weapon.as_deref().map(|w| f.weapon(w)),
            ..EnemyInput::default()
        };
        Enemy {
            posed: f.model.pose_character(&ch.animator.pose(), ch.position(), true, None),
            ch,
            controls: input.character_input(),
            input,
            brain: Brain::of_enemy(spawn),
            targeter: Targeter::of_enemy(spawn, |name| panic!("unknown qualifier {name}")),
            loadout: Loadout::new(&names, |n| allowed.allows(n, true)),
            launchers,
            corrected: Vec2::new(1.0, 0.0),
        }
    }

    fn origin(&self) -> Vec3 {
        let p = self.ch.position();
        Vec3::new(p.x, p.y + TARGET_HEIGHT, DEFAULT_Z)
    }
}

struct Scene<'f> {
    f: &'f Fixture,
    enemy: Enemy,
    player: Character,
    player_health: Health,
    shots: Vec<Shot>,
    frame: u64,
    time: f32,
    fired: Vec<(usize, Shot)>,
    hits: u32,
    damage_taken: f32,
}

impl<'f> Scene<'f> {
    /// The enemy from the scene and a player `offset` from it, dropped onto the ground.
    fn new(f: &'f Fixture, spawn: &EnemySpawn, offset: Vec2) -> Scene<'f> {
        let enemy = Enemy::new(f, spawn);
        let player = Character::new(f.bundle.animation.clone(), &f.bundle.rules, StandingColliders::MAN, enemy.ch.position() + offset);
        Scene {
            f,
            enemy,
            player,
            player_health: LifeCycle::man().health,
            shots: Vec::new(),
            frame: 0,
            time: 0.0,
            fired: Vec::new(),
            hits: 0,
            damage_taken: 0.0,
        }
    }

    fn step(&mut self) {
        let f = self.f;
        self.frame += 1;
        self.time += DT;
        let idle = CharacterInput { aim: Vec2::new(1.0, 0.0), ..CharacterInput::default() };

        // Fixed step: characters act on the input copied last frame, then projectiles fly.
        self.player.fixed_update(&idle, &f.bundle.world, &f.bundle.rules, DT, self.frame);
        let e = &mut self.enemy;
        e.ch.fixed_update(&e.controls, &f.bundle.world, &f.bundle.rules, DT, self.frame);
        let (player_body, enemy_body) = (&self.player.body, &e.ch.body);
        let mut hits = 0;
        let mut damage = 0.0;
        self.shots.retain_mut(|shot| {
            let targets = [
                ShotTarget { position: player_body.position, shape: &player_body.shape, layer: 8 },
                ShotTarget { position: enemy_body.position, shape: &enemy_body.shape, layer: enemy_body.layer },
            ];
            match shot.fixed_step(&f.bundle.world, &f.bundle.rules, &targets, DT) {
                Some(Hit::Target(0)) => {
                    hits += 1;
                    damage += shot.damage;
                    false
                }
                Some(Hit::Target(_)) => panic!("an enemy shot hit the enemy"),
                Some(Hit::Level(_)) => false,
                None => !shot.age_by(DT),
            }
        });
        self.hits += hits;
        self.damage_taken += damage;
        let _ = self.player_health.damage(damage);

        // `CharacterInput.Update` (order -100): the processor as the previous frame left it.
        e.controls = e.input.character_input();

        // The enemy's behaviours: `Targeter.Targets`, then fire / move.
        let p = self.player.position();
        let candidates = [Candidate { position: Vec3::new(p.x, p.y + TARGET_HEIGHT, DEFAULT_Z), controlled: true }];
        let origin = e.origin();
        let world = &f.bundle.world;
        let hit_triggers = f.bundle.rules.raycasts_hit_triggers;
        let targets = e.targeter.targets(origin, &candidates, |a, b, mask| world.linecast(a, b, mask, hit_triggers));
        let projectile = e.loadout.current().and_then(|w| f.projectiles[w].as_ref()).map(|s| Ballistics {
            propelled: s.propelled,
            gravity_scale: s.gravity_scale,
            speed: s.speed,
        });
        e.brain.think(&mut e.input, &targets, projectile, origin, f.bundle.rules.gravity.y);

        // `CharacterControl2D.Update`, animator, aim and weapon switch.
        self.player.frame_update(&idle, DT);
        e.ch.frame_update(&e.controls, DT);
        let face_right = e.ch.facing_right();
        e.corrected = corrected_aim(e.controls.aim, e.corrected, face_right);
        let equipped = e.loadout.current();
        let aimed = equipped.map(|weapon| AimedWeapon { weapon, aim: e.corrected, face_right });
        e.posed = f.model.pose_character(&e.ch.animator.pose(), e.ch.position(), face_right, aimed);
        if let Some(selected) = e.controls.selected_weapon {
            if e.loadout.current() != Some(selected) {
                e.loadout.select(selected);
            }
        }

        // `ProjectileLauncher.Update`.
        let Some(weapon) = e.loadout.current() else { return };
        let Some(launch) = e.launchers[weapon].update(e.controls.fire, self.time, DT) else { return };
        let spec = f.projectiles[weapon].as_ref().unwrap();
        let muzzle = e.posed.world_position(f.model.rig().weapons[weapon].launch_point.unwrap());
        let shot = Shot::launch(spec, spec.damage, Vec2::new(muzzle.x, muzzle.y), e.controls.aim, launch.sub_frame_offset, 1, f.bundle.rules.gravity);
        self.fired.push((weapon, shot.clone()));
        self.shots.push(shot);
    }

    fn run(&mut self, frames: u32) {
        for _ in 0..frames {
            self.step();
        }
    }
}

#[test]
fn the_rifleman_turns_and_shoots_once_a_second_into_the_player() {
    let Some(f) = fixture() else { return };
    let spawn = f.enemy_spawn("Enemy Still Weak", |_| true);
    // The player stands 10 units to the left on the same rooftop ("Apartment 2", with the Abyss
    // on both sides).
    let mut scene = Scene::new(&f, spawn, Vec2::new(-10.0, 2.0));
    assert!(scene.enemy.controls.aim.x > 0.0, "`DefaultAim` before any target");

    scene.run(1);
    assert!(scene.player.position().y > spawn.position[1] - 1.0, "the player lands on the floor");
    // Frame 1 decided to fire left; the character acts on it from frame 2.
    assert!(scene.enemy.input.fire);
    assert!(scene.enemy.input.relative_aim.unwrap().x < 0.0);
    assert!(scene.enemy.ch.facing_right(), "still the copied default this frame");
    scene.run(1);
    assert!(!scene.enemy.ch.facing_right(), "turned towards the player");
    assert!(scene.enemy.controls.fire);

    scene.run(60 * 4 - 2);
    assert!(scene.player.is_grounded());
    let rifle = f.weapon("Assault Rifle");
    assert!(scene.fired.iter().all(|(w, _)| *w == rifle), "the rifleman keeps its rifle");
    // `firingRateTime` 1 and fully automatic: one staple at once, then one per second.
    assert!((4..=5).contains(&scene.fired.len()), "{} shots in four seconds", scene.fired.len());
    for (_, shot) in &scene.fired {
        assert!(shot.velocity.x < 0.0, "fired towards the player: {:?}", shot.velocity);
    }
    assert!(scene.hits >= 3, "{} of {} staples hit", scene.hits, scene.fired.len());
    assert!((scene.damage_taken - 2.5 * scene.hits as f32).abs() < 1e-4);
    assert!((100.0 - scene.player_health.remaining() - scene.damage_taken).abs() < 1e-3);
    // Standing still the whole time.
    assert_eq!(scene.enemy.input.movement, 0.0);
    assert!((scene.enemy.ch.position().x - spawn.position[0]).abs() < 0.05);
}

#[test]
fn a_fire_and_move_rocketeer_never_walks_while_it_can_shoot() {
    let Some(f) = fixture() else { return };
    // A `FireAndMoveIfNecessary` enemy armed with the Rocket Launcher, moved onto the rifleman's
    // floor so the geometry is the one checked above.
    let mover = f.enemy_spawn("Enemy", |e| e.fire_and_move_if_necessary);
    let floor = f.enemy_spawn("Enemy Still Weak", |_| true);
    let mut spawn = mover.clone();
    spawn.position = floor.position;
    let mut scene = Scene::new(&f, &spawn, Vec2::new(-10.0, 2.0));
    scene.run(60 * 3);
    let rocket = f.weapon("Rocket Launcher");
    // `SetEnemyWeapon` switched to the rockets on the first frame (the rifle is the first allowed
    // weapon, so a first-frame rifle shot is possible).
    assert_eq!(scene.enemy.loadout.current(), Some(rocket));
    let rockets: Vec<&Shot> = scene.fired.iter().filter(|(w, _)| *w == rocket).map(|(_, s)| s).collect();
    // `firingRateTime` 1.3: at 0, 1.3 and 2.6 s.
    assert!((2..=3).contains(&rockets.len()), "{} rockets in three seconds", rockets.len());
    for r in &rockets {
        assert!((r.velocity.magnitude() - 15.0).abs() < 1e-3, "rocket speed {:?}", r.velocity);
        assert!(r.velocity.x < 0.0);
    }
    assert!(scene.damage_taken >= 30.0, "a rocket hit costs 30 ({} taken)", scene.damage_taken);
    // A propelled weapon always has a firing solution, so `MoveToFirstTarget` never runs.
    assert_eq!(scene.enemy.input.movement, 0.0);
    assert!((scene.enemy.ch.position().x - spawn.position[0]).abs() < 0.05, "it stayed put");
}

#[test]
fn nobody_in_range_means_no_shots_and_no_steps() {
    let Some(f) = fixture() else { return };
    let mover = f.enemy_spawn("Enemy", |e| e.fire_and_move_if_necessary);
    let floor = f.enemy_spawn("Enemy Still Weak", |_| true);
    let mut spawn = mover.clone();
    spawn.position = floor.position;
    // `MaximumDistanceTargetQualifier` is 100: a player 130 units away is not a target.
    let mut scene = Scene::new(&f, &spawn, Vec2::new(-130.0, 40.0));
    scene.run(120);
    assert!(scene.fired.is_empty());
    assert!(!scene.enemy.input.fire);
    assert_eq!(scene.enemy.input.movement, 0.0);
    assert!(scene.enemy.ch.facing_right(), "never aimed: still facing `DefaultAim`");
}

