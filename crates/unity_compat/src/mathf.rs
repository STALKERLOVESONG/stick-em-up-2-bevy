//! `Mathf` helpers and `SmoothDamp`, with Unity's exact edge-case behavior.

/// Minimal 2D vector used by the numeric helpers (kept engine-agnostic so the crate has no
/// dependencies). All math is `f32`, as in the original.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn dot(self, o: Vec2) -> f32 {
        self.x * o.x + self.y * o.y
    }

    /// `Vector2.sqrMagnitude`.
    #[inline]
    pub fn sqr_magnitude(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    /// `Vector2.magnitude`.
    #[inline]
    pub fn magnitude(self) -> f32 {
        self.sqr_magnitude().sqrt()
    }

    /// Perpendicular (rotated +90 degrees).
    #[inline]
    pub fn perp(self) -> Vec2 {
        Vec2::new(-self.y, self.x)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, k: f32) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, k: f32) -> Vec2 {
        Vec2::new(self.x / k, self.y / k)
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

impl std::ops::AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, o: Vec2) {
        self.x += o.x;
        self.y += o.y;
    }
}

impl std::ops::SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, o: Vec2) {
        self.x -= o.x;
        self.y -= o.y;
    }
}

/// `Mathf.Sign`: unlike `f32::signum`, zero (including negative zero) maps to `+1`.
#[inline]
pub fn sign(f: f32) -> f32 {
    if f >= 0.0 { 1.0 } else { -1.0 }
}

// `Mathf`'s transcendental functions are `(float)Math.F((double)f)`: computed in double and
// rounded once, which can differ in the last ulp from the single-precision functions of `f32`.

/// `Mathf.Sqrt`.
#[inline]
pub fn sqrt(f: f32) -> f32 {
    (f as f64).sqrt() as f32
}

/// `Mathf.Sin`.
#[inline]
pub fn sin(f: f32) -> f32 {
    (f as f64).sin() as f32
}

/// `Mathf.Cos`.
#[inline]
pub fn cos(f: f32) -> f32 {
    (f as f64).cos() as f32
}

/// `Mathf.Atan`.
#[inline]
pub fn atan(f: f32) -> f32 {
    (f as f64).atan() as f32
}

/// `Mathf.Acos` (NaN outside -1..=1, like `Math.Acos`).
#[inline]
pub fn acos(f: f32) -> f32 {
    (f as f64).acos() as f32
}

/// `(float)Math.PI`.
pub const PI: f32 = std::f64::consts::PI as f32;

/// `Mathf.Clamp(value, min, max)` for floats.
#[inline]
pub fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

/// `Mathf.Clamp01`.
#[inline]
pub fn clamp01(value: f32) -> f32 {
    if value < 0.0 {
        0.0
    } else if value > 1.0 {
        1.0
    } else {
        value
    }
}

/// `Mathf.Lerp(a, b, t)`: `a + (b - a) * Clamp01(t)`.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * clamp01(t)
}

/// `Mathf.MoveTowards`.
#[inline]
pub fn move_towards(current: f32, target: f32, max_delta: f32) -> f32 {
    if (target - current).abs() <= max_delta {
        target
    } else {
        current + sign(target - current) * max_delta
    }
}

/// `Mathf.Repeat(t, length)`: `Clamp(t - Floor(t / length) * length, 0, length)`.
pub fn repeat(t: f32, length: f32) -> f32 {
    clamp(t - (t / length).floor() * length, 0.0, length)
}

/// `Mathf.DeltaAngle(current, target)`: the shortest difference in degrees.
pub fn delta_angle(current: f32, target: f32) -> f32 {
    let mut num = repeat(target - current, 360.0);
    if num > 180.0 {
        num -= 360.0;
    }
    num
}

/// `Mathf.SmoothDampAngle(current, target, ref velocity, smoothTime, maxSpeed, deltaTime)`.
pub fn smooth_damp_angle(current: f32, target: f32, velocity: &mut f32, smooth_time: f32, max_speed: f32, delta_time: f32) -> f32 {
    let target = current + delta_angle(current, target);
    smooth_damp(current, target, velocity, smooth_time, max_speed, delta_time)
}

/// `Mathf.SmoothDamp(current, target, ref velocity, smoothTime, maxSpeed, deltaTime)`.
///
/// TODO(UNCERTAIN): transcribed from the publicly documented Unity reference implementation;
/// the 5.1.3 native build may differ in the last ulp. Verify against original-runtime traces.
pub fn smooth_damp(
    current: f32,
    target: f32,
    velocity: &mut f32,
    smooth_time: f32,
    max_speed: f32,
    delta_time: f32,
) -> f32 {
    let smooth_time = smooth_time.max(0.0001);
    let omega = 2.0 / smooth_time;
    let x = omega * delta_time;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let mut change = current - target;
    let original_to = target;
    let max_change = max_speed * smooth_time;
    change = clamp(change, -max_change, max_change);
    let target = current - change;
    let temp = (*velocity + omega * change) * delta_time;
    *velocity = (*velocity - omega * temp) * exp;
    let mut output = target + (change + temp) * exp;
    if (original_to - current > 0.0) == (output > original_to) {
        output = original_to;
        *velocity = (output - original_to) / delta_time;
    }
    output
}

/// `Vector2.SmoothDamp(current, target, ref velocity, smoothTime, maxSpeed, deltaTime)`.
///
/// TODO(UNCERTAIN): see [`smooth_damp`]; the 2D variant clamps the *length* of the change, not
/// each component, which matters only when `max_speed` is finite (the game always passes
/// infinity).
pub fn smooth_damp_vec2(
    current: Vec2,
    target: Vec2,
    velocity: &mut Vec2,
    smooth_time: f32,
    max_speed: f32,
    delta_time: f32,
) -> Vec2 {
    let smooth_time = smooth_time.max(0.0001);
    let omega = 2.0 / smooth_time;
    let x = omega * delta_time;
    let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
    let mut change_x = current.x - target.x;
    let mut change_y = current.y - target.y;
    let original_to = target;
    let max_change = max_speed * smooth_time;
    let max_change_sq = max_change * max_change;
    let sqr_mag = change_x * change_x + change_y * change_y;
    if sqr_mag > max_change_sq {
        let mag = sqr_mag.sqrt();
        change_x = change_x / mag * max_change;
        change_y = change_y / mag * max_change;
    }
    let target_x = current.x - change_x;
    let target_y = current.y - change_y;
    let temp_x = (velocity.x + omega * change_x) * delta_time;
    let temp_y = (velocity.y + omega * change_y) * delta_time;
    velocity.x = (velocity.x - omega * temp_x) * exp;
    velocity.y = (velocity.y - omega * temp_y) * exp;
    let mut out_x = target_x + (change_x + temp_x) * exp;
    let mut out_y = target_y + (change_y + temp_y) * exp;
    let orig_minus_current_x = original_to.x - current.x;
    let orig_minus_current_y = original_to.y - current.y;
    let out_minus_orig_x = out_x - original_to.x;
    let out_minus_orig_y = out_y - original_to.y;
    if orig_minus_current_x * out_minus_orig_x + orig_minus_current_y * out_minus_orig_y > 0.0 {
        out_x = original_to.x;
        out_y = original_to.y;
        velocity.x = (out_x - original_to.x) / delta_time;
        velocity.y = (out_y - original_to.y) / delta_time;
    }
    Vec2::new(out_x, out_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_of_zero_is_positive() {
        assert_eq!(sign(0.0), 1.0);
        assert_eq!(sign(-0.0), 1.0);
        assert_eq!(sign(-3.0), -1.0);
    }

    #[test]
    fn move_towards_clamps_to_target() {
        assert_eq!(move_towards(1.0, 0.0, 10.0), 0.0);
        assert_eq!(move_towards(1.0, 0.0, 0.25), 0.75);
        assert_eq!(move_towards(-1.0, 0.0, 0.25), -0.75);
    }

    #[test]
    fn smooth_damp_converges_without_overshoot() {
        let mut v = 0.0;
        let mut x = 0.0;
        for _ in 0..600 {
            let next = smooth_damp(x, 10.0, &mut v, 0.2, f32::INFINITY, 1.0 / 60.0);
            assert!(next <= 10.0 + 1e-4, "overshoot: {next}");
            x = next;
        }
        assert!((x - 10.0).abs() < 1e-3);
    }

    #[test]
    fn smooth_damp_first_step_matches_hand_computation() {
        // omega = 10, x = omega*dt = 1/6, exp = 1/(1+x+0.48x^2+0.235x^3)
        let dt = 1.0f32 / 60.0;
        let mut v = 0.0;
        let out = smooth_damp(0.0, 1.0, &mut v, 0.2, f32::INFINITY, dt);
        let x = 10.0 * dt;
        let exp = 1.0 / (1.0 + x + 0.48 * x * x + 0.235 * x * x * x);
        let change = -1.0f32;
        let temp = (0.0 + 10.0 * change) * dt;
        let expected = (0.0 - change) + (change + temp) * exp;
        assert!((out - expected).abs() < 1e-6, "{out} vs {expected}");
    }

    #[test]
    fn smooth_damp_vec2_matches_scalar_when_axis_aligned() {
        let mut vs = 0.0;
        let mut vv = Vec2::ZERO;
        let mut xs = 0.0f32;
        let mut xv = Vec2::ZERO;
        for _ in 0..30 {
            xs = smooth_damp(xs, 5.0, &mut vs, 0.2, f32::INFINITY, 1.0 / 60.0);
            xv = smooth_damp_vec2(xv, Vec2::new(5.0, 0.0), &mut vv, 0.2, f32::INFINITY, 1.0 / 60.0);
            assert!((xs - xv.x).abs() < 1e-5);
            assert_eq!(xv.y, 0.0);
        }
    }
}
