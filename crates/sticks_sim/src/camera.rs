//! The level camera rig (`CameraPositioner`): frames every influence point, derives the camera
//! distance from the framed bounds and the lens, then smooths towards the target with Unity's
//! `SmoothDamp`.
//!
//! Runs once per rendered frame in the `LateUpdate` slot, after characters have moved, and
//! consumes the influences registered that frame.

use unity_compat::mathf::{smooth_damp, smooth_damp_vec2};
use unity_compat::Vec2;
pub use unity_data::level::PositionerSettings;

/// Horizontal smoothing time of the rig (`CameraPositioner.smoothTime`, a constant).
pub const SMOOTH_TIME: f32 = 0.2;

/// Camera position in game space (z is negative: the camera looks down +z at the z = 0 plane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPosition {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Debug)]
pub struct CameraRig {
    pub settings: PositionerSettings,
    /// Vertical field of view in degrees (`Camera.fieldOfView`).
    pub field_of_view: f32,
    /// World position of the camera (its parent's position plus `CameraPositioner`'s
    /// `localPosition`).
    pub position: CameraPosition,
    /// The camera's parent ("Camera Offset" in Corruption 2, Moon, Pirates and Treetops) on the
    /// x/y plane: `CameraPositioner` works in `localPosition`, so the framed centre is offset by
    /// it in the world. Zero for a camera at the scene root.
    pub parent_offset: Vec2,
    /// `CameraOffsetXChanger.velocity` (its `SmoothDamp` of the parent's x).
    parent_velocity_x: f32,
    velocity: Vec2,
    velocity_z: f32,
    influencers: Vec<[f32; 2]>,
    distance_only_influencers: Vec<[f32; 2]>,
}

impl CameraRig {
    pub fn new(settings: PositionerSettings, field_of_view: f32, position: CameraPosition) -> CameraRig {
        CameraRig {
            settings,
            field_of_view,
            position,
            parent_offset: Vec2::ZERO,
            parent_velocity_x: 0.0,
            velocity: Vec2::ZERO,
            velocity_z: 0.0,
            influencers: Vec::with_capacity(4),
            distance_only_influencers: Vec::with_capacity(4),
        }
    }

    /// A camera under a parent at `offset` (world `position` stays as given).
    pub fn with_parent_offset(mut self, offset: Vec2) -> CameraRig {
        self.parent_offset = offset;
        self
    }

    /// `CameraOffsetXChanger.Apply`: the parent's x `SmoothDamp`s to `target` (0.2 s), carrying
    /// the camera along.
    pub fn move_parent_x(&mut self, target: f32, delta_time: f32) {
        let x = smooth_damp(self.parent_offset.x, target, &mut self.parent_velocity_x, SMOOTH_TIME, f32::INFINITY, delta_time);
        self.position.x += x - self.parent_offset.x;
        self.parent_offset.x = x;
    }

    /// `CameraPositioner.AddInfluence`: the point must stay on screen this frame.
    pub fn add_influence(&mut self, point: Vec2) {
        self.influencers.push([point.x, point.y]);
    }

    /// `CameraPositioner.AddDistanceOnlyInfluence`: widens the framing symmetrically around the
    /// centre of the ordinary influences without moving that centre.
    pub fn add_distance_only_influence(&mut self, point: Vec2) {
        self.distance_only_influencers.push([point.x, point.y]);
    }

    /// The `LateUpdate` step. `aspect` is width / height of the viewport.
    pub fn late_update(&mut self, aspect: f32, delta_time: f32) {
        let s = &self.settings;
        let tan_half = (std::f32::consts::PI / 180.0 * self.field_of_view * 0.5).tan();
        let tan_half_fov = Vec2::new(aspect * tan_half, tan_half);

        // Unity `Bounds`: centre + size, grown with `Encapsulate`.
        let (mut min, mut max);
        let mut center;
        let offset = self.parent_offset;
        let local = Vec2::new(self.position.x - offset.x, self.position.y - offset.y);
        if self.influencers.is_empty() {
            center = local;
            (min, max) = (center, center);
        } else {
            let first = self.influencers[0];
            (min, max) = (Vec2::new(first[0], first[1]), Vec2::new(first[0], first[1]));
            for p in &self.influencers[1..] {
                min = Vec2::new(min.x.min(p[0]), min.y.min(p[1]));
                max = Vec2::new(max.x.max(p[0]), max.y.max(p[1]));
            }
            // `Bounds.center` is the midpoint of the encapsulated box, not the first point.
            center = (min + max) * 0.5;
        }
        // The original reads `bounds.center` once *before* distance-only points are added, so the
        // camera aims at the ordinary influences while framing the symmetric extent.
        let aim = center;
        for p in &self.distance_only_influencers {
            let point = Vec2::new(p[0], p[1]);
            let mirrored = aim - (point - aim);
            for q in [point, mirrored] {
                min = Vec2::new(min.x.min(q.x), min.y.min(q.y));
                max = Vec2::new(max.x.max(q.x), max.y.max(q.y));
            }
        }
        self.influencers.clear();
        self.distance_only_influencers.clear();
        center = aim;

        let mut size = max - min;
        size = Vec2::new(size.x + s.screen_edge_margin, size.y + s.screen_edge_margin);
        size = Vec2::new(size.x.max(s.minimum_on_screen_width), size.y.max(s.minimum_on_screen_height));
        let extents = size * 0.5;
        let distance = (extents.y / tan_half_fov.y).max(extents.x / tan_half_fov.x);
        let distance = distance.clamp(s.minimum_distance, s.maximum_distance);
        let half_visible_height = tan_half_fov.y * distance;
        center.y = (s.lowest_on_screen_y + half_visible_height).max(center.y);
        let target_z = -distance;

        // `transform.localPosition = SmoothDamp(localPosition, center)`: the world influences are
        // taken as local coordinates, so the parent's offset adds on top.
        let smoothed = smooth_damp_vec2(local, center, &mut self.velocity, SMOOTH_TIME, f32::INFINITY, delta_time);
        let z = smooth_damp(self.position.z, target_z, &mut self.velocity_z, s.smooth_time_z, f32::INFINITY, delta_time);
        self.position = CameraPosition { x: smoothed.x + offset.x, y: smoothed.y + offset.y, z };
    }

    /// `LocalOffScreenReactivator.CameraBounds`: what the camera sees of the z = 0 plane
    /// (`ViewportToWorldPoint` of the viewport corners at depth `-camera.z`), as Unity's `Bounds`
    /// built from centre and size. The camera looks straight down +z.
    ///
    /// TODO(UNCERTAIN): Unity goes through the inverse view-projection matrix, so the corners may
    /// differ from this direct formula in the last ulp.
    pub fn view_bounds(&self, aspect: f32) -> ViewBounds {
        let depth = -self.position.z;
        let half_height = depth * (std::f32::consts::PI / 180.0 * self.field_of_view * 0.5).tan();
        let half_width = half_height * aspect;
        let lower = Vec2::new(self.position.x - half_width, self.position.y - half_height);
        let upper = Vec2::new(self.position.x + half_width, self.position.y + half_height);
        let center = (upper + lower) * 0.5;
        let size = upper - lower;
        ViewBounds { center, extents: size * 0.5 }
    }
}

/// A Unity `Bounds` on the z = 0 plane (centre and extents, as Unity stores it).
///
/// The real bounds are three-dimensional with (almost) zero depth at z = 0; positions are 2D
/// points at z = 0, so the depth only matters through rounding of the corners' z. Whether
/// `Contains` then fails is irrelevant: a point inside has zero `SqrDistance`, which passes the
/// radius test anyway (every radius in use is positive).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewBounds {
    pub center: Vec2,
    pub extents: Vec2,
}

impl ViewBounds {
    pub fn min(&self) -> Vec2 {
        self.center - self.extents
    }

    pub fn max(&self) -> Vec2 {
        self.center + self.extents
    }

    /// `Bounds.Contains`.
    pub fn contains(&self, p: Vec2) -> bool {
        let (min, max) = (self.min(), self.max());
        p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y
    }

    /// `Bounds.SqrDistance`: squared distance from `p` to the box (zero inside).
    pub fn sqr_distance(&self, p: Vec2) -> f32 {
        let (min, max) = (self.min(), self.max());
        let mut d = 0.0;
        for (v, lo, hi) in [(p.x, min.x, max.x), (p.y, min.y, max.y)] {
            if v < lo {
                d += (lo - v) * (lo - v);
            } else if v > hi {
                d += (v - hi) * (v - hi);
            }
        }
        d
    }

    /// `OffScreenReactivator.IsOnScreen`: inside the view, or closer to it than `radius`.
    pub fn is_on_screen(&self, position: Vec2, radius: f32) -> bool {
        self.contains(position) || self.sqr_distance(position) < radius * radius
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn view_bounds_follow_the_lens_and_the_radius_reaches_past_the_edge() {
        // 60° vertical field of view at distance 16: half height = 16 tan 30° ≈ 9.2376.
        let c = CameraRig::new(PositionerSettings::default(), 60.0, CameraPosition { x: 100.0, y: 20.0, z: -16.0 });
        let b = c.view_bounds(16.0 / 9.0);
        let half_h = 16.0 * (30.0f32).to_radians().tan();
        assert!((b.extents.y - half_h).abs() < 1e-4);
        assert!((b.extents.x - half_h * 16.0 / 9.0).abs() < 1e-4);
        assert!((b.center.x - 100.0).abs() < 1e-4 && (b.center.y - 20.0).abs() < 1e-4);
        let right_edge = b.max().x;
        assert!(b.is_on_screen(Vec2::new(100.0, 20.0), 3.0));
        assert!(b.is_on_screen(Vec2::new(right_edge + 2.9, 20.0), 3.0));
        assert!(!b.is_on_screen(Vec2::new(right_edge + 3.01, 20.0), 3.0));
        // Strictly closer than the radius: exactly on the radius is off screen.
        let probe = ViewBounds { center: Vec2::new(0.0, 0.0), extents: Vec2::new(1.0, 1.0) };
        assert!(!probe.is_on_screen(Vec2::new(4.0, 0.0), 3.0));
        assert!(probe.is_on_screen(Vec2::new(1.0, 0.5), 0.0), "the edge itself is inside");
        // Diagonal distance from the corner counts, not each axis separately.
        assert!(!b.is_on_screen(Vec2::new(right_edge + 2.5, b.max().y + 2.5), 3.0));
    }

    const DT: f32 = 1.0 / 60.0;
    const ASPECT: f32 = 16.0 / 9.0;

    fn rig() -> CameraRig {
        let settings = PositionerSettings {
            screen_edge_margin: 18.0,
            lowest_on_screen_y: -105.0,
            minimum_distance: 16.0,
            maximum_distance: 75.0,
            ..PositionerSettings::default()
        };
        CameraRig::new(settings, 60.0, CameraPosition { x: 0.0, y: 0.0, z: -16.0 })
    }

    #[test]
    fn single_influence_converges_on_the_point_at_minimum_distance_clamped_by_margin() {
        let mut c = rig();
        for _ in 0..600 {
            c.add_influence(Vec2::new(12.0, 5.0));
            c.late_update(ASPECT, DT);
        }
        assert!((c.position.x - 12.0).abs() < 1e-3);
        assert!((c.position.y - 5.0).abs() < 1e-3);
        // size = max(0 + 18, minimumOnScreenWidth 50) wide; extents.x = 25 → 25 / (tan30° * 16/9)
        let tan = (30.0f32).to_radians().tan();
        let expected = (25.0 / (tan * ASPECT)).clamp(16.0, 75.0);
        assert!((c.position.z + expected).abs() < 1e-2, "z = {} expected {}", c.position.z, -expected);
    }

    #[test]
    fn two_influences_widen_framing_and_centre_between_them() {
        let mut c = rig();
        for _ in 0..900 {
            c.add_influence(Vec2::new(-30.0, 0.0));
            c.add_influence(Vec2::new(30.0, 10.0));
            c.late_update(ASPECT, DT);
        }
        assert!(c.position.x.abs() < 1e-3);
        assert!((c.position.y - 5.0).abs() < 1e-3);
        let tan = (30.0f32).to_radians().tan();
        let expected = ((60.0 + 18.0) * 0.5 / (tan * ASPECT)).clamp(16.0, 75.0);
        assert!((c.position.z + expected).abs() < 1e-2);
    }

    #[test]
    fn camera_never_shows_below_lowest_on_screen_y() {
        let mut c = rig();
        for _ in 0..900 {
            c.add_influence(Vec2::new(0.0, -500.0));
            c.late_update(ASPECT, DT);
        }
        let tan = (30.0f32).to_radians().tan();
        let half_height = tan * -c.position.z;
        assert!(c.position.y - half_height >= -105.0 - 1e-2, "{} {}", c.position.y, half_height);
    }

    #[test]
    fn follows_with_smooth_damp_lag_not_instantly() {
        let mut c = rig();
        c.add_influence(Vec2::new(40.0, 0.0));
        c.late_update(ASPECT, DT);
        assert!(c.position.x > 0.0 && c.position.x < 5.0, "first-frame x = {}", c.position.x);
    }

    #[test]
    fn without_influences_the_camera_holds_its_xy_and_still_settles_in_z() {
        let mut c = rig();
        c.position = CameraPosition { x: 3.0, y: 4.0, z: -50.0 };
        for _ in 0..600 {
            c.late_update(ASPECT, DT);
        }
        assert!((c.position.x - 3.0).abs() < 1e-3 && (c.position.y - 4.0).abs() < 1e-3);
        assert!(c.position.z > -50.0 && c.position.z <= -16.0);
    }

    #[test]
    fn distance_only_influence_widens_without_moving_centre() {
        let mut c = rig();
        for _ in 0..900 {
            c.add_influence(Vec2::new(0.0, 0.0));
            c.add_distance_only_influence(Vec2::new(40.0, 0.0));
            c.late_update(ASPECT, DT);
        }
        assert!(c.position.x.abs() < 1e-3);
        let tan = (30.0f32).to_radians().tan();
        let expected = ((80.0 + 18.0) * 0.5 / (tan * ASPECT)).clamp(16.0, 75.0);
        assert!((c.position.z + expected).abs() < 1e-2);
    }

    #[test]
    fn a_parent_offset_lifts_the_framed_centre() {
        let mut plain = rig();
        let mut offset = rig().with_parent_offset(Vec2::new(0.0, 5.0));
        offset.position.y += 5.0;
        for _ in 0..600 {
            plain.add_influence(Vec2::new(10.0, 20.0));
            offset.add_influence(Vec2::new(10.0, 20.0));
            plain.late_update(16.0 / 9.0, 1.0 / 60.0);
            offset.late_update(16.0 / 9.0, 1.0 / 60.0);
        }
        assert!((offset.position.y - plain.position.y - 5.0).abs() < 1e-3, "{:?} vs {:?}", offset.position, plain.position);
        assert!((offset.position.x - plain.position.x).abs() < 1e-3);
        offset.move_parent_x(13.0, 10.0);
        assert!(offset.parent_offset.x > 12.0 && offset.position.x > plain.position.x + 12.0);
    }
}
