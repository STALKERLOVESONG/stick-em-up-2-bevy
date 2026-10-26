//! `Vector3` and `Quaternion` with Unity's conventions (left-handed, `a * b` applies `b` first).
//!
//! Only what the character rig needs: composition, rotation of vectors, axis rotations,
//! normalized lerp and `LookRotation`.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const RIGHT: Vec3 = Vec3 { x: 1.0, y: 0.0, z: 0.0 };
    pub const UP: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    pub const FORWARD: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 1.0 };
    pub const BACK: Vec3 = Vec3 { x: 0.0, y: 0.0, z: -1.0 };

    pub const fn new(x: f32, y: f32, z: f32) -> Vec3 {
        Vec3 { x, y, z }
    }

    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
    }

    pub fn sqr_magnitude(self) -> f32 {
        self.dot(self)
    }

    pub fn magnitude(self) -> f32 {
        self.sqr_magnitude().sqrt()
    }

    /// `Vector3.normalized` (zero stays zero).
    pub fn normalized(self) -> Vec3 {
        let m = self.magnitude();
        if m > 1e-5 { self * (1.0 / m) } else { Vec3::ZERO }
    }

    pub fn lerp(a: Vec3, b: Vec3, t: f32) -> Vec3 {
        a + (b - a) * t
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    fn default() -> Quat {
        Quat::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Quat {
        Quat { x, y, z, w }
    }

    pub fn from_array(q: [f32; 4]) -> Quat {
        Quat::new(q[0], q[1], q[2], q[3])
    }

    /// `Quaternion.AngleAxis` for a unit axis, radians.
    pub fn axis_angle(axis: Vec3, radians: f32) -> Quat {
        let half = radians * 0.5;
        let s = half.sin();
        Quat::new(axis.x * s, axis.y * s, axis.z * s, half.cos())
    }

    pub fn about_x(radians: f32) -> Quat {
        Quat::axis_angle(Vec3::RIGHT, radians)
    }

    pub fn about_y(radians: f32) -> Quat {
        Quat::axis_angle(Vec3::UP, radians)
    }

    pub fn about_z(radians: f32) -> Quat {
        Quat::axis_angle(Vec3::FORWARD, radians)
    }

    /// `Quaternion.Euler(0, degrees, 0)`.
    pub fn euler_y_degrees(degrees: f32) -> Quat {
        Quat::about_y(degrees.to_radians())
    }

    /// Inverse of a unit quaternion.
    pub fn conjugate(self) -> Quat {
        Quat::new(-self.x, -self.y, -self.z, self.w)
    }

    pub fn dot(self, o: Quat) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }

    pub fn normalized(self) -> Quat {
        let m = self.dot(self).sqrt();
        if m > 0.0 { Quat::new(self.x / m, self.y / m, self.z / m, self.w / m) } else { Quat::IDENTITY }
    }

    /// `self * v` (rotate a vector).
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }

    /// Normalized lerp along the shorter arc.
    pub fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
        let b = if a.dot(b) < 0.0 { Quat::new(-b.x, -b.y, -b.z, -b.w) } else { b };
        Quat::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t, a.z + (b.z - a.z) * t, a.w + (b.w - a.w) * t).normalized()
    }

    /// Rotation matrix rows (`[row][column]`), as Unity's `Matrix4x4.Rotate`.
    pub fn to_matrix(self) -> [[f32; 3]; 3] {
        let Quat { x, y, z, w } = self;
        [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
        ]
    }

    /// `Quaternion.eulerAngles` in degrees, each component in `[0, 360)`.
    ///
    /// Unity applies Euler angles in z, x, y order, so `R = Ry · Rx · Rz`.
    pub fn euler_angles_degrees(self) -> Vec3 {
        let r = self.to_matrix();
        let sx = (-r[1][2]).clamp(-1.0, 1.0);
        let x = sx.asin();
        let (y, z) = if sx.abs() < 0.999_999 {
            (r[0][2].atan2(r[2][2]), r[1][0].atan2(r[1][1]))
        } else {
            // Gimbal lock: fold the roll into the yaw.
            ((-r[2][0]).atan2(r[0][0]), 0.0)
        };
        let wrap = |radians: f32| {
            let d = radians.to_degrees();
            let d = d % 360.0;
            if d < 0.0 { d + 360.0 } else { d }
        };
        Vec3::new(wrap(x), wrap(y), wrap(z))
    }

    /// `Quaternion.FromToRotation`: the shortest rotation taking direction `from` to direction `to`.
    pub fn from_to(from: Vec3, to: Vec3) -> Quat {
        let (a, b) = (from.normalized(), to.normalized());
        if a == Vec3::ZERO || b == Vec3::ZERO {
            return Quat::IDENTITY;
        }
        let d = a.dot(b);
        if d > 0.999_999 {
            return Quat::IDENTITY;
        }
        if d < -0.999_999 {
            // Opposite directions: any perpendicular axis.
            let axis = if a.x.abs() < 0.9 { Vec3::RIGHT.cross(a) } else { Vec3::UP.cross(a) }.normalized();
            return Quat::axis_angle(axis, std::f32::consts::PI);
        }
        let c = a.cross(b);
        Quat::new(c.x, c.y, c.z, 1.0 + d).normalized()
    }

    /// `Quaternion.LookRotation(forward, up)`.
    pub fn look_rotation(forward: Vec3, up: Vec3) -> Quat {
        let f = forward.normalized();
        if f == Vec3::ZERO {
            return Quat::IDENTITY;
        }
        let mut r = up.cross(f).normalized();
        if r == Vec3::ZERO {
            // `up` parallel to `forward`: pick any perpendicular, as Unity does.
            r = if f.x.abs() < 0.9 { Vec3::RIGHT.cross(f).normalized() } else { Vec3::UP.cross(f).normalized() };
        }
        let u = f.cross(r);
        // Columns r, u, f.
        let (m00, m01, m02) = (r.x, u.x, f.x);
        let (m10, m11, m12) = (r.y, u.y, f.y);
        let (m20, m21, m22) = (r.z, u.z, f.z);
        let trace = m00 + m11 + m22;
        let q = if trace > 0.0 {
            let s = (trace + 1.0).sqrt() * 2.0;
            Quat::new((m21 - m12) / s, (m02 - m20) / s, (m10 - m01) / s, 0.25 * s)
        } else if m00 > m11 && m00 > m22 {
            let s = (1.0 + m00 - m11 - m22).sqrt() * 2.0;
            Quat::new(0.25 * s, (m01 + m10) / s, (m02 + m20) / s, (m21 - m12) / s)
        } else if m11 > m22 {
            let s = (1.0 + m11 - m00 - m22).sqrt() * 2.0;
            Quat::new((m01 + m10) / s, 0.25 * s, (m12 + m21) / s, (m02 - m20) / s)
        } else {
            let s = (1.0 + m22 - m00 - m11).sqrt() * 2.0;
            Quat::new((m02 + m20) / s, (m12 + m21) / s, 0.25 * s, (m10 - m01) / s)
        };
        q.normalized()
    }
}

impl Mul for Quat {
    type Output = Quat;
    /// Hamilton product as Unity defines it: `(a * b)` rotates by `b` first, then `a`.
    fn mul(self, b: Quat) -> Quat {
        Quat::new(
            self.w * b.x + self.x * b.w + self.y * b.z - self.z * b.y,
            self.w * b.y - self.x * b.z + self.y * b.w + self.z * b.x,
            self.w * b.z + self.x * b.y - self.y * b.x + self.z * b.w,
            self.w * b.w - self.x * b.x - self.y * b.y - self.z * b.z,
        )
    }
}

impl Mul<Vec3> for Quat {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        self.rotate(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).magnitude() < 1e-5
    }

    #[test]
    fn yaw_90_maps_forward_to_right_like_unity() {
        let q = Quat::euler_y_degrees(90.0);
        assert!(close(q * Vec3::FORWARD, Vec3::RIGHT));
    }

    #[test]
    fn product_applies_right_hand_side_first() {
        let q = Quat::about_y(std::f32::consts::FRAC_PI_2) * Quat::about_x(std::f32::consts::FRAC_PI_2);
        // x-rotation takes up→forward, then the y-rotation takes forward→right.
        assert!(close(q * Vec3::UP, Vec3::RIGHT));
    }

    #[test]
    fn look_rotation_points_forward_with_up_hint() {
        let q = Quat::look_rotation(Vec3::new(1.0, 0.0, 0.0), Vec3::UP);
        assert!(close(q * Vec3::FORWARD, Vec3::RIGHT));
        assert!(close(q * Vec3::UP, Vec3::UP));
        let q = Quat::look_rotation(Vec3::new(0.0, 1.0, 0.0), Vec3::BACK);
        assert!(close(q * Vec3::FORWARD, Vec3::UP));
        assert!(close(q * Vec3::UP, Vec3::BACK), "the up hint ends up on the local up axis");
    }

    #[test]
    fn from_to_takes_one_direction_onto_another() {
        for (a, b) in [
            (Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)),
            (Vec3::new(1.0, 2.0, 3.0), Vec3::new(-2.0, 0.5, 1.0)),
            (Vec3::new(0.0, 1.0, 0.0), Vec3::new(0.0, -1.0, 0.0)),
            (Vec3::new(0.0, 0.0, 1.0), Vec3::new(0.0, 0.0, 1.0)),
        ] {
            let q = Quat::from_to(a, b);
            assert!(close(q * a.normalized(), b.normalized()), "{a:?} -> {b:?}");
        }
    }

    #[test]
    fn euler_angles_round_trip_unity_order() {
        // Rotation z, then x, then y.
        let q = Quat::about_y(0.7) * Quat::about_x(0.3) * Quat::about_z(1.1);
        let e = q.euler_angles_degrees();
        assert!((e.x - 0.3f32.to_degrees()).abs() < 1e-3, "{e:?}");
        assert!((e.y - 0.7f32.to_degrees()).abs() < 1e-3, "{e:?}");
        assert!((e.z - 1.1f32.to_degrees()).abs() < 1e-3, "{e:?}");
        let left = Quat::euler_y_degrees(-90.0).euler_angles_degrees();
        assert!((left.y - 270.0).abs() < 1e-3, "{left:?}");
    }

    #[test]
    fn nlerp_takes_the_short_arc() {
        let a = Quat::IDENTITY;
        let b = Quat::new(0.0, 0.0, 0.0, -1.0);
        let mid = Quat::nlerp(a, b, 0.5);
        assert!(mid.w.abs() > 0.99);
    }
}
