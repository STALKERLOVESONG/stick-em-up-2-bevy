//! Computer-controlled characters: what an enemy decides each frame.
//!
//! Recovered from `EnemyCharacterInput`, the `NeverDontPlay.TinyTargeter` library (`Targeter`,
//! `Targetable`, its qualifiers and sorter), `FireAtFirstTarget`, `MoveToFirstTarget`,
//! `FireAndMoveIfNecessary`, `Duck` and `DeactivateOffScreen`.
//!
//! An enemy is a `Man` whose `CharacterInput` takes its values from an [`EnemyInput`] instead of
//! a device. The behaviours write into that processor during `Update`; `CharacterInput` (script
//! execution order −100) copies it into the character at the start of the *next* frame's
//! `Update`, so the controller always acts on the previous frame's decision.

use unity_compat::mathf::{self, sign};
use unity_compat::{Vec2, Vec3};

use unity_data::level::{EnemySpawn, TargetQualifier};

use crate::camera::ViewBounds;
use crate::character::CharacterInput;

// ---------------------------------------------------------------------------------------------
// The processor
// ---------------------------------------------------------------------------------------------

/// `EnemyCharacterInput`: the values the behaviours set, read by `CharacterInput` each frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EnemyInput {
    pub movement: f32,
    pub jump: bool,
    pub duck: bool,
    pub fire: bool,
    /// `RelativeAim`: null until the first shot is aimed, then the last aim (it is never cleared).
    pub relative_aim: Option<Vec2>,
    /// `SelectedWeapon` (set by `SetEnemyWeapon.Awake`), as an index into the rig's weapons.
    pub selected_weapon: Option<usize>,
}

impl EnemyInput {
    /// `CharacterInput.OwnerOnlyUpdate` with this processor: what the character acts on until the
    /// next copy. A missing aim is `CharacterInput.DefaultAim` (right); weapons never step.
    pub fn character_input(&self) -> CharacterInput {
        CharacterInput {
            movement: self.movement,
            jump: self.jump,
            duck: self.duck,
            aim: self.relative_aim.unwrap_or(Vec2::new(1.0, 0.0)),
            fire: self.fire,
            next_weapon: false,
            previous_weapon: false,
            selected_weapon: self.selected_weapon,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Targeting
// ---------------------------------------------------------------------------------------------

/// A `Targetable` that is enabled: the `Target` child of a player's `Man` (enemies lose theirs
/// to `RemovePlayerOnlyParts`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub position: Vec3,
    /// `IsRootControlledTargetQualifier`: the avatar's `ControlSlot` has a controller.
    pub controlled: bool,
}

/// `NeverDontPlay.TinyTargeter.Target`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    /// Index into the candidate list.
    pub index: usize,
    pub position: Vec3,
    pub distance_sqr: f32,
}

/// `Targeter.Qualifier` subclasses, in the order the `qualifiers` array lists them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Qualifier {
    /// `MaximumDistanceTargetQualifier.maximumDistance`.
    MaximumDistance(f32),
    /// `LineOfSightTargetQualifier.obstructionLayers`.
    LineOfSight { obstruction_layers: u32 },
    /// `IsRootControlledTargetQualifier`.
    IsRootControlled,
}

/// `Targeter`: qualifiers filter the targetables, sorters order the survivors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Targeter {
    pub qualifiers: Vec<Qualifier>,
    /// One `DistanceTargetSorter` per entry of `sorters` (the only sorter that exists).
    pub distance_sorters: usize,
}

impl Targeter {
    /// The `Targeter` of a placed enemy (none: no qualifiers, no sorters). Qualifier classes this
    /// crate does not know are skipped and reported through `unknown`.
    pub fn of_enemy(spawn: &EnemySpawn, mut unknown: impl FnMut(&str)) -> Targeter {
        let Some(settings) = &spawn.targeter else { return Targeter::default() };
        let qualifiers = settings
            .qualifiers
            .iter()
            .filter_map(|q| match q {
                TargetQualifier::MaximumDistance(d) => Some(Qualifier::MaximumDistance(*d)),
                TargetQualifier::LineOfSight(mask) => Some(Qualifier::LineOfSight { obstruction_layers: *mask }),
                TargetQualifier::IsRootControlled => Some(Qualifier::IsRootControlled),
                TargetQualifier::Other(name) => {
                    unknown(name);
                    None
                }
            })
            .collect();
        Targeter { qualifiers, distance_sorters: settings.distance_sorters }
    }

    /// `Targeter.Targets`, evaluated from `origin` (the targeter's `transform.position`).
    /// `obstructed(from, to, layers)` is `Physics2D.Linecast`.
    ///
    /// The original caches this per frame (`Time.frameCount`); callers evaluate it once per frame.
    ///
    /// TODO(UNCERTAIN): `List.Sort` is not stable in general; equal distances are kept in
    /// registration order here (what Mono's insertion sort does for short lists).
    pub fn targets(&self, origin: Vec3, candidates: &[Candidate], obstructed: impl Fn(Vec2, Vec2, u32) -> bool) -> Vec<Target> {
        let mut targets = Vec::new();
        for (index, candidate) in candidates.iter().enumerate() {
            let position = candidate.position;
            let target = Target { index, position, distance_sqr: (position - origin).sqr_magnitude() };
            let qualified = self.qualifiers.iter().all(|q| match *q {
                // `maximumDistanceSqr` is squared once in `Start`.
                Qualifier::MaximumDistance(max) => target.distance_sqr <= max * max,
                Qualifier::LineOfSight { obstruction_layers } => {
                    !obstructed(Vec2::new(origin.x, origin.y), Vec2::new(position.x, position.y), obstruction_layers)
                }
                Qualifier::IsRootControlled => candidate.controlled,
            });
            if qualified {
                targets.push(target);
            }
        }
        for _ in 0..self.distance_sorters {
            targets.sort_by(|a, b| a.distance_sqr.partial_cmp(&b.distance_sqr).unwrap_or(std::cmp::Ordering::Equal));
        }
        targets
    }
}

// ---------------------------------------------------------------------------------------------
// Behaviours
// ---------------------------------------------------------------------------------------------

/// `FireAtFirstTarget.Status`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FireStatus {
    NoTarget,
    NoFiringSolution,
    Fired,
}

/// What `FireAtFirstTarget` reads from the equipped launcher's projectile prefab.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ballistics {
    /// `Projectile.propelled`: flies straight, so it is aimed straight at the target.
    pub propelled: bool,
    /// `Rigidbody2D.gravityScale` of the prefab.
    pub gravity_scale: f32,
    /// `Projectile.speed`.
    pub speed: f32,
}

/// `FireAtFirstTarget.Fire`. `origin` is the behaviour's `transform.position`; `projectile` is
/// the equipped weapon's projectile (none without a weapon); `gravity_y` is
/// `Physics2D.gravity.y`.
///
/// A gravity-bound projectile is aimed with the closed-form launch angle for its speed: of the
/// two solutions, the one closer to the straight line to the target.
// The negated comparisons are the original's: they decide what NaN does.
#[allow(clippy::neg_cmp_op_on_partial_ord)]
pub fn fire_at_first_target(input: &mut EnemyInput, projectile: Option<Ballistics>, first: Option<&Target>, origin: Vec3, gravity_y: f32) -> FireStatus {
    let Some(projectile) = projectile else {
        input.fire = false;
        return FireStatus::NoFiringSolution;
    };
    let Some(target) = first else {
        input.fire = false;
        return FireStatus::NoTarget;
    };
    let vector = target.position - origin;
    let num = if !projectile.propelled { projectile.gravity_scale } else { 0.0 };
    if num == 0.0 {
        input.relative_aim = Some(Vec2::new(vector.x, vector.y));
        input.fire = true;
        return FireStatus::Fired;
    }
    // Operation order as compiled from C# (left to right, all single precision).
    let speed = projectile.speed;
    let num2 = gravity_y * num / (2.0 * speed * speed);
    let num3 = num2 * vector.x * vector.x;
    let x = vector.x;
    let num4 = 0.0 - vector.y + num2 * vector.x * vector.x;
    let num5 = x * x - 4.0 * num3 * num4;
    // `!(num5 >= 0f)`: NaN has no solution either.
    if !(num5 >= 0.0) {
        input.fire = false;
        return FireStatus::NoFiringSolution;
    }
    let num6 = mathf::sqrt(num5);
    let num7 = 2.0 * num3;
    let mut num8 = mathf::atan((0.0 - x + num6) / num7);
    let mut num9 = mathf::atan((0.0 - x - num6) / num7);
    if vector.x < 0.0 {
        num8 += mathf::PI;
        num9 += mathf::PI;
    }
    let v2 = Vec2::new(mathf::cos(num8), mathf::sin(num8));
    let v3 = Vec2::new(mathf::cos(num9), mathf::sin(num9));
    let out_of_unit_box = |v: Vec2| v.x < -1.0 || v.x > 1.0 || v.y < -1.0 || v.y > 1.0;
    let (flag, flag2) = (out_of_unit_box(v2), out_of_unit_box(v3));
    let value = if flag && !flag2 {
        v3
    } else if flag2 && !flag {
        v2
    } else {
        // `vector.normalized` (Vector3), then implicitly a Vector2 for `Vector2.Dot`.
        let magnitude = mathf::sqrt(vector.x * vector.x + vector.y * vector.y + vector.z * vector.z);
        let normalized = if magnitude > 1e-5 { Vec2::new(vector.x / magnitude, vector.y / magnitude) } else { Vec2::ZERO };
        let num10 = mathf::acos(v2.dot(normalized)).abs();
        let num11 = mathf::acos(v3.dot(normalized)).abs();
        if !(num10 < num11) { v3 } else { v2 }
    };
    input.relative_aim = Some(value);
    input.fire = true;
    FireStatus::Fired
}

/// `MoveToFirstTarget.Move`: walk until the target is about 5 units away horizontally (within a
/// 1.5 dead band), and jump while it is more than 10 units higher.
pub fn move_to_first_target(input: &mut EnemyInput, first: Option<&Target>, origin: Vec3) {
    let Some(target) = first else {
        input.movement = 0.0;
        input.jump = false;
        return;
    };
    let vector = target.position - origin;
    let f = vector.x - sign(vector.x) * 5.0;
    input.movement = if f.abs() <= 1.5 { 0.0 } else { sign(f) };
    input.jump = vector.y > 10.0;
}

/// The `SubsumableBehavior`s an enemy runs every `Update`, and its `Duck`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Brain {
    /// `FireAtFirstTarget` (enabled).
    pub fire_at_first_target: bool,
    /// `MoveToFirstTarget` (enabled).
    pub move_to_first_target: bool,
    /// `FireAndMoveIfNecessary`: subsumes the two above in its `Start` and drives them itself.
    pub fire_and_move_if_necessary: bool,
    /// `Duck`: `OnEnable` sets the processor's `Duck`, `OnDisable` clears it, so it is held
    /// whenever the enemy is active.
    pub duck: bool,
}

impl Brain {
    /// The behaviours of a placed enemy.
    pub fn of_enemy(spawn: &EnemySpawn) -> Brain {
        Brain {
            fire_at_first_target: spawn.fire_at_first_target,
            move_to_first_target: spawn.move_to_first_target,
            fire_and_move_if_necessary: spawn.fire_and_move_if_necessary,
            duck: spawn.duck,
        }
    }

    /// One frame of the enemy's `Update` behaviours. `targets` is this frame's `Targeter.Targets`.
    ///
    /// TODO(UNCERTAIN): the relative `Update` order of the behaviours on one object; they write
    /// disjoint processor fields, so it does not change the result.
    pub fn think(&self, input: &mut EnemyInput, targets: &[Target], projectile: Option<Ballistics>, origin: Vec3, gravity_y: f32) {
        let first = targets.first();
        if self.fire_and_move_if_necessary {
            // `FireAndMoveIfNecessary.UnsubsumedUpdate`.
            if fire_at_first_target(input, projectile, first, origin, gravity_y) != FireStatus::Fired {
                move_to_first_target(input, first, origin);
            } else {
                input.movement = 0.0;
                input.jump = false;
            }
            return;
        }
        if self.fire_at_first_target {
            fire_at_first_target(input, projectile, first, origin, gravity_y);
        }
        if self.move_to_first_target {
            move_to_first_target(input, first, origin);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Emerging
// ---------------------------------------------------------------------------------------------

/// `Emerge.characterControl.z` while hidden and once out.
pub const EMERGE_HIDDEN_Z: f32 = 0.0;
pub const EMERGE_DONE_Z: f32 = -0.1;
/// The layer an emerging enemy's tree is on until it is out (`Player.SetLayer(root, 25)`).
pub const EMERGE_LAYER: u8 = 25;

/// Where an `Emerge` coroutine is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EmergePhase {
    /// `while (!activationTrigger.Triggered) yield return null`.
    Waiting,
    /// Walking towards `target_x`, aiming (`fireAtFirstTarget.Fire()` then `input.Fire = false`).
    Moving { target_x: f32, going_right: bool },
    /// `yield return postEmergeWait`.
    Settling { remaining: f32 },
    Done,
}

/// What [`Emergence::update`] tells the game to do with the character.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EmergeEvent {
    None,
    /// Out: `characterControl.z = -0.1` and the root's original layer again.
    Emerged,
    /// The behaviours it held back run again.
    Completed,
}

/// `Emerge` (started in `Start`): the enemy's `FireAtFirstTarget`, the behaviours in
/// `unsubsumeOnCompletion` and those in `enableOnCompletion` are held back until it has walked
/// `point_x` out from behind the scenery.
#[derive(Clone, Debug, PartialEq)]
pub struct Emergence {
    pub point_x: f32,
    pub post_wait: f32,
    /// `FireAndMoveIfNecessary` is in `unsubsumeOnCompletion`.
    pub holds_fire_and_move: bool,
    /// `MoveToFirstTarget` is in `unsubsumeOnCompletion`.
    pub holds_move: bool,
    /// `Duck` is in `enableOnCompletion` (disabled until done, which clears the processor's duck).
    pub disables_duck: bool,
    pub phase: EmergePhase,
}

impl Emergence {
    pub fn of(spec: &unity_data::level::EmergeSpec) -> Emergence {
        let listed = |list: &[String], class_name: &str| list.iter().any(|c| c == class_name);
        Emergence {
            point_x: spec.point_x,
            post_wait: spec.post_emerge_wait_time,
            holds_fire_and_move: listed(&spec.unsubsume_on_completion, "FireAndMoveIfNecessary"),
            holds_move: listed(&spec.unsubsume_on_completion, "MoveToFirstTarget"),
            disables_duck: listed(&spec.enable_on_completion, "Duck"),
            phase: EmergePhase::Waiting,
        }
    }

    pub fn is_done(&self) -> bool {
        self.phase == EmergePhase::Done
    }

    /// The behaviours that run their own `Update` while emerging: `FireAtFirstTarget` is held,
    /// the listed ones too, and `MoveToFirstTarget` stays subsumed by a `FireAndMoveIfNecessary`.
    pub fn restricted(&self, brain: &Brain) -> Brain {
        if self.is_done() {
            return *brain;
        }
        Brain {
            fire_at_first_target: false,
            move_to_first_target: brain.move_to_first_target && !brain.fire_and_move_if_necessary && !self.holds_move,
            fire_and_move_if_necessary: brain.fire_and_move_if_necessary && !self.holds_fire_and_move,
            duck: brain.duck && !self.disables_duck,
        }
    }

    /// One step of the coroutine (after the frame's `Update`s): `triggered` is the activation
    /// trigger's state, `controller_x` the controller object's x; `aim` is
    /// `fireAtFirstTarget.Fire()`.
    pub fn update(&mut self, input: &mut EnemyInput, triggered: bool, controller_x: f32, delta: f32, aim: impl FnOnce(&mut EnemyInput)) -> EmergeEvent {
        if self.phase == EmergePhase::Waiting {
            if !triggered {
                return EmergeEvent::None;
            }
            input.movement = sign(self.point_x);
            self.phase = EmergePhase::Moving { target_x: controller_x + self.point_x, going_right: input.movement >= 0.0 };
        }
        match self.phase {
            EmergePhase::Moving { target_x, going_right } => {
                let delta_x = target_x - controller_x;
                if going_right ^ (delta_x >= 0.0) {
                    input.movement = 0.0;
                    self.phase = EmergePhase::Settling { remaining: self.post_wait };
                    return EmergeEvent::Emerged;
                }
                aim(input);
                input.fire = false;
                EmergeEvent::None
            }
            EmergePhase::Settling { remaining } => {
                let remaining = remaining - delta;
                if remaining <= 0.0 {
                    self.phase = EmergePhase::Done;
                    if self.disables_duck {
                        input.duck = true;
                    }
                    EmergeEvent::Completed
                } else {
                    self.phase = EmergePhase::Settling { remaining };
                    EmergeEvent::None
                }
            }
            EmergePhase::Waiting | EmergePhase::Done => EmergeEvent::None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Off-screen deactivation
// ---------------------------------------------------------------------------------------------

/// `DeactivateOffScreen`: once the level is loaded everywhere (and ten more frames passed), the
/// object deactivates its root whenever it is further than `radius` outside the camera view.
/// `OffScreenReactivator` reactivates it once the position it was deactivated at comes within
/// `radius` of the view again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OffScreenSleep {
    pub radius: f32,
    /// −1 until `OnLoadedLevelEverywhere`, which sets 10.
    frames_until_ready: i32,
}

/// `OffScreenSleep::on_loaded_level_everywhere` arms the check after this many frames.
pub const OFF_SCREEN_READY_FRAMES: i32 = 10;

impl OffScreenSleep {
    pub fn new(radius: f32) -> OffScreenSleep {
        OffScreenSleep { radius, frames_until_ready: -1 }
    }

    /// `OnLoadedLevelEverywhere`.
    pub fn on_loaded_level_everywhere(&mut self) {
        self.frames_until_ready = OFF_SCREEN_READY_FRAMES;
    }

    /// `DeactivateOffScreen.Update`: true when the root must be deactivated now (remember
    /// `position` for the reactivation test).
    pub fn update(&mut self, position: Vec2, view: &ViewBounds) -> bool {
        if self.frames_until_ready != 0 {
            if self.frames_until_ready > 0 {
                self.frames_until_ready -= 1;
            }
            return false;
        }
        !view.is_on_screen(position, self.radius)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emerging_enemy_waits_walks_out_then_fights() {
        let spec = unity_data::level::EmergeSpec {
            point_x: -3.0,
            trigger_object: None,
            post_emerge_wait_time: 0.4,
            unsubsume_on_completion: vec!["FireAndMoveIfNecessary".into()],
            enable_on_completion: vec![],
        };
        let mut e = Emergence::of(&spec);
        let brain = Brain { fire_at_first_target: true, move_to_first_target: true, fire_and_move_if_necessary: true, duck: false };
        let held = e.restricted(&brain);
        assert!(!held.fire_at_first_target && !held.fire_and_move_if_necessary && !held.move_to_first_target);
        let mut input = EnemyInput::default();
        assert_eq!(e.update(&mut input, false, 10.0, 0.1, |_| {}), EmergeEvent::None);
        assert_eq!(input.movement, 0.0);
        assert_eq!(e.update(&mut input, true, 10.0, 0.1, |i| i.fire = true), EmergeEvent::None);
        assert_eq!(input.movement, -1.0);
        assert!(!input.fire, "aims but never fires on the way out");
        assert_eq!(e.update(&mut input, true, 8.0, 0.1, |_| {}), EmergeEvent::None);
        assert_eq!(e.update(&mut input, true, 6.9, 0.1, |_| {}), EmergeEvent::Emerged);
        assert_eq!(input.movement, 0.0);
        // 0.4 s later (four or five 0.1 s frames, depending on rounding).
        let frames = (0..6).position(|_| e.update(&mut input, true, 6.9, 0.1, |_| {}) == EmergeEvent::Completed);
        assert!(matches!(frames, Some(3 | 4)), "{frames:?}");
        assert_eq!(e.restricted(&brain), brain);
    }

    fn target_at(x: f32, y: f32) -> Target {
        Target { index: 0, position: Vec3::new(x, y, -0.1), distance_sqr: x * x + y * y }
    }

    const ORIGIN: Vec3 = Vec3 { x: 0.0, y: 0.0, z: -0.1 };

    fn rocket() -> Ballistics {
        Ballistics { propelled: true, gravity_scale: 1.0, speed: 15.0 }
    }

    #[test]
    fn processor_defaults_aim_right_and_keeps_the_last_aim() {
        let mut input = EnemyInput::default();
        assert_eq!(input.character_input().aim, Vec2::new(1.0, 0.0), "`DefaultAim` before the first shot");
        fire_at_first_target(&mut input, Some(rocket()), Some(&target_at(-8.0, 3.0)), ORIGIN, -16.0);
        assert!(input.fire);
        assert_eq!(input.relative_aim, Some(Vec2::new(-8.0, 3.0)));
        // The target disappears: the trigger is released but the aim stays.
        assert_eq!(fire_at_first_target(&mut input, Some(rocket()), None, ORIGIN, -16.0), FireStatus::NoTarget);
        assert!(!input.fire);
        assert_eq!(input.character_input().aim, Vec2::new(-8.0, 3.0));
    }

    #[test]
    fn no_weapon_means_no_firing_solution_even_with_a_target() {
        let mut input = EnemyInput { fire: true, ..EnemyInput::default() };
        assert_eq!(fire_at_first_target(&mut input, None, Some(&target_at(5.0, 0.0)), ORIGIN, -16.0), FireStatus::NoFiringSolution);
        assert!(!input.fire);
    }

    #[test]
    fn lobbed_projectiles_follow_the_flatter_arc_that_reaches_the_target() {
        let gravity = -16.076;
        let lob = Ballistics { propelled: false, gravity_scale: 1.0, speed: 20.0 };
        for (x, y) in [(12.0, 0.0), (-12.0, 4.0), (8.0, -3.0)] {
            let mut input = EnemyInput::default();
            assert_eq!(fire_at_first_target(&mut input, Some(lob), Some(&target_at(x, y)), ORIGIN, gravity), FireStatus::Fired);
            let aim = input.relative_aim.unwrap();
            assert!((aim.magnitude() - 1.0).abs() < 1e-5, "unit direction");
            // Fly the parabola: it must pass through the target.
            let (vx, vy) = (aim.x * 20.0, aim.y * 20.0);
            let t = x / vx;
            assert!(t > 0.0, "fired towards the target");
            let height = vy * t + 0.5 * gravity * t * t;
            assert!((height - y).abs() < 1e-3, "({x}, {y}): reaches {height}");
            // The flatter of the two solutions: closer to the straight line.
            assert!(aim.y < 0.8, "({x}, {y}): {aim:?}");
        }
        // Out of range: no solution, trigger released.
        let mut input = EnemyInput { fire: true, ..EnemyInput::default() };
        assert_eq!(fire_at_first_target(&mut input, Some(lob), Some(&target_at(100.0, 0.0)), ORIGIN, gravity), FireStatus::NoFiringSolution);
        assert!(!input.fire);
    }

    #[test]
    fn moving_keeps_five_units_with_a_dead_band_and_jumps_to_high_targets() {
        let mut input = EnemyInput::default();
        let cases = [
            // (dx, dy) → (movement, jump)
            (20.0, 0.0, 1.0, false),
            (-20.0, 0.0, -1.0, false),
            (6.5, 0.0, 0.0, false),   // f = 1.5: inside the dead band
            (6.6, 0.0, 1.0, false),   // f = 1.6
            (3.0, 0.0, -1.0, false),  // too close: back off
            (0.0, 0.0, -1.0, false),  // Sign(0) = +1: f = -5
            (-3.0, 11.0, 1.0, true),  // f = 2, target high above
            (20.0, 10.0, 1.0, false), // exactly 10 higher does not jump
        ];
        for (dx, dy, movement, jump) in cases {
            move_to_first_target(&mut input, Some(&target_at(dx, dy)), ORIGIN);
            assert_eq!((input.movement, input.jump), (movement, jump), "({dx}, {dy})");
        }
        move_to_first_target(&mut input, None, ORIGIN);
        assert_eq!((input.movement, input.jump), (0.0, false));
    }

    #[test]
    fn fire_and_move_only_moves_when_it_cannot_shoot() {
        let brain = Brain { fire_at_first_target: true, move_to_first_target: true, fire_and_move_if_necessary: true, duck: false };
        let far = [target_at(30.0, 0.0)];
        let mut input = EnemyInput { movement: 1.0, jump: true, ..EnemyInput::default() };
        brain.think(&mut input, &far, Some(rocket()), ORIGIN, -16.0);
        assert!(input.fire);
        assert_eq!((input.movement, input.jump), (0.0, false), "firing stands still");
        // Without a weapon it closes in instead.
        brain.think(&mut input, &far, None, ORIGIN, -16.0);
        assert!(!input.fire);
        assert_eq!(input.movement, 1.0);
        // Nobody to fight: it stops.
        brain.think(&mut input, &[], Some(rocket()), ORIGIN, -16.0);
        assert_eq!((input.fire, input.movement), (false, 0.0));
    }

    #[test]
    fn targeter_filters_by_distance_and_sight_then_sorts_by_distance() {
        let targeter = Targeter {
            qualifiers: vec![Qualifier::MaximumDistance(100.0), Qualifier::LineOfSight { obstruction_layers: 1 }],
            distance_sorters: 1,
        };
        let candidates = [
            Candidate { position: Vec3::new(50.0, 0.0, -0.1), controlled: true },
            Candidate { position: Vec3::new(-20.0, 0.0, -0.1), controlled: true },
            Candidate { position: Vec3::new(0.0, 100.0, -0.1), controlled: true }, // exactly at range
            Candidate { position: Vec3::new(0.0, -100.5, -0.1), controlled: true }, // out of range
            Candidate { position: Vec3::new(10.0, 0.0, -0.1), controlled: true },  // behind a wall
        ];
        let wall_at_x_5 = |from: Vec2, to: Vec2, layers: u32| layers & 1 != 0 && (from.x - 5.0) * (to.x - 5.0) < 0.0;
        let targets = targeter.targets(ORIGIN, &candidates, wall_at_x_5);
        let order: Vec<usize> = targets.iter().map(|t| t.index).collect();
        assert_eq!(order, vec![1, 2]);
        // Candidate 0 is at x = 50, also behind the wall. Without the sight test it is in.
        let blind = Targeter { qualifiers: vec![Qualifier::MaximumDistance(100.0)], distance_sorters: 1 };
        let order: Vec<usize> = blind.targets(ORIGIN, &candidates, wall_at_x_5).iter().map(|t| t.index).collect();
        assert_eq!(order, vec![4, 1, 0, 2]);
        // No sorter: registration order.
        let unsorted = Targeter { qualifiers: vec![Qualifier::MaximumDistance(100.0)], distance_sorters: 0 };
        let order: Vec<usize> = unsorted.targets(ORIGIN, &candidates, wall_at_x_5).iter().map(|t| t.index).collect();
        assert_eq!(order, vec![0, 1, 2, 4]);
    }

    #[test]
    fn off_screen_check_waits_for_the_level_then_ten_frames() {
        let view = ViewBounds { center: Vec2::new(0.0, 0.0), extents: Vec2::new(10.0, 5.0) };
        let far = Vec2::new(100.0, 0.0);
        let mut sleep = OffScreenSleep::new(3.0);
        for _ in 0..50 {
            assert!(!sleep.update(far, &view), "never before the level is loaded everywhere");
        }
        sleep.on_loaded_level_everywhere();
        for frame in 0..OFF_SCREEN_READY_FRAMES {
            assert!(!sleep.update(far, &view), "frame {frame}");
        }
        assert!(sleep.update(far, &view));
        assert!(!sleep.update(Vec2::new(12.9, 0.0), &view), "within the radius of the view");
    }
}
