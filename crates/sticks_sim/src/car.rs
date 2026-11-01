//! `CarControl` (the moon rovers): the driver's desired speed spins the wheels and pushes the
//! body, the rotation input tilts it, and a car that lands after two full flips earns the
//! "rover 720" goal ("rover 1440" after four). The bodies are stepped by
//! `crate::contact::step_with_joints` (chassis and wheels on `WheelJoint`s).

use unity_compat::{Vec2, mathf};
use unity_data::vehicles::CarTuning;

use crate::contact::DynamicBody;

/// `CarControl.SignOrZero`.
fn sign_or_zero(f: f32) -> f32 {
    if f == 0.0 {
        0.0
    } else if f > 0.0 {
        1.0
    } else {
        -1.0
    }
}

/// What landing after flips earned (`Goals.GoalDone`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlipGoal {
    /// "rover 720".
    TwoFlips,
    /// "rover 720" and "rover 1440".
    FourFlips,
}

/// A car's controller state.
#[derive(Clone, Debug, PartialEq)]
pub struct CarDrive {
    pub tuning: CarTuning,
    /// `2π r` of each wheel.
    wheel_circumferences: Vec<f32>,
    /// `desiredSpeedControl` / `desiredClockwiseRotationControl`.
    desired_speed: f32,
    clockwise: f32,
    last_airborne: bool,
    half_flips: u32,
    last_upside_down: bool,
}

impl CarDrive {
    pub fn new(tuning: CarTuning, wheel_radii: &[f32]) -> CarDrive {
        CarDrive {
            tuning,
            wheel_circumferences: wheel_radii.iter().map(|r| std::f32::consts::PI * 2.0 * r).collect(),
            desired_speed: 0.0,
            clockwise: 0.0,
            last_airborne: false,
            half_flips: 0,
            last_upside_down: false,
        }
    }

    /// `ClearController` / `OnRespawned`: no input.
    pub fn clear(&mut self) {
        self.desired_speed = 0.0;
        self.clockwise = 0.0;
    }

    /// `engineSound.pitch`.
    pub fn engine_pitch(&self) -> f32 {
        mathf::clamp01(self.desired_speed.abs()) * 0.17 + 0.48
    }

    /// `CarControl.FixedUpdate`, before the physics step. `bodies[0]` is the chassis and
    /// `bodies[1..]` the wheels; `wheels_grounded[i]` is whether wheel `i` touches anything
    /// (`IsTouchingLayers(~(1 << 29))`), `body_touching` whether the chassis does. `input` is the
    /// driver's `(DesiredSpeed, ClockwiseRotation)` while somebody drives. A landing that
    /// completes the flip goal is returned when somebody drives.
    pub fn fixed_update(&mut self, bodies: &mut [DynamicBody], wheels_grounded: &[bool], body_touching: bool, input: Option<(f32, f32)>, dt: f32) -> Option<FlipGoal> {
        let driven = input.is_some();
        if let Some((speed, rotation)) = input {
            self.desired_speed = speed;
            self.clockwise = rotation;
        }
        let t = self.tuning;
        let target = self.desired_speed * t.max_speed;
        let n = self.wheel_circumferences.len().min(bodies.len().saturating_sub(1));
        let mut grounded = 0;
        for i in 0..n {
            let wanted = -360.0 * (target / self.wheel_circumferences[i]);
            let wheel = &mut bodies[1 + i].body;
            let w = wheel.angular_velocity_degrees();
            if sign_or_zero(w) != sign_or_zero(wanted) || !(w.abs() > wanted.abs()) {
                wheel.set_angular_velocity_degrees(wanted);
            }
            if wheels_grounded.get(i).copied().unwrap_or(false) {
                grounded += 1;
            }
        }
        let chassis = bodies[0].body;
        let (s, c) = chassis.angle.sin_cos();
        let right = Vec2::new(c, s);
        let x = right.dot(chassis.linear_velocity);
        let reversing = sign_or_zero(x) != sign_or_zero(target);
        if n > 0 && (reversing || !(x.abs() > target.abs())) {
            let f = (target - x) / dt;
            let limit = if reversing { t.max_deceleration } else { t.max_acceleration } * (grounded as f32 / n as f32);
            let push = f.abs().min(limit) * mathf::sign(f);
            let force = right * push;
            bodies[0].body.force = bodies[0].body.force + force * chassis.mass;
            for i in 0..n {
                let wheel = &mut bodies[1 + i].body;
                wheel.force = wheel.force + force * wheel.mass;
            }
            bodies[0].body.torque += push * chassis.mass * t.acceleration_body_torque_factor;
        }
        // Flips while airborne.
        let airborne = grounded == 0 && !body_touching;
        let angle = mathf::delta_angle(0.0, bodies[0].body.rotation_degrees());
        let mut goal = None;
        if airborne {
            let upside_down = !(-90.0..=90.0).contains(&angle);
            if !self.last_airborne {
                self.last_upside_down = upside_down;
                self.half_flips = 0;
            } else if self.last_upside_down != upside_down {
                self.half_flips += 1;
                self.last_upside_down = upside_down;
            }
        } else if self.last_airborne {
            let upright = (-60.0..=60.0).contains(&angle);
            if driven && self.half_flips >= 4 && upright {
                goal = Some(if self.half_flips >= 8 { FlipGoal::FourFlips } else { FlipGoal::TwoFlips });
            }
        }
        self.last_airborne = airborne;
        // Air (and ground) rotation control.
        let w = bodies[0].body.angular_velocity_degrees() + -self.clockwise * t.max_angular_acceleration * dt;
        bodies[0].body.set_angular_velocity_degrees(w);
        goal
    }
}

/// `RocketControl` (Moon's lunar module): the pilot's throttle (`Clamp01(DesiredSpeed)`) pushes
/// the body along its up axis from the engine, and its rotation input steers the angular
/// velocity towards `-ClockwiseRotation × maxAngularVelocity`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RocketDrive {
    pub tuning: unity_data::vehicles::RocketTuning,
    /// `desiredSpeedControl` / `desiredClockwiseRotationControl`.
    desired_speed: f32,
    clockwise: f32,
}

impl RocketDrive {
    /// `engineSound.volume = desiredSpeedControl` (the throttle).
    pub fn engine_volume(&self) -> f32 {
        self.desired_speed
    }

    pub fn new(tuning: unity_data::vehicles::RocketTuning) -> RocketDrive {
        RocketDrive { tuning, desired_speed: 0.0, clockwise: 0.0 }
    }

    /// `ClearController` / `OnRespawned`.
    pub fn clear(&mut self) {
        self.desired_speed = 0.0;
        self.clockwise = 0.0;
    }

    /// The throttle (`engineSound.volume`, and × `engineParticleRate` the exhaust's rate).
    pub fn throttle(&self) -> f32 {
        self.desired_speed
    }

    /// `RocketControl.FixedUpdate`, before the physics step. `engine` is the engine's position in
    /// the body's frame; `input` the pilot's `(DesiredSpeed, ClockwiseRotation)` while flown.
    pub fn fixed_update(&mut self, body: &mut crate::rigid_body::RigidBody, engine: Vec2, input: Option<(f32, f32)>, dt: f32) {
        if let Some((speed, rotation)) = input {
            self.desired_speed = mathf::clamp01(speed);
            self.clockwise = rotation;
        }
        let t = self.tuning;
        // `AddForceAtPosition(TransformDirection(0, mass × acceleration), engine.position)`.
        let thrust = body.mass * self.desired_speed * t.max_acceleration;
        let (s, c) = body.angle.sin_cos();
        let force = Vec2::new(-s * thrust, c * thrust);
        let at = body.position + Vec2::new(c * engine.x - s * engine.y, s * engine.x + c * engine.y);
        let r = at - body.world_center();
        body.force = body.force + force;
        body.torque += r.x * force.y - r.y * force.x;
        let target = -self.clockwise * t.max_angular_velocity;
        let w = mathf::move_towards(body.angular_velocity_degrees(), target, t.max_angular_acceleration * dt);
        body.set_angular_velocity_degrees(w);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contact::{Polygon, WarmStart, WheelJoint, mass_properties, step_with_joints};
    use crate::physics::{PhysicsRules, StaticCollider, StaticShape, World};
    use crate::rigid_body::RigidBody;

    fn rover() -> (Vec<DynamicBody>, Vec<WheelJoint>, CarDrive) {
        let chassis = vec![Polygon::new(vec![Vec2::new(-3.0, -0.5), Vec2::new(3.0, -0.5), Vec2::new(3.0, 0.6), Vec2::new(-3.0, 0.6)])];
        let mut body = RigidBody::new(Vec2::new(0.0, 2.5), 0.0, 5.0, 0.0, 0.05, 1.0);
        let (lc, inertia) = mass_properties(&chassis, 5.0);
        body.local_center = lc;
        body.inertia = inertia;
        body.inv_inertia = 1.0 / inertia;
        let mut bodies = vec![DynamicBody { key: 1, body, pieces: chassis, layer: 0 }];
        let mut joints = Vec::new();
        let radius = 0.873;
        for (k, x) in [2.52f32, -2.84].into_iter().enumerate() {
            let mut wheel = RigidBody::new(Vec2::new(x, 2.2), 0.0, 1.0, 0.0, 0.05, 1.0);
            wheel.inertia = 0.5 * radius * radius;
            wheel.inv_inertia = 1.0 / wheel.inertia;
            bodies.push(DynamicBody { key: 2 + k as u64, body: wheel, pieces: vec![Polygon::disc(Vec2::ZERO, radius)], layer: 0 });
            joints.push(WheelJoint::new(0, 1 + k, Vec2::new(x, -0.3), Vec2::ZERO, Vec2::new(0.0, 1.0), 5.0, 0.25));
        }
        let tuning = CarTuning { max_speed: 50.0, max_acceleration: 500.0, max_deceleration: 125.0, max_angular_acceleration: 1080.0, acceleration_body_torque_factor: 0.25 };
        (bodies, joints, CarDrive::new(tuning, &[radius, radius]))
    }

    #[test]
    fn rover_drives_to_its_speed_on_flat_ground() {
        let world = World::build(vec![(StaticCollider { id: 1, ..Default::default() }, StaticShape::Polygon(vec![Vec2::new(-500.0, -1.0), Vec2::new(500.0, -1.0), Vec2::new(500.0, 0.0), Vec2::new(-500.0, 0.0)]))]);
        let rules = PhysicsRules { gravity: Vec2::new(0.0, -9.81), layer_matrix: [u32::MAX; 32], raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 };
        let (mut bodies, mut joints, mut drive) = rover();
        let mut warm = WarmStart::default();
        let dt = 0.02;
        let mut touching = vec![false; 2];
        for frame in 0..200 {
            let input = if frame < 50 { Some((0.0, 0.0)) } else { Some((0.4, 0.0)) };
            drive.fixed_update(&mut bodies, &touching, false, input, dt);
            let touches = step_with_joints(&mut bodies, &mut joints, &world, &rules, dt, 8, 3, &mut warm);
            touching = (1..=2).map(|i| touches.iter().any(|t| matches!(t, crate::contact::Touch::Static { body, .. } if *body == i))).collect();
        }
        let v = bodies[0].body.linear_velocity.x;
        // Desired speed 0.4 × 50 = 20 units per second along the car.
        assert!((v - 20.0).abs() < 3.0, "speed {v}");
        assert!(bodies[0].body.angle.abs() < 0.3, "tilted {}", bodies[0].body.angle);
    }

    #[test]
    fn full_throttle_lifts_the_module_and_the_stick_turns_it() {
        let tuning = unity_data::vehicles::RocketTuning { engine: None, max_acceleration: 20.0, max_angular_velocity: 60.0, max_angular_acceleration: 135.0, engine_particle_rate: 500.0 };
        let mut drive = RocketDrive::new(tuning);
        let mut body = RigidBody::new(Vec2::ZERO, 0.0, 10.0, 0.0, 0.05, 1.0);
        drive.fixed_update(&mut body, Vec2::new(0.0, -2.86), Some((1.5, 0.0)), 0.02);
        // Clamped to full throttle: 10 kg × 20 m/s² straight up, through the centre of mass.
        assert_eq!(drive.throttle(), 1.0);
        assert!((body.force.y - 200.0).abs() < 1e-3 && body.force.x.abs() < 1e-4 && body.torque.abs() < 1e-3);
        // Clockwise input: the angular velocity heads for -60°/s at 135°/s².
        body.force = Vec2::ZERO;
        drive.fixed_update(&mut body, Vec2::new(0.0, -2.86), Some((0.0, 1.0)), 0.02);
        assert!((body.angular_velocity_degrees() + 2.7).abs() < 1e-3);
        for _ in 0..100 {
            drive.fixed_update(&mut body, Vec2::new(0.0, -2.86), Some((0.0, 1.0)), 0.02);
        }
        assert!((body.angular_velocity_degrees() + 60.0).abs() < 1e-3);
    }
}
