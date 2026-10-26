//! Aim → fire → projectile → collision → damage → target health on the real `Platforms` level,
//! with the real `Man` rig, Assault Rifle and Staple prefab. Skipped when the export is missing.

mod common;

use sticks_sim::aim::{AimedWeapon, PosedCharacter};
use sticks_sim::character::{Character, CharacterInput, StandingColliders};
use sticks_sim::life::LifeCycle;
use sticks_sim::pose::CharacterRigModel;
use sticks_sim::setup::{LevelBundle, load_level_bundle};
use sticks_sim::weapons::{Hit, Launcher, Shot, ShotTarget, corrected_aim};
use sticks_sim::Vec2;
use unity_data::humanoid::parse_avatar;
use unity_data::projectile::{ProjectileSpec, load_projectile};
use unity_data::rig::load_man_rig;

const DT: f32 = 1.0 / 60.0;

struct Fixture {
    bundle: LevelBundle,
    model: CharacterRigModel,
    staple: ProjectileSpec,
}

fn fixture() -> Option<Fixture> {
    let loaded = load_fixture();
    if loaded.is_none() {
        eprintln!("shooting tests skipped: export not available (set STICK_EXPORT_ROOT)");
    }
    loaded
}

fn load_fixture() -> Option<Fixture> {
    let root = common::export_root()?;
    root.join("Assets/Game").is_dir().then_some(())?;
    let assets = root.join("Assets");
    let bundle = load_level_bundle(&root, "Assets/Game/Levels/Platforms/Platforms.unity").ok()?;
    let rig = load_man_rig(&assets).ok()?;
    let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).ok()?)?;
    let staple = load_projectile(&assets, &rig.weapons[0].projectile).ok()?;
    let model = CharacterRigModel::new(avatar, rig)?;
    Some(Fixture { bundle, model, staple })
}

/// A shooter standing on the first spawn point with the rifle up, aiming at `target`'s chest.
struct Shooter {
    ch: Character,
    posed: PosedCharacter,
    corrected: Vec2,
    launcher: Launcher,
    time: f32,
}

impl Shooter {
    fn new(f: &Fixture) -> Shooter {
        let spawn = &f.bundle.level.spawn_points[0];
        let mut ch = Character::new(f.bundle.animation.clone(), &f.bundle.rules, StandingColliders::MAN, Vec2::new(spawn.position[0], spawn.position[1]));
        ch.set_facing_right(true);
        let mut shooter = Shooter {
            posed: f.model.pose_character(&ch.animator.pose(), ch.position(), true, None),
            ch,
            corrected: Vec2::new(1.0, 0.0),
            launcher: Launcher::new(f.model.rig().weapons[0].firing_rate_time, true, false),
            time: 0.0,
        };
        // Settle on the platform with the trigger up.
        for frame in 1..=240u64 {
            shooter.frame(f, frame, Vec2::new(1.0, 0.0), false);
        }
        assert!(shooter.ch.is_grounded(), "the spawn point is on solid ground");
        shooter
    }

    /// One rendered frame with one fixed step: returns the shot the launcher released, if any.
    fn frame(&mut self, f: &Fixture, frame: u64, aim: Vec2, fire: bool) -> Option<Shot> {
        let input = CharacterInput { aim, fire, ..CharacterInput::default() };
        self.ch.fixed_update(&input, &f.bundle.world, &f.bundle.rules, DT, frame);
        self.ch.frame_update(&input, DT);
        self.time += DT;
        let face_right = self.ch.facing_right();
        self.corrected = corrected_aim(aim, self.corrected, face_right);
        let aimed = AimedWeapon { weapon: 0, aim: self.corrected, face_right };
        self.posed = f.model.pose_character(&self.ch.animator.pose(), self.ch.position(), face_right, Some(aimed));
        let launch = self.launcher.update(fire, self.time, DT)?;
        let rifle = &f.model.rig().weapons[0];
        let origin = self.posed.world_position(rifle.launch_point.unwrap());
        Some(Shot::launch(&f.staple, f.staple.damage, Vec2::new(origin.x, origin.y), aim, launch.sub_frame_offset, 0, f.bundle.rules.gravity))
    }

    fn pivot(&self, f: &Fixture) -> Vec2 {
        let p = self.posed.world_position(f.model.rig().weapons[0].aim_pivot.unwrap());
        Vec2::new(p.x, p.y)
    }
}

#[test]
fn a_held_trigger_walks_staples_into_the_target_and_costs_it_health() {
    let Some(f) = fixture() else { return };
    let mut shooter = Shooter::new(&f);
    let chest = shooter.ch.position() + Vec2::new(7.0, 1.5);
    // The target is a second Man on the enemy layer (9), standing still.
    let target = Character::new(f.bundle.animation.clone(), &f.bundle.rules, StandingColliders::MAN, chest - Vec2::new(0.0, 1.5));
    let mut target_body = target.body.clone();
    target_body.layer = 9;
    let mut health = LifeCycle::man().health;
    let start = health.remaining();

    let mut shots: Vec<Shot> = Vec::new();
    let mut fired = 0;
    let mut hits = 0;
    let mut first_hit_frame = None;
    for frame in 241..=241 + 60u64 {
        let aim = chest - shooter.pivot(&f);
        if let Some(shot) = shooter.frame(&f, frame, aim, true) {
            fired += 1;
            shots.push(shot);
        }
        // Projectile `FixedUpdate`s follow the character's.
        let own = &shooter.ch.body;
        shots.retain_mut(|shot| {
            let targets = [
                ShotTarget { position: target_body.position, shape: &target_body.shape, layer: target_body.layer },
                ShotTarget { position: own.position, shape: &own.shape, layer: own.layer },
            ];
            match shot.fixed_step(&f.bundle.world, &f.bundle.rules, &targets, DT) {
                Some(Hit::Target(0)) => {
                    health.damage(shot.damage);
                    hits += 1;
                    first_hit_frame.get_or_insert(frame);
                    false
                }
                Some(Hit::Target(_)) => panic!("a staple hit the man who fired it"),
                Some(Hit::Level(_)) => false,
                None => !shot.age_by(DT),
            }
        });
    }

    // 0.075 s between shots → one shot per 4.5 frames, plus the first.
    assert!((12..=14).contains(&fired), "one second of fire released {fired} staples");
    assert!(hits >= 10, "most staples reach a target 7 units away at 25 u/s ({hits} of {fired})");
    let first = first_hit_frame.unwrap();
    assert!(first <= 241 + 20, "first hit after {} frames", first - 241);
    assert!((start - health.remaining() - 2.5 * hits as f32).abs() < 1e-3, "each staple costs 2.5 health");
    assert!(health.remaining() < start);
}

#[test]
fn aiming_away_from_the_target_misses_it() {
    let Some(f) = fixture() else { return };
    let mut shooter = Shooter::new(&f);
    let chest = shooter.ch.position() + Vec2::new(7.0, 1.5);
    let target = Character::new(f.bundle.animation.clone(), &f.bundle.rules, StandingColliders::MAN, chest - Vec2::new(0.0, 1.5));
    let mut body = target.body.clone();
    body.layer = 9;
    // Straight up (well inside the near-vertical snap cone is fine, but face right first).
    let aim = Vec2::new(1.0, 6.0);
    let mut shot = (241..300u64).find_map(|frame| shooter.frame(&f, frame, aim, true)).expect("the rifle fires");
    for _ in 0..120 {
        let targets = [ShotTarget { position: body.position, shape: &body.shape, layer: body.layer }];
        assert_ne!(shot.fixed_step(&f.bundle.world, &f.bundle.rules, &targets, DT), Some(Hit::Target(0)));
    }
}

#[test]
fn the_staple_leaves_the_muzzle_along_the_raw_aim() {
    let Some(f) = fixture() else { return };
    let mut shooter = Shooter::new(&f);
    let aim = Vec2::new(5.0, 3.0);
    let shot = (241..300u64).find_map(|frame| shooter.frame(&f, frame, aim, true)).expect("the rifle fires");
    let want = aim / aim.magnitude();
    assert!((shot.velocity - want * 25.0).magnitude() < 1e-3, "velocity {:?}", shot.velocity);
    // It starts in front of the shoulder, a short way along the aim.
    let along = (shot.position - shooter.pivot(&f)).dot(want);
    assert!(along > 0.3 && along < 3.0, "muzzle {along} ahead of the pivot");
}

#[test]
fn a_released_trigger_stops_the_stream() {
    let Some(f) = fixture() else { return };
    let mut shooter = Shooter::new(&f);
    let aim = Vec2::new(1.0, 0.0);
    let mut shots = 0;
    for frame in 241..241 + 30u64 {
        shots += usize::from(shooter.frame(&f, frame, aim, frame < 241 + 10).is_some());
    }
    // Ten frames of fire at 0.075 s: the first shot immediately, then roughly every 4.5 frames.
    assert!((2..=3).contains(&shots), "{shots} shots in ten frames");
}
