//! Moving scenery and breakable props (`unity_data::scenery`).
//!
//! * [`ScrollerState`]: `HorizontalScroller2D` — the kinematic body moves at `velocity` (set in
//!   `OnEnable`); each `FixedUpdate`, before the physics step, a body outside `[left, right]` is
//!   moved back by the width.
//! * [`prop_sprite`]: a prop's sprite as an `Explosion` part (`ExplodeSpritesAndLines` takes every
//!   `SpriteShadower` sprite of the object).

use unity_compat::Vec3;
use unity_data::math::Affine;

use crate::visual::{FlatSpriteInstance, shadow_offset_and_scale};

/// A `HorizontalScroller2D` body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollerState {
    /// x as saved in the scene (its sprites' meshes are built there).
    pub start_x: f32,
    pub x: f32,
    pub velocity: f32,
    pub left: f32,
    pub right: f32,
}

impl ScrollerState {
    pub fn of(spec: &unity_data::scenery::Scroller) -> ScrollerState {
        ScrollerState { start_x: spec.x, x: spec.x, velocity: spec.velocity, left: spec.left, right: spec.right }
    }

    /// `FixedUpdate` (the wrap), then the physics step moving the body.
    pub fn fixed_step(&mut self, delta: f32) {
        if self.x < self.left || self.x > self.right {
            let width = self.right - self.left;
            if self.x < self.left {
                self.x += width;
            } else if self.x > self.right {
                self.x -= width;
            }
        }
        self.x += self.velocity * delta;
    }

    /// How far its sprites are from where they were built.
    pub fn offset(&self) -> [f32; 2] {
        [self.x - self.start_x, 0.0]
    }
}

/// A level sprite at `world` (world rotation `rotation`) as a flat sprite part: its roll about
/// the view axis, turned round when it faces away, with its shadow copy's offset and scale from
/// the light.
pub fn prop_sprite(name: &str, world: &Affine, rotation: [f32; 4], scale: [f32; 3], color: [f32; 4], shadow_plane_z: f32, light: Option<Vec3>) -> FlatSpriteInstance {
    let [x, y, z, w] = rotation;
    let roll = (2.0 * (w * z + x * y)).atan2(1.0 - 2.0 * (y * y + z * z)).to_degrees();
    // The image of the local forward axis: a sprite turned about y faces away.
    let forward_z = 1.0 - 2.0 * (x * x + y * y);
    let position = Vec3::new(world.t[0], world.t[1], world.t[2]);
    let (shadow_offset, shadow_scale) = match light {
        Some(light) => shadow_offset_and_scale(light, position, shadow_plane_z),
        None => ([0.0, 0.0], 1.0),
    };
    FlatSpriteInstance {
        bone: 0,
        sprite_name: name.to_owned(),
        position,
        flipped: forward_z < 0.0,
        roll_degrees: roll,
        tilt: crate::visual::NO_TILT,
        scale,
        color,
        shadow_plane_z,
        shadow_offset,
        shadow_scale,
    }
}

/// `Mathf.SmoothStep(from, to, t)`.
fn smooth_step(from: f32, to: f32, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let t = -2.0 * t * t * t + 3.0 * t * t;
    to * t + from * (1.0 - t)
}

/// `Whale.Go` (Pirates): the whale's height above its start `time` seconds after the level loaded
/// everywhere. It waits `start_wait_time`, then repeats: `down_time` down, rise over
/// `transition_time`, `up_time` up, sink over `transition_time`.
pub fn whale_offset(spec: &unity_data::scenery::WhaleSpec, time: f32) -> f32 {
    let t = time - spec.start_wait_time;
    let transition = spec.transition_time.max(1e-3);
    let cycle = spec.down_time + spec.up_time + 2.0 * transition;
    if t <= 0.0 || cycle <= 0.0 {
        return 0.0;
    }
    let p = t % cycle;
    let up = spec.up_y_offset;
    if p < spec.down_time {
        0.0
    } else if p < spec.down_time + transition {
        smooth_step(0.0, up, (p - spec.down_time) / transition)
    } else if p < spec.down_time + transition + spec.up_time {
        up
    } else {
        smooth_step(up, 0.0, (p - spec.down_time - transition - spec.up_time) / transition)
    }
}

/// `StarShower.Update`'s rate per second for a target at `x` (before the vehicle multiplier and
/// the number of targets); `None` outside `[start_x, end_x]`.
pub fn star_rate(spec: &unity_data::scenery::StarShowerSpec, x: f32) -> Option<f32> {
    if x < spec.start_x || x > spec.end_x {
        return None;
    }
    let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t.clamp(0.0, 1.0);
    Some(if x < spec.peak_start_x {
        lerp(spec.start_rate, spec.peak_rate, (x - spec.start_x) / (spec.peak_start_x - spec.start_x))
    } else if x < spec.peak_end_x {
        spec.peak_rate
    } else {
        lerp(spec.peak_rate, spec.end_rate, (x - spec.peak_end_x) / (spec.end_x - spec.peak_end_x))
    })
}

/// `StarShower.Update`'s launch point for a target at `target`: `launch_offset_y` above it and
/// back along `launch_velocity` by the time the fall takes (the larger root of
/// `½ g t² + v_y t + offset = 0`), before the random spread.
pub fn star_launch_point(spec: &unity_data::scenery::StarShowerSpec, target: [f32; 2], gravity_y: f32) -> [f32; 2] {
    let vy = spec.launch_velocity[1];
    let a = 0.5 * gravity_y;
    let c = spec.launch_offset_y;
    let root = (vy * vy - 4.0 * a * c).max(0.0).sqrt();
    let t = ((-vy + root) / (2.0 * a)).max((-vy - root) / (2.0 * a));
    [target[0] - t * spec.launch_velocity[0], target[1] + spec.launch_offset_y]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clouds_wrap_before_moving() {
        let spec = unity_data::scenery::Scroller { object: 1, name: "Cloud".into(), velocity: -3.0, left: -60.0, right: 660.0, x: -59.0 };
        let mut s = ScrollerState::of(&spec);
        s.fixed_step(0.5);
        assert!((s.x - (-60.5)).abs() < 1e-4);
        // Out of range: wrapped first, then moved.
        s.fixed_step(0.5);
        assert!((s.x - (-60.5 + 720.0 - 1.5)).abs() < 1e-3);
        assert!((s.offset()[0] - (s.x + 59.0)).abs() < 1e-4);
    }

    #[test]
    fn prop_sprite_takes_roll_and_shadow() {
        let world = Affine::from_trs([2.0, 3.0, -0.1], (0.0, 0.0, 0.0, 1.0), [1.0; 3]);
        let part = prop_sprite("Ball", &world, [0.0, 0.0, 0.0, 1.0], [1.0; 3], [1.0; 4], 0.0, Some(Vec3::new(0.0, 0.0, -10.0)));
        assert_eq!(part.roll_degrees, 0.0);
        assert!(!part.flipped);
        assert!(part.shadow_scale > 1.0);
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let turned = prop_sprite("Ball", &world, [0.0, 0.0, half, half], [1.0; 3], [1.0; 4], 0.0, None);
        assert!((turned.roll_degrees - 90.0).abs() < 1e-3);
    }

    #[test]
    fn whales_wait_rise_hold_and_sink() {
        let w = unity_data::scenery::WhaleSpec { object: 1, up_time: 4.0, down_time: 4.0, transition_time: 1.0, up_y_offset: 14.0, start_wait_time: 2.0 };
        assert_eq!(super::whale_offset(&w, 1.0), 0.0);
        assert_eq!(super::whale_offset(&w, 5.9), 0.0);
        assert!((super::whale_offset(&w, 6.5) - 7.0).abs() < 1e-4);
        assert_eq!(super::whale_offset(&w, 9.0), 14.0);
        assert!((super::whale_offset(&w, 11.5) - 7.0).abs() < 1e-4);
        assert_eq!(super::whale_offset(&w, 12.5), 0.0);
        // The cycle repeats every 10 s.
        assert!((super::whale_offset(&w, 16.5) - 7.0).abs() < 1e-4);
    }

    #[test]
    fn stars_fall_onto_their_target() {
        let spec = unity_data::scenery::StarShowerSpec {
            prefab: String::new(),
            start_x: 326.0,
            peak_start_x: 504.0,
            peak_end_x: 768.0,
            end_x: 818.0,
            start_rate: 0.5,
            peak_rate: 8.0,
            end_rate: 0.5,
            vehicle_rate_multiplier: 10.0,
            launch_angular_velocity_min: -229.18312,
            launch_angular_velocity_max: -57.29578,
            launch_velocity: [-2.0, -1.0],
            launch_offset_y: 32.0,
            random_range: 60.0,
        };
        assert_eq!(super::star_rate(&spec, 300.0), None);
        assert_eq!(super::star_rate(&spec, 326.0), Some(0.5));
        assert_eq!(super::star_rate(&spec, 600.0), Some(8.0));
        assert!((super::star_rate(&spec, 793.0).unwrap() - 4.25).abs() < 1e-4);
        // Launched there with the launch velocity, a body under gravity reaches the target's
        // height at the target's x.
        let g = -8.055;
        let start = super::star_launch_point(&spec, [500.0, -10.0], g);
        let t = {
            let (a, b, c) = (0.5 * g, spec.launch_velocity[1], spec.launch_offset_y);
            (-b - (b * b - 4.0 * a * c).sqrt()) / (2.0 * a)
        };
        assert!((start[0] + spec.launch_velocity[0] * t - 500.0).abs() < 1e-3);
        assert!((start[1] + spec.launch_velocity[1] * t + 0.5 * g * t * t + 10.0).abs() < 1e-3);
    }
}
