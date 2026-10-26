//! Random numbers for gameplay decisions (spawn point choice, particle velocities).
//!
//! The original draws from `UnityEngine.Random`, which Unity seeds from the clock at startup, so
//! no recording can ever depend on its exact stream. This is a small xorshift128 generator with
//! the same call shapes (`Range(int, int)`, `value`), seedable so tests are deterministic.
//! TODO(UNCERTAIN): bit-exactness with Unity's generator is neither possible to verify nor needed.

#[derive(Clone, Debug)]
pub struct GameRng {
    s: [u32; 4],
}

impl GameRng {
    /// Seed with Unity's `Random.InitState`-style linear congruential expansion.
    pub fn from_seed(seed: u32) -> GameRng {
        let mut s = [seed, 0, 0, 0];
        for i in 1..4 {
            s[i] = 0x6C07_8965u32.wrapping_mul(s[i - 1]).wrapping_add(i as u32);
        }
        GameRng { s }
    }

    fn next_u32(&mut self) -> u32 {
        let t = self.s[0] ^ (self.s[0] << 11);
        self.s[0] = self.s[1];
        self.s[1] = self.s[2];
        self.s[2] = self.s[3];
        self.s[3] = (self.s[3] ^ (self.s[3] >> 19)) ^ (t ^ (t >> 8));
        self.s[3]
    }

    /// `Random.value`: uniform in [0, 1].
    pub fn value(&mut self) -> f32 {
        (self.next_u32() & 0x7F_FFFF) as f32 / 8_388_607.0
    }

    /// `Random.Range(min, max)` for floats: both ends inclusive.
    /// TODO(UNCERTAIN): how Unity maps its uniform draw onto the range is not recovered.
    pub fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        let t = self.value();
        min + (max - min) * t
    }

    /// `Random.insideUnitCircle`: a point uniformly inside the unit circle.
    /// TODO(UNCERTAIN): Unity's native algorithm (and how many draws it takes) is not recovered;
    /// this takes a radius `sqrt(value)` and an angle `value * 2π`.
    pub fn inside_unit_circle(&mut self) -> (f32, f32) {
        let r = self.value().sqrt();
        let angle = self.value() * std::f32::consts::TAU;
        (r * angle.cos(), r * angle.sin())
    }

    /// `Random.Range(min, max)` for integers: `min` inclusive, `max` exclusive.
    pub fn range_int(&mut self, min: i32, max: i32) -> i32 {
        if max <= min {
            return min;
        }
        min + (self.next_u32() % (max - min) as u32) as i32
    }
}

impl Default for GameRng {
    fn default() -> Self {
        GameRng::from_seed(0x5EED_5EED)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_is_half_open_and_deterministic() {
        let mut a = GameRng::from_seed(7);
        let mut b = GameRng::from_seed(7);
        for _ in 0..1000 {
            let x = a.range_int(0, 5);
            assert!((0..5).contains(&x));
            assert_eq!(x, b.range_int(0, 5));
        }
    }

    #[test]
    fn empty_range_returns_min() {
        assert_eq!(GameRng::default().range_int(3, 3), 3);
    }

    #[test]
    fn value_is_in_unit_interval() {
        let mut r = GameRng::default();
        for _ in 0..1000 {
            let v = r.value();
            assert!((0.0..=1.0).contains(&v));
        }
    }
}
