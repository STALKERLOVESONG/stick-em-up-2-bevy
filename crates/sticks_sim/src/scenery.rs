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
}
