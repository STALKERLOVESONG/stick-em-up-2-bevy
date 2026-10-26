//! Frame/fixed-step timing as observed in the original game.
//!
//! Evidence:
//! * `ProjectSettings/TimeManager.asset`: fixed timestep 0.016666668, maximum allowed
//!   timestep 0.33333334, time scale 1.
//! * `FixedUpdateSynchronizer.FixedUpdate` rewrites `Time.fixedDeltaTime` on every physics
//!   callback to `min(unscaledDeltaTime * timeScale, 1/15)`. In steady state the fixed step
//!   therefore tracks the *frame* time instead of staying at 1/60 s.
//!
//! TODO(UNCERTAIN): whether the physics step that follows a `FixedUpdate` uses the value written
//! by that same callback or the previous one is not recorded in the evidence. This implementation
//! uses the previous value (the callback's write takes effect on the next step); in steady
//! state both agree to within one frame-time jitter.

/// Values from `TimeManager.asset`.
pub const FIXED_TIMESTEP: f32 = 0.016_666_668;
pub const MAXIMUM_ALLOWED_TIMESTEP: f32 = 0.333_333_34;
/// Upper bound applied by `FixedUpdateSynchronizer`.
pub const SYNCHRONIZED_FIXED_DELTA_MAX: f32 = 1.0 / 15.0;
/// Unity refuses to run fixed steps with a smaller interval than this.
const MINIMUM_FIXED_DELTA: f32 = 0.0001;

/// What `Time.*` reports during a frame's `Update`/`LateUpdate`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameTime {
    /// `Time.deltaTime` (scaled, clamped to the maximum allowed timestep).
    pub delta: f32,
    /// `Time.unscaledDeltaTime`.
    pub unscaled_delta: f32,
    /// `Time.timeScale`.
    pub time_scale: f32,
    /// `Time.frameCount`.
    pub frame_count: u64,
}

impl Default for FrameTime {
    fn default() -> Self {
        Self { delta: 0.0, unscaled_delta: 0.0, time_scale: 1.0, frame_count: 0 }
    }
}

/// Drives the variable number of fixed steps per rendered frame.
#[derive(Clone, Debug)]
pub struct FixedStepper {
    /// `Time.fixedDeltaTime`.
    pub fixed_delta: f32,
    accumulator: f32,
    /// `Time.fixedTime`.
    pub fixed_time: f32,
    frame: FrameTime,
}

impl Default for FixedStepper {
    fn default() -> Self {
        Self::new()
    }
}

impl FixedStepper {
    pub fn new() -> Self {
        Self { fixed_delta: FIXED_TIMESTEP, accumulator: 0.0, fixed_time: 0.0, frame: FrameTime::default() }
    }

    /// Current frame timing (as seen by `Update`).
    pub fn frame(&self) -> FrameTime {
        self.frame
    }

    pub fn set_time_scale(&mut self, time_scale: f32) {
        self.frame.time_scale = time_scale;
    }

    /// Start a rendered frame with the real elapsed time. Returns the frame timing for `Update`.
    pub fn begin_frame(&mut self, real_delta: f32) -> FrameTime {
        let unscaled = real_delta.min(MAXIMUM_ALLOWED_TIMESTEP);
        self.frame.unscaled_delta = unscaled;
        self.frame.delta = unscaled * self.frame.time_scale;
        self.frame.frame_count += 1;
        self.accumulator += self.frame.delta;
        self.frame
    }

    /// Pop the next fixed step for this frame, if any. The returned value is `Time.deltaTime`
    /// during that step's `FixedUpdate` and the physics step length.
    pub fn next_step(&mut self) -> Option<f32> {
        let step = self.fixed_delta.max(MINIMUM_FIXED_DELTA);
        if self.accumulator < step {
            return None;
        }
        self.accumulator -= step;
        self.fixed_time += step;
        Some(step)
    }

    /// `FixedUpdateSynchronizer.FixedUpdate`; call once per executed fixed step.
    pub fn synchronize_fixed_delta(&mut self) {
        self.fixed_delta = (self.frame.unscaled_delta * self.frame.time_scale).min(SYNCHRONIZED_FIXED_DELTA_MAX);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps(s: &mut FixedStepper, real: f32) -> Vec<f32> {
        s.begin_frame(real);
        let mut out = Vec::new();
        while let Some(dt) = s.next_step() {
            out.push(dt);
            s.synchronize_fixed_delta();
        }
        out
    }

    #[test]
    fn frame_delta_is_clamped_to_maximum_allowed_timestep() {
        let mut s = FixedStepper::new();
        let f = s.begin_frame(5.0);
        assert_eq!(f.delta, MAXIMUM_ALLOWED_TIMESTEP);
        assert_eq!(f.frame_count, 1);
    }

    #[test]
    fn steady_sixty_hz_runs_one_step_per_frame_of_frame_length() {
        let mut s = FixedStepper::new();
        let dt = 1.0 / 60.0;
        // First frame uses the 1/60 default; afterwards fixed_delta tracks the frame time.
        let mut total_steps = 0;
        for _ in 0..120 {
            let v = steps(&mut s, dt);
            total_steps += v.len();
            assert!(v.len() <= 2);
        }
        assert!((119..=121).contains(&total_steps), "{total_steps}");
        assert!((s.fixed_delta - dt).abs() < 1e-6);
    }

    #[test]
    fn long_hitch_is_bounded_by_synchronized_fixed_delta() {
        let mut s = FixedStepper::new();
        steps(&mut s, 1.0 / 60.0);
        // A 0.3 s hitch: fixed_delta becomes min(0.3, 1/15) after the first step, so the
        // frame runs a handful of steps rather than 18 steps of 1/60.
        let v = steps(&mut s, 0.3);
        assert!(!v.is_empty());
        assert!(v.len() < 18);
        assert!(s.fixed_delta <= SYNCHRONIZED_FIXED_DELTA_MAX + 1e-6);
    }

    #[test]
    fn paused_time_scale_runs_no_steps() {
        let mut s = FixedStepper::new();
        s.set_time_scale(0.0);
        assert!(steps(&mut s, 1.0 / 60.0).is_empty());
    }
}
