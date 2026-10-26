//! Small affine-transform type for composing Unity `Transform` hierarchies.

/// A 3D affine transform (3x3 linear part + translation), `f32` like Unity's.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    /// Row-major linear part.
    pub m: [[f32; 3]; 3],
    pub t: [f32; 3],
}

impl Affine {
    pub const IDENTITY: Affine = Affine { m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], t: [0.0; 3] };

    /// `T * R * S` for a unit quaternion `(x, y, z, w)`, matching `Transform.localToWorldMatrix`.
    pub fn from_trs(position: [f32; 3], rotation: (f32, f32, f32, f32), scale: [f32; 3]) -> Self {
        let (x, y, z, w) = rotation;
        let r = [
            [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w)],
            [2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w)],
            [2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y)],
        ];
        let mut m = [[0.0; 3]; 3];
        for (row, r_row) in r.iter().enumerate() {
            for col in 0..3 {
                m[row][col] = r_row[col] * scale[col];
            }
        }
        Self { m, t: position }
    }

    /// `self * rhs` (apply `rhs` first, then `self`).
    pub fn mul(&self, rhs: &Affine) -> Affine {
        let mut m = [[0.0; 3]; 3];
        for row in 0..3 {
            for col in 0..3 {
                m[row][col] = self.m[row][0] * rhs.m[0][col] + self.m[row][1] * rhs.m[1][col] + self.m[row][2] * rhs.m[2][col];
            }
        }
        Affine { m, t: self.transform_point(rhs.t) }
    }

    pub fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let mut out = self.t;
        for (row, o) in out.iter_mut().enumerate() {
            *o += self.m[row][0] * p[0] + self.m[row][1] * p[1] + self.m[row][2] * p[2];
        }
        out
    }

    /// Transform a direction (no translation).
    pub fn transform_vector(&self, v: [f32; 3]) -> [f32; 3] {
        let mut out = [0.0; 3];
        for (row, o) in out.iter_mut().enumerate() {
            *o = self.m[row][0] * v[0] + self.m[row][1] * v[1] + self.m[row][2] * v[2];
        }
        out
    }

    /// The inverse transform (identity when singular).
    pub fn inverse(&self) -> Affine {
        let m = &self.m;
        let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
        if det.abs() < 1e-12 {
            return Affine::IDENTITY;
        }
        let inv = [
            [(m[1][1] * m[2][2] - m[1][2] * m[2][1]) / det, (m[0][2] * m[2][1] - m[0][1] * m[2][2]) / det, (m[0][1] * m[1][2] - m[0][2] * m[1][1]) / det],
            [(m[1][2] * m[2][0] - m[1][0] * m[2][2]) / det, (m[0][0] * m[2][2] - m[0][2] * m[2][0]) / det, (m[0][2] * m[1][0] - m[0][0] * m[1][2]) / det],
            [(m[1][0] * m[2][1] - m[1][1] * m[2][0]) / det, (m[0][1] * m[2][0] - m[0][0] * m[2][1]) / det, (m[0][0] * m[1][1] - m[0][1] * m[1][0]) / det],
        ];
        let t = [0, 1, 2].map(|r| -(inv[r][0] * self.t[0] + inv[r][1] * self.t[1] + inv[r][2] * self.t[2]));
        Affine { m: inv, t }
    }

    /// Per-axis lossy scale (column lengths), like `Transform.lossyScale` for non-sheared input.
    pub fn lossy_scale(&self) -> [f32; 3] {
        let mut s = [0.0; 3];
        for (col, o) in s.iter_mut().enumerate() {
            *o = (self.m[0][col].powi(2) + self.m[1][col].powi(2) + self.m[2][col].powi(2)).sqrt();
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yaw_90_degrees_maps_forward_z_to_x() {
        // Quaternion for +90 degrees around Y: (0, sin45, 0, cos45). Unity rotates +z to +x.
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let a = Affine::from_trs([0.0; 3], (0.0, s, 0.0, s), [1.0; 3]);
        let v = a.transform_vector([0.0, 0.0, 1.0]);
        assert!((v[0] - 1.0).abs() < 1e-6 && v[2].abs() < 1e-6);
    }

    #[test]
    fn child_composes_with_parent() {
        let parent = Affine::from_trs([10.0, 0.0, 0.0], (0.0, 0.0, 0.0, 1.0), [2.0, 2.0, 2.0]);
        let child = Affine::from_trs([1.0, 1.0, 0.0], (0.0, 0.0, 0.0, 1.0), [1.0; 3]);
        let world = parent.mul(&child);
        assert_eq!(world.t, [12.0, 2.0, 0.0]);
        assert_eq!(world.lossy_scale(), [2.0, 2.0, 2.0]);
    }
}
