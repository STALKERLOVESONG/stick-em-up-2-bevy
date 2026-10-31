//! The playable stick-figure's movement rules (`CharacterControl2D`).
//!
//! Two entry points mirror the two places the original ran code, and the game schedules them in
//! Unity's order:
//!
//! * [`Character::fixed_update`] — once per physics step, *before* the physics step it feeds:
//!   resolves grounding, jump, slope following, air control, writes the body's velocity, then
//!   steps the body and digests the step's contacts and trigger overlaps.
//! * [`Character::frame_update`] — once per rendered frame: facing, animator parameters, the
//!   animator evaluation, and the `OnAnimatorMove` rule that turns root motion into ground
//!   velocity and resizes the colliders when ducking.
//!
//! Ground speed is *not* a constant: it is whatever the run animation's root motion yields
//! (≈ the designed 11.94 u/s at full input), scaled by the controller's 1.301065 constant.
//!
//! Not yet modelled (documented gaps, not guesses): moving platforms (`HorizontalScroller2D`),
//! network smoothing, vehicle control slots and the weapon aiming that shares this component.

use std::collections::BTreeSet;
use std::sync::Arc;

use unity_compat::mathf::{clamp, move_towards, sign};
use unity_compat::Vec2;

use crate::animator::{Animator, AnimatorModel, ParamId};
use crate::physics::{Body, BodyShape, BoxShape, CircleShape, PhysicsRules, StepReport, World};

/// `CharacterControl2D.maxHorizontalSpeed`.
pub const MAX_HORIZONTAL_SPEED: f32 = 11.940628;
/// `CharacterControl2D.airHorizontalAcceleration`.
pub const AIR_HORIZONTAL_ACCELERATION: f32 = 23.881256;
/// Controller-side scale applied to animator root motion.
pub const ROOT_MOTION_SCALE: f32 = 1.301065;
/// `duckSlideDeceleration`.
pub const DUCK_SLIDE_DECELERATION: f32 = 10.4;
/// Hard-coded `groundLayerMask` (layers 0 and 27).
pub const GROUND_LAYER_MASK: u32 = 134_217_729;
/// `Man.prefab` collision layer ("Player").
pub const PLAYER_LAYER: u8 = 8;
/// `CharacterControl2D.z`.
pub const DEFAULT_Z: f32 = -0.1;
/// Speed of the sampling arc used to follow slopes (25π/72 start, 25π/36 total sweep, 8 samples).
const SLOPE_SAMPLES: usize = 8;
const FACING_DEAD_ZONE: f32 = 0.03;

/// What drives a character this frame. The player's keyboard/mouse processor and the enemy AI
/// both fill the same struct, as in the original `CharacterInput` role.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterInput {
    /// -1..1 (keyboard walk modifier gives 0.2).
    pub movement: f32,
    pub jump: bool,
    pub duck: bool,
    /// Aim direction relative to the weapon pivot (`CharacterInput.RelativeAim`).
    pub aim: Vec2,
    /// `CharacterInput.Fire`: the trigger is held.
    pub fire: bool,
    /// `CharacterInput.NextWeapon` / `PreviousWeapon`: pressed this frame.
    pub next_weapon: bool,
    pub previous_weapon: bool,
    /// `CharacterInput.SelectedWeapon` as an index into the rig's weapons: the weapon to switch
    /// to when the loadout holds it (enemies, via `SetEnemyWeapon`); `None` for devices.
    pub selected_weapon: Option<usize>,
}

impl Default for CharacterInput {
    fn default() -> Self {
        Self { movement: 0.0, jump: false, duck: false, aim: Vec2::new(1.0, 0.0), fire: false, next_weapon: false, previous_weapon: false, selected_weapon: None }
    }
}

/// Animator parameter handles the controller reads and writes.
#[derive(Clone, Copy, Debug)]
struct ParamIds {
    forward_velocity: ParamId,
    airborne: ParamId,
    jump: ParamId,
    duck: ParamId,
    collider_height_fraction: ParamId,
}

/// Standing collider layout (recovered from `Man.prefab`: box offset (0, 1.5) size 1.15 × 1.8,
/// circles at y = 0.6 and y = 2.4 with radius 0.6).
#[derive(Clone, Copy, Debug)]
pub struct StandingColliders {
    pub box_offset_y: f32,
    pub box_size: Vec2,
    pub circle_radius: f32,
    pub bottom_circle_y: f32,
}

impl StandingColliders {
    pub const MAN: StandingColliders =
        StandingColliders { box_offset_y: 1.5, box_size: Vec2 { x: 1.15, y: 1.8 }, circle_radius: 0.6, bottom_circle_y: 0.6 };
}

#[derive(Clone, Debug)]
pub struct Character {
    pub body: Body,
    pub animator: Animator,
    ids: ParamIds,
    standing: StandingColliders,

    jump_velocity: f32,
    jump_cancel_time: f32,
    jump_cancel_impulse: f32,
    jump_cancel_time_remaining: f32,

    /// The script-side velocity the controller integrates; the body's own velocity is rewritten
    /// from it every fixed step.
    velocity: Vec2,
    grounded: bool,
    manually_grounded_last_frame: bool,
    frames_since_jump: u32,
    ground_touches: BTreeSet<u32>,
    ground_trigger_touches: BTreeSet<u32>,
    /// Triggers overlapped after the previous step, for `OnTriggerEnter2D` edge detection.
    trigger_overlaps: BTreeSet<u32>,
    recently_exited_ground_trigger: Option<u32>,
    recently_exited_expires_on_frame: u64,
    wall_hit: bool,
    wall_hit_normal: Vec2,

    facing_right: bool,
    last_movement_direction: f32,

    collider_bottom_offset: Vec2,
    last_collider_bottom_position: Vec2,
    collider_bottom_position: Vec2,

    last_safe_position: Vec2,
    last_last_safe_position: Vec2,

    /// `networkTargetPosition`: where the owner last said the character is (characters another
    /// peer simulates); `None` offline and for the owner.
    network_target: Option<Vec2>,
    /// `networkAddedVelocity`: the pull towards the target added last step, taken out again
    /// before the next.
    network_added_velocity: Vec2,
}

impl Character {
    /// Spawn a character with its feet origin at `position`.
    ///
    /// # Panics
    /// If the animator model lacks the controller parameters the character relies on.
    pub fn new(model: Arc<AnimatorModel>, rules: &PhysicsRules, standing: StandingColliders, position: Vec2) -> Character {
        let param = |name: &str| model.param(name).unwrap_or_else(|| panic!("animator controller has no parameter {name:?}"));
        let ids = ParamIds {
            forward_velocity: param("Forward Velocity"),
            airborne: param("Airborne"),
            jump: param("Jump"),
            duck: param("Duck"),
            collider_height_fraction: param("Collider Height Fraction"),
        };
        let gravity_y = rules.gravity.y;
        let jump_velocity = 0.0 - gravity_y;
        let jump_cancel_time = (0.0 - jump_velocity / gravity_y) * 0.5;
        let jump_cancel_impulse = gravity_y * 0.5;
        let half = Vec2::new(standing.box_size.x * 0.5, standing.box_size.y * 0.5);
        let shape = BodyShape {
            circles: vec![
                CircleShape { offset: Vec2::new(0.0, standing.bottom_circle_y), radius: standing.circle_radius },
                CircleShape { offset: Vec2::new(0.0, standing.box_size.y + standing.circle_radius), radius: standing.circle_radius },
            ],
            boxes: vec![BoxShape { offset: Vec2::new(0.0, standing.box_offset_y), half_extents: half }], polygons: Vec::new()
        };
        let body = Body { position, velocity: Vec2::ZERO, gravity_scale: 1.0, layer: PLAYER_LAYER, shape };
        // box.offset - (0, box.size.y * 0.5) - (0, circle.radius) - (0, 0.005)
        let collider_bottom_offset = Vec2::new(0.0, standing.box_offset_y) - Vec2::new(0.0, standing.box_size.y * 0.5)
            - Vec2::new(0.0, standing.circle_radius)
            - Vec2::new(0.0, 0.005);
        Character {
            body,
            animator: Animator::new(model),
            ids,
            standing,
            jump_velocity,
            jump_cancel_time,
            jump_cancel_impulse,
            jump_cancel_time_remaining: 0.0,
            velocity: Vec2::ZERO,
            grounded: false,
            manually_grounded_last_frame: false,
            frames_since_jump: 0,
            ground_touches: BTreeSet::new(),
            ground_trigger_touches: BTreeSet::new(),
            trigger_overlaps: BTreeSet::new(),
            recently_exited_ground_trigger: None,
            recently_exited_expires_on_frame: 0,
            wall_hit: false,
            wall_hit_normal: Vec2::ZERO,
            facing_right: true,
            last_movement_direction: 0.0,
            collider_bottom_offset,
            last_collider_bottom_position: Vec2::ZERO,
            collider_bottom_position: Vec2::ZERO,
            // Unity's default `Vector3` field value: a character that dies before it has stood
            // on anything twice respawns at the world origin.
            last_safe_position: Vec2::ZERO,
            last_last_safe_position: Vec2::ZERO,
            network_target: None,
            network_added_velocity: Vec2::ZERO,
        }
    }

    /// `CharacterControl2D.OnSerializeNetworkView` on a peer that does not own the character:
    /// the owner's latest position, which the next fixed steps pull the character towards.
    pub fn set_network_target(&mut self, position: Vec2) {
        self.network_target = Some(position);
    }

    pub fn position(&self) -> Vec2 {
        self.body.position
    }

    pub fn is_grounded(&self) -> bool {
        self.grounded
    }

    pub fn facing_right(&self) -> bool {
        self.facing_right
    }

    /// Initial facing, from the spawn point's orientation.
    pub fn set_facing_right(&mut self, facing_right: bool) {
        self.facing_right = facing_right;
    }

    /// The previous-but-one safe standing position, where campaign respawns put the character.
    pub fn last_last_safe_position(&self) -> Vec2 {
        self.last_last_safe_position
    }

    /// `CharacterControl2D.OnRespawned` (campaign): the controller is reactivated at the
    /// last-but-one safe standing position, flagged as grounded for the next fixed step, and the
    /// animator is told it is no longer airborne.
    ///
    /// Deactivating the original object tore down its `Animator` and `Rigidbody2D`, so both are
    /// recreated here. TODO(UNCERTAIN): that the animator controller state and parameters reset
    /// on re-enable (Unity default `keepAnimatorControllerStateOnDisable = false`) and that the
    /// rigid body restarts at rest; the script-side `velocity` field is kept as the original
    /// kept it.
    pub fn respawn_at_last_safe_position(&mut self) {
        self.network_target = None;
        self.body.position = self.last_last_safe_position;
        self.body.velocity = Vec2::ZERO;
        self.animator.restart();
        self.grounded = true;
        self.manually_grounded_last_frame = true;
        self.ground_touches.clear();
        // A recreated body reports every overlapped trigger as an enter again.
        self.trigger_overlaps.clear();
        self.animator.set_bool(self.ids.airborne, false);
    }

    /// `Player.Update` after leaving a vehicle: `MoveAvatarTo` (x/y only) and
    /// `SetActive(true)`. Reactivation recreates the `Animator` and `Rigidbody2D` (as for a
    /// respawn) but, unlike `OnRespawned`, leaves the controller's own fields (`grounded`, the
    /// safe positions, the script-side velocity) as they were when the avatar was deactivated.
    /// TODO(UNCERTAIN): the same animator-reset and body-at-rest assumptions as
    /// [`Character::respawn_at_last_safe_position`].
    pub fn reactivate_at(&mut self, position: Vec2) {
        self.body.position = position;
        self.body.velocity = Vec2::ZERO;
        self.animator.restart();
        self.ground_touches.clear();
        self.trigger_overlaps.clear();
    }

    /// Versus respawn variant: the respawner has already moved the object to a spawn point.
    pub fn respawn_at(&mut self, position: Vec2) {
        self.respawn_at_last_safe_position();
        self.body.position = position;
    }

    /// Current box collider height (shrinks while ducking).
    pub fn collider_height(&self) -> f32 {
        self.body.shape.boxes[0].half_extents.y * 2.0
    }

    // -----------------------------------------------------------------------------------------
    // FixedUpdate
    // -----------------------------------------------------------------------------------------

    /// One physics step: the controller's `FixedUpdate`, then the body step it drives, then the
    /// collision / trigger callbacks of that step.
    pub fn fixed_update(
        &mut self,
        input: &CharacterInput,
        world: &World,
        rules: &PhysicsRules,
        delta_time: f32,
        frame_count: u64,
    ) -> StepReport {
        self.run_fixed_update(input, world, delta_time, frame_count);
        let mut report = self.body.step(world, rules, delta_time);
        report.entered_triggers = report.triggers.iter().copied().filter(|id| !self.trigger_overlaps.contains(id)).collect();
        report.exited_triggers = self.trigger_overlaps.iter().copied().filter(|id| !report.triggers.contains(id)).collect();
        self.trigger_overlaps = report.triggers.iter().copied().collect();
        self.digest_step(&report, world, frame_count);
        report
    }

    fn run_fixed_update(&mut self, input: &CharacterInput, world: &World, delta_time: f32, frame_count: u64) {
        let gravity = Vec2::new(0.0, self.jump_cancel_impulse * 2.0);
        if self.body.gravity_scale != 1.0 {
            self.body.gravity_scale = 1.0;
        }
        if self.network_target.is_some() {
            self.velocity -= self.network_added_velocity;
            self.network_added_velocity = Vec2::ZERO;
        }
        self.grounded = self.frames_since_jump > 1
            && (self.manually_grounded_last_frame || !self.ground_touches.is_empty() || !self.ground_trigger_touches.is_empty());
        self.manually_grounded_last_frame = false;

        if (self.grounded || self.wall_hit) && input.jump {
            if self.wall_hit {
                self.velocity.x = MAX_HORIZONTAL_SPEED * self.wall_hit_normal.x;
            }
            self.velocity.y = self.jump_velocity;
            self.jump_cancel_time_remaining = self.jump_cancel_time;
            self.frames_since_jump = 0;
            self.grounded = false;
        }
        self.wall_hit = false;

        if self.grounded && !input.jump {
            let y = self.velocity.y;
            if !self.ground_trigger_touches.is_empty() {
                self.velocity.y = 0.0;
                self.body.gravity_scale = 0.0;
            } else {
                self.velocity.y = self.body.velocity.y;
            }
            self.jump_cancel_time_remaining = 0.0;

            let distance = (self.velocity.x * delta_time).abs();
            if distance > 0.0 {
                self.follow_ground(world, distance, delta_time, y);
            }
        }

        if !self.grounded {
            if self.frames_since_jump > 0 {
                self.jump_cancel_time_remaining -= delta_time;
                if self.jump_cancel_time_remaining > 0.0 && !input.jump {
                    self.velocity.y += self.jump_cancel_impulse;
                    self.jump_cancel_time_remaining = 0.0;
                }
                self.velocity += gravity * delta_time;
                let target = movement_fraction_of_max_speed(input.movement) * MAX_HORIZONTAL_SPEED;
                let max_change = AIR_HORIZONTAL_ACCELERATION * delta_time;
                let diff = target - self.velocity.x;
                self.velocity.x += diff.abs().min(max_change) * sign(diff);
            }
        }

        // A character owned elsewhere moves a tenth of the way to the owner's position each step
        // (by velocity, so it still collides), or jumps there when more than 10 units off.
        if let Some(target) = self.network_target {
            let position = self.body.position;
            if (position - target).sqr_magnitude() <= 100.0 {
                let blended = position * 0.9 + target * 0.1;
                self.network_added_velocity = (blended - position) / delta_time;
                self.velocity += self.network_added_velocity;
            } else {
                self.body.position = target;
            }
        }
        self.body.velocity = self.velocity;
        self.ground_touches.clear();
        if let Some(&first) = self.ground_trigger_touches.iter().next() {
            self.exit_trigger(first, frame_count);
        }
        self.frames_since_jump += 1;
        self.last_collider_bottom_position = self.collider_bottom_position;
        self.collider_bottom_position = self.body.position + self.collider_bottom_offset;
        if self.recently_exited_ground_trigger.is_some() && frame_count >= self.recently_exited_expires_on_frame {
            self.recently_exited_ground_trigger = None;
        }
    }

    /// Sweep eight sample circles from steeply up to steeply down along the movement direction
    /// and steer the ground velocity at the first one that touches ground, so the character
    /// stays glued to slopes (and keeps pressing into flat ground).
    fn follow_ground(&mut self, world: &World, distance: f32, delta_time: f32, previous_y: f32) {
        let step = std::f32::consts::PI * 25.0 / 36.0 / SLOPE_SAMPLES as f32;
        let bottom_circle = self.body.shape.circles[0];
        let origin = self.body.position + bottom_circle.offset;
        let radius = bottom_circle.radius;
        let mut found = false;
        let mut target = Vec2::ZERO;
        let mut angle = std::f32::consts::PI * 25.0 / 72.0;
        for i in 0..SLOPE_SAMPLES {
            target = origin + Vec2::new(angle.cos() * sign(self.velocity.x), angle.sin()) * distance;
            if world.overlap_circle(target, radius, GROUND_LAYER_MASK).is_some() {
                if i != 0 || !self.ground_trigger_touches.is_empty() {
                    found = true;
                }
                break;
            }
            angle -= step;
        }
        if found {
            self.velocity = (target - origin) / delta_time;
            self.manually_grounded_last_frame = true;
        } else if !self.ground_trigger_touches.is_empty() {
            self.grounded = false;
            self.velocity.y = previous_y;
            self.body.gravity_scale = 1.0;
        }
    }

    /// `OnCollisionStay2D` / `OnTriggerStay2D` for everything the step touched.
    fn digest_step(&mut self, report: &StepReport, world: &World, frame_count: u64) {
        for contact in &report.contacts {
            let layer = contact.layer;
            if layer == 28 || (12..20).contains(&layer) {
                continue;
            }
            if contact.normal.y > 0.2 {
                self.ground_touches.insert(contact.collider);
                let standable = world.find(contact.collider).is_none_or(|c| !c.no_safe_standing);
                if !contact.unsafe_ground && standable && !world.in_unsafe_respawn_area(self.body.position) {
                    self.last_last_safe_position = self.last_safe_position;
                    self.last_safe_position = self.body.position;
                }
            }
            if contact.walljumpable && contact.normal.y.abs() < 0.1 {
                self.wall_hit = true;
                self.wall_hit_normal = contact.normal;
            }
        }
        for &id in &report.triggers {
            if world.find(id).is_some_and(|c| c.one_way) {
                self.one_way_trigger_stay(id, world, frame_count);
            }
        }
    }

    /// `OnTriggerEnter2D` / `OnTriggerStay2D` for a `One-way Platform`.
    fn one_way_trigger_stay(&mut self, id: u32, world: &World, _frame_count: u64) {
        if self.recently_exited_ground_trigger == Some(id) {
            self.ground_trigger_touches.insert(id);
            return;
        }
        if self.velocity.y >= 0.0 {
            return;
        }
        let hits = world.raycast_down(self.last_collider_bottom_position, GROUND_LAYER_MASK);
        // The original keeps only the first ten hits (`hits = new RaycastHit2D[10]`).
        if hits.iter().take(10).any(|&h| h == id) {
            self.ground_trigger_touches.insert(id);
        }
    }

    fn exit_trigger(&mut self, id: u32, frame_count: u64) {
        if self.ground_trigger_touches.remove(&id) {
            self.recently_exited_ground_trigger = Some(id);
            self.recently_exited_expires_on_frame = frame_count + 5;
        }
    }

    // -----------------------------------------------------------------------------------------
    // Update + animator
    // -----------------------------------------------------------------------------------------

    /// Per-frame step: `Update`, then the animator evaluation with its `OnAnimatorMove`.
    pub fn frame_update(&mut self, input: &CharacterInput, delta_time: f32) {
        // Update
        if input.aim.x.abs() > FACING_DEAD_ZONE {
            self.facing_right = input.aim.x >= 0.0;
        }
        let facing = if self.facing_right { 1.0 } else { -1.0 };
        self.animator.set_float(self.ids.forward_velocity, facing * input.movement);
        self.animator.set_bool(self.ids.jump, input.jump);
        self.animator.set_bool(self.ids.duck, input.duck);

        // Animator evaluation, then OnAnimatorMove.
        let step = self.animator.update(delta_time);
        self.on_animator_move(input, step.delta_position, delta_time);
    }

    fn on_animator_move(&mut self, input: &CharacterInput, local_delta: [f32; 3], delta_time: f32) {
        if input.movement != 0.0 {
            self.last_movement_direction = sign(input.movement);
        }
        let model = self.animator.model();
        let ducking_state = model.state_index("Ducking");
        let ducking = ducking_state.is_some_and(|d| self.animator.is_in_or_entering(d));
        let airborne =
            self.animator.current_tag() == "Airborne" || self.animator.next_tag().is_some_and(|t| t == "Airborne");
        if !ducking && self.grounded && !airborne {
            // Local +z is the facing direction (the model is turned ±90° about Y).
            let world_x = if self.facing_right { local_delta[2] } else { -local_delta[2] };
            let delta = Vec2::new(world_x, local_delta[1]);
            self.velocity = delta * ROOT_MOTION_SCALE / delta_time;
            // Not in the original: the root motion replaced the velocity, so last step's network
            // pull is no longer in it. The original still subtracted it in the next `FixedUpdate`,
            // which stalls a standing remote character at 1/11 of the way to its owner's
            // position; leaving it out lets the pull converge as intended.
            self.network_added_velocity = Vec2::ZERO;
            self.velocity.x = self.velocity.x.abs() * self.last_movement_direction;
        }
        if ducking {
            let x = self.velocity.x;
            self.velocity.x = sign(x) * move_towards(x.abs(), 0.0, DUCK_SLIDE_DECELERATION * delta_time);
        }
        let fraction = self.animator.float(self.ids.collider_height_fraction);
        let height = fraction * self.standing.box_size.y;
        if self.collider_height() != height {
            self.set_collider_height(height);
        }
        self.animator.set_bool(self.ids.airborne, !self.grounded);
    }

    /// Resize the box and lift the head circle with it (`boxCollider.size/offset`,
    /// `topCircleCollider.offset`).
    fn set_collider_height(&mut self, height: f32) {
        let standing_height = self.standing.box_size.y;
        let shape = &mut self.body.shape;
        shape.boxes[0].half_extents.y = height * 0.5;
        shape.boxes[0].offset = Vec2::new(0.0, self.standing.box_offset_y - (standing_height - height) * 0.5);
        shape.circles[1].offset = Vec2::new(0.0, height + self.standing.circle_radius);
    }
}

/// `MovementInputToFractionOfMaximumHorizontalSpeed`: any input already means half speed.
fn movement_fraction_of_max_speed(movement: f32) -> f32 {
    if movement == 0.0 {
        return 0.0;
    }
    let magnitude = clamp(movement.abs(), 0.0, f32::MAX);
    let fraction = magnitude / 1.0 * 0.5 + 0.5;
    fraction * sign(movement)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{StaticCollider, StaticShape};
    use std::path::PathBuf;

    const DT: f32 = 1.0 / 60.0;
    const G: f32 = -16.076;

    fn model() -> Option<Arc<AnimatorModel>> {
        let root = std::env::var_os("STICK_EXPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
        let data = unity_data::anim::load_character_animation(&root.join("Assets")).ok()?;
        Some(AnimatorModel::new(data))
    }

    fn rules() -> PhysicsRules {
        PhysicsRules { gravity: Vec2::new(0.0, G), layer_matrix: [u32::MAX; 32], raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 }
    }

    fn collider(id: u32, layer: u8) -> StaticCollider {
        StaticCollider { id, layer, ..Default::default() }
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> StaticShape {
        StaticShape::Polygon(vec![Vec2::new(x0, y0), Vec2::new(x1, y0), Vec2::new(x1, y1), Vec2::new(x0, y1)])
    }

    struct Sim {
        world: World,
        rules: PhysicsRules,
        ch: Character,
        frame: u64,
        last: StepReport,
    }

    impl Sim {
        fn new(items: Vec<(StaticCollider, StaticShape)>, at: Vec2) -> Option<Sim> {
            let rules = rules();
            let ch = Character::new(model()?, &rules, StandingColliders::MAN, at);
            Some(Sim { world: World::build(items), rules, ch, frame: 0, last: StepReport::default() })
        }

        /// One rendered frame under the fixed-delta synchronizer: one physics step, then Update.
        fn frame(&mut self, input: CharacterInput) {
            self.frame += 1;
            self.last = self.ch.fixed_update(&input, &self.world, &self.rules, DT, self.frame);
            self.ch.frame_update(&input, DT);
        }

        fn run(&mut self, frames: usize, input: CharacterInput) {
            for _ in 0..frames {
                self.frame(input);
            }
        }
    }

    fn flat() -> Vec<(StaticCollider, StaticShape)> {
        vec![(collider(1, 0), rect(-200.0, -10.0, 200.0, 0.0))]
    }

    fn right() -> CharacterInput {
        CharacterInput { movement: 1.0, ..CharacterInput::default() }
    }

    #[test]
    fn character_falls_lands_and_stands_still() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(0.0, 3.0)) else { return };
        s.run(120, CharacterInput::default());
        assert!(s.ch.is_grounded());
        assert!((s.ch.position().y + 0.005).abs() < 0.01, "y = {}", s.ch.position().y);
        assert!(s.ch.body.velocity.magnitude() < 0.05, "v = {:?}", s.ch.body.velocity);
        assert_eq!(s.ch.position().x, 0.0);
    }

    #[test]
    fn a_character_owned_elsewhere_follows_the_owners_position() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(0.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        // Close: pulled a tenth of the way each step, so it converges without teleporting.
        s.ch.set_network_target(Vec2::new(2.0, 0.0));
        s.run(1, CharacterInput::default());
        let after_one = s.ch.position().x;
        assert!(after_one > 0.05 && after_one < 0.5, "x = {after_one}");
        s.run(120, CharacterInput::default());
        assert!((s.ch.position().x - 2.0).abs() < 0.05, "x = {}", s.ch.position().x);
        // Far: it jumps there.
        s.ch.set_network_target(Vec2::new(40.0, 0.0));
        s.run(1, CharacterInput::default());
        assert!((s.ch.position().x - 40.0).abs() < 0.5, "x = {}", s.ch.position().x);
    }

    #[test]
    fn running_reaches_designed_top_speed_and_stays_on_the_ground() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(-150.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        let x0 = s.ch.position().x;
        s.run(60, right());
        // Instantaneous speed breathes with the stride; compare the mean over ~4 stride cycles.
        let mut sum = 0.0;
        for _ in 0..120 {
            s.frame(right());
            sum += s.ch.body.velocity.x;
        }
        let speed = sum / 120.0;
        // Designed 11.940628; root-motion derivation gives 11.985 (see animator tests).
        assert!((speed - MAX_HORIZONTAL_SPEED).abs() / MAX_HORIZONTAL_SPEED < 0.01, "speed {speed}");
        assert!(s.ch.is_grounded());
        assert!(s.ch.position().y.abs() < 0.02, "y = {}", s.ch.position().y);
        assert!(s.ch.position().x > x0 + 20.0);
    }

    #[test]
    fn running_left_mirrors_running_right() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(150.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        let input = CharacterInput { movement: -1.0, aim: Vec2::new(-1.0, 0.0), ..CharacterInput::default() };
        s.run(60, input);
        let mut sum = 0.0;
        for _ in 0..120 {
            s.frame(input);
            sum += s.ch.body.velocity.x;
        }
        assert!(!s.ch.facing_right());
        let speed = sum / 120.0;
        assert!((speed + MAX_HORIZONTAL_SPEED).abs() / MAX_HORIZONTAL_SPEED < 0.01, "{speed}");
    }

    #[test]
    fn jump_reaches_expected_height_and_lands() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(0.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        let hold = CharacterInput { jump: true, ..CharacterInput::default() };
        let mut apex = f32::MIN;
        let mut apex_frame = 0;
        for f in 0..120 {
            s.frame(hold);
            if s.ch.position().y > apex {
                apex = s.ch.position().y;
                apex_frame = f;
            }
        }
        // v0 = 16.076, g = 16.076 and Box2D adds one extra g·dt per step: apex ≈ 7.4–8.0.
        assert!(apex > 7.0 && apex < 8.1, "apex {apex}");
        assert!((50..65).contains(&apex_frame), "apex after {apex_frame} frames");
        s.run(120, CharacterInput::default());
        assert!(s.ch.is_grounded());
        assert!(s.ch.position().y.abs() < 0.02);
    }

    #[test]
    fn releasing_jump_early_cuts_the_jump_short() {
        let Some(mut a) = Sim::new(flat(), Vec2::ZERO) else { return };
        let Some(mut b) = Sim::new(flat(), Vec2::ZERO) else { return };
        a.run(60, CharacterInput::default());
        b.run(60, CharacterInput::default());
        let hold = CharacterInput { jump: true, ..CharacterInput::default() };
        let (mut apex_a, mut apex_b) = (0.0f32, 0.0f32);
        for f in 0..90 {
            a.frame(hold);
            // Tap: held for 2 frames only.
            b.frame(if f < 2 { hold } else { CharacterInput::default() });
            apex_a = apex_a.max(a.ch.position().y);
            apex_b = apex_b.max(b.ch.position().y);
        }
        assert!(apex_b < apex_a * 0.5, "tap {apex_b} vs hold {apex_a}");
        assert!(apex_b > 1.0, "tap still leaves the ground: {apex_b}");
    }

    #[test]
    fn air_control_accelerates_horizontally_at_the_designed_rate() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(0.0, 40.0)) else { return };
        s.run(3, CharacterInput::default());
        let before = s.ch.velocity.x;
        s.run(6, right());
        let gained = s.ch.velocity.x - before;
        let expected = AIR_HORIZONTAL_ACCELERATION * DT * 6.0;
        assert!((gained - expected).abs() < 1e-3, "{gained} vs {expected}");
        s.run(30, right());
        assert!((s.ch.velocity.x - MAX_HORIZONTAL_SPEED).abs() < 1e-3, "capped at max speed: {}", s.ch.velocity.x);
    }

    #[test]
    fn walljump_pushes_away_from_a_walljumpable_wall() {
        let mut wall = collider(2, 0);
        wall.walljumpable = true;
        let mut items = flat();
        items.push((wall, rect(5.0, 0.0, 6.0, 60.0)));
        let Some(mut s) = Sim::new(items, Vec2::new(0.0, 8.0)) else { return };
        // Fall (mid-air) pressing into the wall until touching it.
        let mut touching = false;
        for _ in 0..120 {
            s.frame(right());
            if s.ch.wall_hit {
                touching = true;
                break;
            }
        }
        assert!(touching, "never touched wall at {:?}", s.ch.position());
        assert!(s.ch.wall_hit_normal.x < -0.9);
        s.frame(CharacterInput { jump: true, movement: 1.0, ..CharacterInput::default() });
        assert!((s.ch.velocity.x + MAX_HORIZONTAL_SPEED).abs() < 0.8, "pushed left at ≈ -11.94: {}", s.ch.velocity.x);
        assert!(s.ch.velocity.y > 10.0);
    }

    #[test]
    fn non_walljumpable_wall_gives_no_walljump() {
        let mut items = flat();
        items.push((collider(2, 0), rect(5.0, 0.0, 6.0, 60.0)));
        let Some(mut s) = Sim::new(items, Vec2::new(0.0, 8.0)) else { return };
        for _ in 0..120 {
            s.frame(right());
        }
        assert!(!s.ch.wall_hit);
    }

    #[test]
    fn ducking_shrinks_colliders_and_decelerates_a_slide() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(-100.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        s.run(120, right());
        assert!((s.ch.collider_height() - 1.8).abs() < 1e-5);
        let duck = CharacterInput { duck: true, ..CharacterInput::default() };
        let v0 = s.ch.velocity.x;
        s.run(3, duck);
        assert!(s.ch.velocity.x < v0, "sliding slows while ducking");
        s.run(90, duck);
        let expected = 0.816_666_66 * 1.8;
        assert!((s.ch.collider_height() - expected).abs() < 1e-3, "{}", s.ch.collider_height());
        let head = s.ch.body.shape.circles[1].offset.y;
        assert!((head - (expected + 0.6)).abs() < 1e-3);
        assert!(s.ch.velocity.x.abs() < 0.01, "slide has stopped");
        // Standing back up restores the standing collider.
        s.run(120, CharacterInput::default());
        assert!((s.ch.collider_height() - 1.8).abs() < 1e-4);
        assert!((s.ch.body.shape.circles[1].offset.y - 2.4).abs() < 1e-4);
    }

    #[test]
    fn character_follows_an_upward_slope_without_leaving_the_ground() {
        // 20° ramp from x = 0 to x = 40.
        let slope = 20.0f32.to_radians();
        let top = Vec2::new(40.0, 40.0 * slope.tan());
        let ramp = StaticShape::Polygon(vec![
            Vec2::new(-200.0, -10.0),
            Vec2::new(200.0, -10.0),
            Vec2::new(200.0, top.y),
            top,
            Vec2::new(0.0, 0.0),
            Vec2::new(-200.0, 0.0),
        ]);
        let Some(mut s) = Sim::new(vec![(collider(1, 0), ramp)], Vec2::new(-20.0, 0.0)) else { return };
        s.run(60, CharacterInput::default());
        let mut airborne_frames = 0;
        for _ in 0..240 {
            s.frame(right());
            if s.ch.position().x > 2.0 && s.ch.position().x < 38.0 && !s.ch.is_grounded() {
                airborne_frames += 1;
            }
        }
        assert!(s.ch.position().y > 7.0, "climbed: {:?}", s.ch.position());
        assert!(airborne_frames <= 2, "left the ramp {airborne_frames} frames");
    }

    #[test]
    fn one_way_platform_catches_a_falling_character_but_not_a_rising_one() {
        let mut platform = collider(2, 27);
        platform.trigger = true;
        platform.one_way = true;
        // Thin trigger slab, top at y = 6.
        let mut items = flat();
        items.push((platform, rect(-10.0, 5.0, 10.0, 6.0)));
        let Some(mut s) = Sim::new(items, Vec2::new(0.0, 12.0)) else { return };
        s.run(180, CharacterInput::default());
        assert!(s.ch.is_grounded());
        assert!(s.ch.position().y > 4.0 && s.ch.position().y < 7.0, "stands on the platform: {:?}", s.ch.position());
        // Stand on ground below the platform and jump up through it.
        let Some(mut t) = Sim::new(
            {
                let mut p = collider(2, 27);
                p.trigger = true;
                p.one_way = true;
                let mut v = flat();
                v.push((p, rect(-10.0, 5.0, 10.0, 6.0)));
                v
            },
            Vec2::new(0.0, 0.0),
        ) else {
            return;
        };
        t.run(60, CharacterInput::default());
        let hold = CharacterInput { jump: true, ..CharacterInput::default() };
        let mut max_y = 0.0f32;
        for _ in 0..40 {
            t.frame(hold);
            max_y = max_y.max(t.ch.position().y);
        }
        assert!(max_y > 7.0, "passed through the platform: {max_y}");
    }

    #[test]
    fn safe_position_trails_by_one_ground_contact() {
        let Some(mut sim) = Sim::new(vec![(collider(1, 0), rect(-50.0, -10.0, 50.0, 0.0))], Vec2::new(0.0, 3.0)) else { return };
        // Never touched ground: the original's zero-initialised fields.
        assert_eq!(sim.ch.last_last_safe_position(), Vec2::ZERO);
        sim.run(120, CharacterInput::default());
        let standing = sim.ch.position();
        // Recorded at the end of a physics step; resting jitter is far below 1e-4.
        assert!((sim.ch.last_last_safe_position() - standing).magnitude() < 1e-4);
        // Run: last-last trails the current position by one step of ground contact.
        sim.run(30, CharacterInput { movement: 1.0, ..CharacterInput::default() });
        let behind = (sim.ch.position() - sim.ch.last_last_safe_position()).magnitude();
        assert!(behind > 0.0 && behind < 1.0, "{behind}");
    }

    #[test]
    fn unsafe_ground_projectiles_scrollers_and_unsafe_areas_never_become_safe() {
        for flag in 0..4 {
            let mut ground = collider(1, 0);
            match flag {
                0 => ground.unsafe_ground = true,
                1 => ground.no_safe_standing = true,
                2 => ground.unsafe_respawn_area = true,
                _ => {}
            }
            let mut items = vec![(collider(1, 0), rect(-50.0, -10.0, 50.0, 0.0))];
            if flag == 2 {
                // A trigger-less marker area covering the standing spot, on another collider.
                let mut area = collider(2, 0);
                area.unsafe_respawn_area = true;
                area.trigger = true;
                items.push((area, rect(-5.0, -1.0, 5.0, 5.0)));
            } else {
                items[0].0 = ground;
            }
            let Some(mut sim) = Sim::new(items, Vec2::new(0.0, 3.0)) else { return };
            sim.run(120, CharacterInput::default());
            let recorded = sim.ch.last_last_safe_position() != Vec2::ZERO;
            assert_eq!(recorded, flag == 3, "case {flag}");
        }
    }

    #[test]
    fn respawn_puts_the_character_back_grounded_and_restarts_the_animator() {
        let Some(mut sim) = Sim::new(vec![(collider(1, 0), rect(-50.0, -10.0, 50.0, 0.0))], Vec2::new(0.0, 3.0)) else { return };
        sim.run(120, CharacterInput::default());
        let safe = sim.ch.last_last_safe_position();
        // Fall off into the void, airborne.
        sim.ch.body.position = Vec2::new(0.0, -200.0);
        sim.ch.body.velocity = Vec2::new(0.0, -40.0);
        sim.run(10, CharacterInput::default());
        sim.ch.respawn_at_last_safe_position();
        assert_eq!(sim.ch.position(), safe);
        assert_eq!(sim.ch.body.velocity, Vec2::ZERO);
        assert!(sim.ch.is_grounded());
        assert_eq!(sim.ch.animator.current_state(), sim.ch.animator.model().default_state());
        sim.run(60, CharacterInput::default());
        assert!(sim.ch.is_grounded());
        assert!((sim.ch.position() - safe).magnitude() < 0.1);
    }

    #[test]
    fn trigger_enter_is_reported_once_per_overlap() {
        let mut zone = collider(2, 0);
        zone.trigger = true;
        let items = vec![(collider(1, 0), rect(-50.0, -10.0, 50.0, 0.0)), (zone, rect(10.0, -5.0, 20.0, 10.0))];
        let Some(mut sim) = Sim::new(items, Vec2::new(0.0, 3.0)) else { return };
        let mut enters = Vec::new();
        for _ in 0..240 {
            sim.frame(CharacterInput { movement: 1.0, ..CharacterInput::default() });
            enters.extend(sim.last.entered_triggers.iter().copied());
        }
        assert_eq!(enters, vec![2], "entered exactly once while running through the zone");
    }

    #[test]
    fn spawn_in_air_matches_the_two_frame_startup_of_the_original() {
        let Some(mut s) = Sim::new(flat(), Vec2::new(0.0, 5.0)) else { return };
        s.frame(CharacterInput::default());
        // First step: framesSinceJump == 0 → script velocity untouched (0); only Box2D gravity acts.
        assert_eq!(s.ch.velocity, Vec2::ZERO);
        s.frame(CharacterInput::default());
        assert!((s.ch.velocity.y - G * DT).abs() < 1e-6, "{}", s.ch.velocity.y);
    }
}
