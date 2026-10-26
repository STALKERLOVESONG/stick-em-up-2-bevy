//! Helicopters: `HelicopterControl` (velocity steering and roll), the enemy controller
//! (`HelicopterMoveToFirstTargetHeight`, `HelicopterFireAtFirstTarget` through
//! `EnemyHelicopterInput`), `HelicopterGunnerControl` (guns turned to the aim) and the rotors'
//! blade lines (`HelicopterTopRotor` / `HelicopterBackRotor`).

use unity_compat::{Vec2, Vec3, mathf};

use crate::rigid_body::RigidBody;

/// `HelicopterControl.maxVelocity`.
pub const MAX_VELOCITY: Vec2 = Vec2 { x: 16.0, y: 8.0 };
/// `HelicopterControl.maxAcceleration`.
pub const MAX_ACCELERATION: Vec2 = Vec2 { x: 10.0, y: 10.0 };
/// `HelicopterControl.maxAngle` = 72 / π (degrees of roll at full sideways input).
pub const MAX_ANGLE: f32 = 72.0 / std::f32::consts::PI;
/// `HelicopterControl.smoothRotationTime`.
pub const SMOOTH_ROTATION_TIME: f32 = 0.45;
/// `HelicopterMoveToFirstTargetHeight`: the climb aims to close the height gap in 3.5 s.
pub const HEIGHT_APPROACH_TIME: f32 = 3.5;

/// `HelicopterControl`'s state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HelicopterControl {
    /// `movementControl`: the last input movement (kept while nobody controls it).
    pub movement_control: Vec2,
}

impl HelicopterControl {
    /// `HelicopterControl.FixedUpdate` (before the physics step): steer the velocity towards
    /// `movement × maxVelocity` by at most `maxAcceleration × dt` per axis, and set the angular
    /// velocity that `SmoothDampAngle` would use to roll towards `-movement.x × maxAngle`.
    /// `input` is `None` while nobody (and no AI) controls the helicopter.
    pub fn fixed_update(&mut self, body: &mut RigidBody, input: Option<Vec2>, dt: f32) {
        if let Some(movement) = input {
            self.movement_control = movement;
        }
        let m = self.movement_control;
        let wanted = Vec2::new(m.x * MAX_VELOCITY.x, m.y * MAX_VELOCITY.y);
        let change = wanted - body.linear_velocity;
        let limit = Vec2::new(MAX_ACCELERATION.x * dt, MAX_ACCELERATION.y * dt);
        let step = Vec2::new(mathf::clamp(change.x, 0.0 - limit.x, limit.x), mathf::clamp(change.y, 0.0 - limit.y, limit.y));
        body.linear_velocity = body.linear_velocity + step;
        let target = m.x * MAX_ANGLE * -1.0;
        let mut velocity = body.angular_velocity_degrees();
        // The smoothed angle itself is thrown away; only the velocity is kept.
        let _ = mathf::smooth_damp_angle(body.rotation_degrees(), target, &mut velocity, SMOOTH_ROTATION_TIME, f32::INFINITY, dt);
        body.set_angular_velocity_degrees(velocity);
    }

    /// `HelicopterControl.OnRespawned`.
    pub fn on_respawned(&mut self) {
        self.movement_control = Vec2::ZERO;
    }
}

/// `HelicopterMoveToFirstTargetHeight.Update`: the vertical input that brings the controller
/// (`position`, the "Enemy" child) to the target's height; no target → vertical input 0.
/// The horizontal input is left as it is.
pub fn move_to_first_target_height(movement: Vec2, position: Vec2, target_y: Option<f32>, body_velocity: Vec2) -> Vec2 {
    let Some(target_y) = target_y else {
        return Vec2::new(movement.x, 0.0);
    };
    let wanted = Vec2::new(position.x, target_y);
    let toward = (wanted - position) / HEIGHT_APPROACH_TIME;
    let mut v = toward - body_velocity;
    let sqr = v.x * v.x + v.y * v.y;
    if sqr > 1.0 {
        v = v / mathf::sqrt(sqr);
    }
    Vec2::new(movement.x, v.y)
}

/// `HelicopterGunnerControl.Update`: the gun's z angle for an aim relative to its position.
pub fn gun_angle_degrees(relative_aim: Vec2) -> f32 {
    relative_aim.y.atan2(relative_aim.x) * 57.295_78
}

/// The blade lines of a rotor this frame, in the rotor's local space (`a`, `b`, foreground).
pub struct RotorBlade {
    pub a: Vec3,
    pub b: Vec3,
}

/// `HelicopterBackRotor.Update`: two blades through the hub, `angle` advanced by
/// `angularVelocity` (degrees per second) first.
pub fn back_rotor_blades(angle: &mut f32, angular_velocity: f32, radius: f32, delta_time: f32) -> Vec<RotorBlade> {
    *angle += angular_velocity * (std::f32::consts::PI / 180.0) * delta_time;
    let mut num = *angle;
    let mut out = Vec::with_capacity(2);
    for _ in 0..2 {
        let v = Vec3::new(radius * num.cos(), radius * num.sin(), 0.0);
        out.push(RotorBlade { a: v, b: Vec3::new(0.0 - v.x, 0.0 - v.y, 0.0 - v.z) });
        num += std::f32::consts::PI / 2.0;
    }
    out
}

/// `HelicopterTopRotor.Update`: four blades seen in perspective. Each tip
/// `radius × (cos, 0, sin)` is placed in the world (`to_world`), projected to the viewport and
/// back at the hub's depth (`reproject`), clamped to ±radius, and drawn from the hub: blades whose
/// tip is in front (`z < 0`) at local z −0.1 (foreground renderer), the others at 0. Returns
/// (offset from the hub in world x, y; foreground).
pub fn top_rotor_blades(
    angle: &mut f32,
    angular_velocity: f32,
    radius: f32,
    delta_time: f32,
    to_world: &dyn Fn(Vec3) -> Vec3,
    hub: Vec3,
    reproject: &dyn Fn(Vec3) -> Vec3,
) -> Vec<(Vec2, bool)> {
    *angle += angular_velocity * (std::f32::consts::PI / 180.0) * delta_time;
    let mut num = *angle;
    let mut out = Vec::with_capacity(4);
    for _ in 0..4 {
        let tip_local = Vec3::new(radius * num.cos(), 0.0, radius * num.sin());
        let tip = reproject(to_world(tip_local));
        let d = tip - hub;
        let d = Vec2::new(mathf::clamp(d.x, 0.0 - radius, radius), mathf::clamp(d.y, 0.0 - radius, radius));
        out.push((d, tip_local.z < 0.0));
        num += std::f32::consts::PI / 2.0;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_data::settings::Box2dTuning;

    #[test]
    fn steering_is_acceleration_limited_and_rolls_into_the_turn() {
        let mut body = RigidBody::new(Vec2::ZERO, 0.0, 1000.0, 0.0, 0.05, 0.0);
        let mut control = HelicopterControl::default();
        let dt = 0.02;
        control.fixed_update(&mut body, Some(Vec2::new(1.0, 0.0)), dt);
        // 10 units/s² for 0.02 s.
        assert!((body.linear_velocity.x - 0.2).abs() < 1e-6 && body.linear_velocity.y == 0.0);
        // Rolling towards -72/π degrees: a negative angular velocity.
        assert!(body.angular_velocity < 0.0);
        for _ in 0..500 {
            control.fixed_update(&mut body, None, dt);
            body.step(dt, Vec2::ZERO, &Box2dTuning::default());
        }
        // Full speed (16) and settled at the roll angle; the input was kept.
        assert!((body.linear_velocity.x - 16.0).abs() < 1e-3);
        assert!((body.rotation_degrees() + MAX_ANGLE).abs() < 0.05, "{}", body.rotation_degrees());
    }

    #[test]
    fn enemy_climbs_towards_the_target_height_with_unit_input_at_most() {
        let m = move_to_first_target_height(Vec2::new(0.3, 0.9), Vec2::new(10.0, 20.0), Some(13.0), Vec2::ZERO);
        assert_eq!(m.x, 0.3);
        assert!((m.y + 1.0).abs() < 1e-6, "7/3.5 = 2 → normalised to -1");
        let near = move_to_first_target_height(Vec2::ZERO, Vec2::new(0.0, 20.0), Some(19.0), Vec2::new(0.0, -0.1));
        assert!((near.y - (-1.0 / 3.5 + 0.1)).abs() < 1e-6);
        assert_eq!(move_to_first_target_height(Vec2::new(0.5, 0.5), Vec2::ZERO, None, Vec2::ZERO), Vec2::new(0.5, 0.0));
    }

    #[test]
    fn back_rotor_has_two_crossed_blades() {
        let mut angle = 0.0;
        let blades = back_rotor_blades(&mut angle, 90.0, 2.0, 1.0);
        assert!((angle - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
        assert!((blades[0].a.y - 2.0).abs() < 1e-5 && (blades[0].b.y + 2.0).abs() < 1e-5);
        assert!((blades[1].a.x + 2.0).abs() < 1e-5);
    }
}
