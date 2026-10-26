//! The Abyss on the real `Platforms` level drives the recovered death cycle end to end:
//! trigger enter → `Health.Damage(float.MaxValue)` → life lost → explosion → campaign respawn at
//! the last-but-one safe standing position. Skipped when the export is missing.

mod common;

use sticks_sim::character::{Character, CharacterInput, StandingColliders};
use sticks_sim::life::{LifeCycle, LifeEvent, Lives, RespawnEnv, Score, Stage};
use sticks_sim::physics::StepReport;
use sticks_sim::rng::GameRng;
use sticks_sim::setup::{LevelBundle, load_level_bundle};
use sticks_sim::Vec2;
use unity_data::level::Shape;

const DT: f32 = 1.0 / 60.0;

fn bundle() -> Option<LevelBundle> {
    let root = common::export_root()?;
    root.join("Assets/Game").is_dir().then_some(())?;
    load_level_bundle(&root, "Assets/Game/Levels/Platforms/Platforms.unity").ok()
}

fn abyss_centre(bundle: &LevelBundle) -> (u32, Vec2) {
    let (id, c) = bundle
        .level
        .colliders
        .iter()
        .enumerate()
        .find(|(_, c)| c.trigger && c.has_behaviour("DamageOtherOnTriggerEnter"))
        .expect("the level has a DamageOtherOnTriggerEnter trigger (the Abyss)");
    let Shape::Polygon(points) = &c.shape else { panic!("abyss is a polygon, got {:?}", c.shape) };
    let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
    for p in points {
        for i in 0..2 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    (id as u32, Vec2::new((min[0] + max[0]) * 0.5, (min[1] + max[1]) * 0.5))
}

#[test]
fn falling_into_the_abyss_kills_then_respawns_at_the_last_safe_spot() {
    let Some(bundle) = bundle() else { return };
    let (rules, world) = (&bundle.rules, &bundle.world);
    let spawn = &bundle.level.spawn_points[0];
    let mut ch = Character::new(bundle.animation.clone(), rules, StandingColliders::MAN, Vec2::new(spawn.position[0], spawn.position[1]));
    let mut frame = 0u64;
    let input = CharacterInput::default();
    let mut step = |ch: &mut Character| -> StepReport {
        frame += 1;
        let r = ch.fixed_update(&input, world, rules, DT, frame);
        ch.frame_update(&input, DT);
        r
    };

    // Stand on the platform so safe positions are recorded.
    for _ in 0..300 {
        step(&mut ch);
    }
    assert!(ch.is_grounded());
    let safe = ch.last_last_safe_position();
    assert!((safe - ch.position()).magnitude() < 0.1, "safe {safe:?} vs standing {:?}", ch.position());

    // Controller-side state (the `Player` object) and the avatar's death cycle.
    let mut lives = Lives::new(2, 0);
    let mut score = Score::default();
    let mut life = LifeCycle::man();
    life.health.link_to_lives();
    life.explosion_prepared();
    let mut rng = GameRng::from_seed(11);
    let spawn_count = bundle.level.spawn_points.len();
    let (abyss_id, abyss_at) = abyss_centre(&bundle);

    // Drop the avatar into the Abyss; the first step that overlaps it reports the enter.
    ch.body.position = abyss_at;
    ch.body.velocity = Vec2::ZERO;
    let report = step(&mut ch);
    assert!(report.entered_triggers.contains(&abyss_id), "abyss {abyss_id} not entered: {:?}", report.entered_triggers);

    // `DamageOtherOnTriggerEnter.OnTriggerEnter2D`.
    let was_exhausted = life.health.is_exhausted();
    let mut env = RespawnEnv { campaign: true, versus_ended: false, spawn_point_count: spawn_count, time_delta: DT, rng: &mut rng };
    let events = life.damage(f32::MAX, Some(&mut lives), &mut env);
    if life.health.is_exhausted() && !was_exhausted {
        score.self_fragged(0);
    }
    assert!(events.contains(&LifeEvent::Exploded));
    assert_eq!(lives.remaining(), 1);
    assert_eq!(score.frags(), -1);
    life.late_update();
    assert!(!life.avatar_active());

    // The explosion runs; nothing else touches the character while it is inactive.
    let mut respawned = false;
    for _ in 0..400 {
        let mut env = RespawnEnv { campaign: true, versus_ended: false, spawn_point_count: spawn_count, time_delta: DT, rng: &mut rng };
        let events = life.tick(DT, &mut env);
        life.late_update();
        if events.iter().any(|e| matches!(e, LifeEvent::Respawned { .. })) {
            ch.respawn_at_last_safe_position();
            respawned = true;
            break;
        }
    }
    assert!(respawned);
    assert_eq!(life.stage(), Stage::Alive);
    assert_eq!(ch.position(), safe);
    assert!(ch.is_grounded());

    // The respawned avatar stands there again.
    for _ in 0..120 {
        step(&mut ch);
    }
    assert!((ch.position() - safe).magnitude() < 0.5, "drifted to {:?} from {safe:?}", ch.position());
    assert!(ch.is_grounded());
}
