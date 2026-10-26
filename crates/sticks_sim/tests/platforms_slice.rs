//! First playable slice against the real exported `Platforms` level: spawn, fall, land, run, jump.
//! Skipped when the export is missing (`STICK_EXPORT_ROOT` = the `ExportedProject` directory).

mod common;

use std::path::PathBuf;

use sticks_sim::character::{Character, CharacterInput, StandingColliders};
use sticks_sim::physics::StepReport;
use sticks_sim::setup::load_level_bundle;
use sticks_sim::Vec2;

const DT: f32 = 1.0 / 60.0;

fn export_root() -> Option<PathBuf> {
    let root = common::export_root()?;
    root.join("Assets/Game").is_dir().then_some(root)
}

#[test]
fn player_spawns_lands_runs_and_jumps_on_the_real_platforms_level() {
    let Some(root) = export_root() else { return };
    let bundle = load_level_bundle(&root, "Assets/Game/Levels/Platforms/Platforms.unity").unwrap();
    let (level, rules, world) = (&bundle.level, &bundle.rules, &bundle.world);
    let spawn = &level.spawn_points[0];
    let mut ch = Character::new(bundle.animation.clone(), rules, StandingColliders::MAN, Vec2::new(spawn.position[0], spawn.position[1]));
    let mut frame = 0u64;
    let mut last = StepReport::default();
    let mut step = |ch: &mut Character, input: CharacterInput| {
        frame += 1;
        last = ch.fixed_update(&input, world, rules, DT, frame);
        ch.frame_update(&input, DT);
    };

    for _ in 0..300 {
        step(&mut ch, CharacterInput::default());
    }
    assert!(ch.is_grounded(), "settled at {:?}, spawn {:?}", ch.position(), spawn.position);
    let landed = ch.position();
    assert!((landed.y - spawn.position[1]).abs() < 6.0, "landed {landed:?} from spawn {:?}", spawn.position);
    println!("spawn {:?} -> landed {:?}", spawn.position, landed);

    // Run right for 2 s, then jump while running.
    let run = CharacterInput { movement: 1.0, ..CharacterInput::default() };
    for _ in 0..120 {
        step(&mut ch, run);
    }
    let after_run = ch.position();
    assert!(after_run.x > landed.x + 5.0 || ch.body.velocity.x.abs() < 1.0, "ran or hit a wall: {after_run:?}");
    println!("after run {after_run:?} vx {}", ch.body.velocity.x);
    let jump = CharacterInput { movement: 1.0, jump: true, ..CharacterInput::default() };
    let mut peak = after_run.y;
    for _ in 0..60 {
        step(&mut ch, jump);
        peak = peak.max(ch.position().y);
    }
    assert!(peak > after_run.y + 3.0, "jumped: peak {peak} from {}", after_run.y);
    assert!(ch.position().y > -1000.0, "never fell out of the world");
}
