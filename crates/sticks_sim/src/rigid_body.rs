//! Dynamic `Rigidbody2D`s, ported from Box2D 2.3 (`b2Body`, `b2Island::Solve`), the engine under
//! Unity 5.1's Physics2D. Box2D works in radians; Unity's API (`rotation`, `angularVelocity`)
//! converts to and from degrees, which is reproduced at the API boundary.
//!
//! [`RigidBody::integrate_velocities`] and [`RigidBody::integrate_positions`] are the two halves
//! of `b2Island::Solve` around the contact solver (`crate::contact`):
//!
//! ```text
//! v += h * (gravityScale * gravity + invMass * force); w += h * invI * torque
//! v *= 1 / (1 + h * linearDamping);  w *= 1 / (1 + h * angularDamping)
//! ... velocity constraints ...
//! translation = h * v, clamped to maxTranslation; rotation = h * w, clamped to maxRotation
//! c += h * v; a += h * w
//! ... position constraints ...
//! ```
//!
//! TODO(UNCERTAIN): Unity replaced Box2D's per-step `b2_maxTranslation` / `b2_maxRotation` with
//! the speeds in `Physics2DSettings` (`maxTranslationSpeed`, `maxRotationSpeed`); taken here as
//! `speed * h` per step.

use unity_compat::Vec2;
use unity_data::settings::Box2dTuning;

/// `Mathf.Deg2Rad` / `Rad2Deg` as Unity's floats.
pub const DEG2RAD: f32 = std::f32::consts::PI / 180.0;
pub const RAD2DEG: f32 = 57.295_78;

/// A dynamic body (Box2D state: centre of mass position, angle in radians).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RigidBody {
    /// Body origin (`b2Body.m_xf.p`; the centre of mass offset is kept in `local_center`).
    pub position: Vec2,
    /// `b2Sweep.a` (radians).
    pub angle: f32,
    pub linear_velocity: Vec2,
    /// Radians per second.
    pub angular_velocity: f32,
    pub mass: f32,
    pub inv_mass: f32,
    pub inertia: f32,
    pub inv_inertia: f32,
    /// Centre of mass in body coordinates.
    pub local_center: Vec2,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,
    pub force: Vec2,
    pub torque: f32,
}

impl RigidBody {
    pub fn new(position: Vec2, angle_degrees: f32, mass: f32, linear_drag: f32, angular_drag: f32, gravity_scale: f32) -> RigidBody {
        RigidBody {
            position,
            angle: angle_degrees * DEG2RAD,
            linear_velocity: Vec2::ZERO,
            angular_velocity: 0.0,
            mass,
            inv_mass: if mass > 0.0 { 1.0 / mass } else { 0.0 },
            inertia: 0.0,
            inv_inertia: 0.0,
            local_center: Vec2::ZERO,
            linear_damping: linear_drag,
            angular_damping: angular_drag,
            gravity_scale,
            force: Vec2::ZERO,
            torque: 0.0,
        }
    }

    /// `Rigidbody2D.rotation` (degrees).
    pub fn rotation_degrees(&self) -> f32 {
        self.angle * RAD2DEG
    }

    /// `Rigidbody2D.angularVelocity` (degrees per second).
    pub fn angular_velocity_degrees(&self) -> f32 {
        self.angular_velocity * RAD2DEG
    }

    pub fn set_angular_velocity_degrees(&mut self, w: f32) {
        self.angular_velocity = w * DEG2RAD;
    }

    /// The world centre of mass (`b2Sweep.c`).
    pub fn world_center(&self) -> Vec2 {
        let (s, c) = self.angle.sin_cos();
        Vec2::new(self.position.x + c * self.local_center.x - s * self.local_center.y, self.position.y + s * self.local_center.x + c * self.local_center.y)
    }

    /// First half of `b2Island::Solve` for this body: gravity, forces, damping.
    pub fn integrate_velocities(&mut self, h: f32, gravity: Vec2) {
        let mut v = self.linear_velocity;
        let mut w = self.angular_velocity;
        v = v + (gravity * self.gravity_scale + self.force * self.inv_mass) * h;
        w += h * self.inv_inertia * self.torque;
        v = v * (1.0 / (1.0 + h * self.linear_damping));
        w *= 1.0 / (1.0 + h * self.angular_damping);
        self.linear_velocity = v;
        self.angular_velocity = w;
    }

    /// Second half: clamp to the maximum translation / rotation per step, then move.
    pub fn integrate_positions(&mut self, h: f32, tuning: &Box2dTuning) {
        let mut v = self.linear_velocity;
        let mut w = self.angular_velocity;
        let max_translation = tuning.max_translation_speed * h;
        let translation = v * h;
        let d2 = translation.x * translation.x + translation.y * translation.y;
        if d2 > max_translation * max_translation {
            let ratio = max_translation / d2.sqrt();
            v = v * ratio;
        }
        let max_rotation = tuning.max_rotation_speed * DEG2RAD * h;
        let rotation = h * w;
        if rotation * rotation > max_rotation * max_rotation {
            let ratio = max_rotation / rotation.abs();
            w *= ratio;
        }
        // The centre of mass moves; the origin follows (no centre offset yet for these bodies).
        let c = self.world_center() + v * h;
        self.angle += h * w;
        let (s, co) = self.angle.sin_cos();
        self.position = Vec2::new(c.x - (co * self.local_center.x - s * self.local_center.y), c.y - (s * self.local_center.x + co * self.local_center.y));
        self.linear_velocity = v;
        self.angular_velocity = w;
        self.force = Vec2::ZERO;
        self.torque = 0.0;
    }

    /// One `Physics2D` step with no contacts.
    pub fn step(&mut self, h: f32, gravity: Vec2, tuning: &Box2dTuning) {
        self.integrate_velocities(h, gravity);
        self.integrate_positions(h, tuning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn damping_then_integration_in_box2d_order() {
        let mut b = RigidBody::new(Vec2::new(1.0, 2.0), 0.0, 1000.0, 0.0, 0.05, 0.0);
        b.linear_velocity = Vec2::new(4.0, -2.0);
        b.set_angular_velocity_degrees(90.0);
        let h = 0.02;
        b.step(h, Vec2::new(0.0, -16.076), &Box2dTuning::default());
        // No gravity (scale 0), no linear drag: straight line.
        assert!((b.position.x - 1.08).abs() < 1e-6 && (b.position.y - 1.96).abs() < 1e-6);
        // Angular velocity damped by 1 / (1 + h * 0.05) before it moves the angle.
        let w = 90.0 * DEG2RAD / (1.0 + h * 0.05);
        assert!((b.angular_velocity - w).abs() < 1e-6);
        assert!((b.angle - w * h).abs() < 1e-7);
    }

    #[test]
    fn speed_is_capped_by_max_translation_speed() {
        let mut b = RigidBody::new(Vec2::ZERO, 0.0, 1.0, 0.0, 0.0, 0.0);
        b.linear_velocity = Vec2::new(300.0, 0.0);
        b.step(0.02, Vec2::ZERO, &Box2dTuning::default());
        assert!((b.linear_velocity.x - 100.0).abs() < 1e-3);
        assert!((b.position.x - 2.0).abs() < 1e-4);
    }
}
