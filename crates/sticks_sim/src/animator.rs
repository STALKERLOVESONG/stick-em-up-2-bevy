//! Animator state machine of the `Character` controller, reduced to what is observable in
//! gameplay: which state is active (tags, ducking), humanoid root motion, and the parameters that
//! clips animate (`Collider Height Fraction`).
//!
//! Rendering never reads this module's internals: the game layer may sample poses however it
//! likes, but ground speed, collider height and airborne detection come from here, exactly as
//! the original's `OnAnimatorMove` consumed them.
//!
//! Unity's Mecanim is replaced by the smallest model that reproduces the recovered controller:
//! normalized-time states, 1D blend trees, linear cross-fades, exit-time transitions with
//! conditions, destination offsets and interruption. Everything not settled by the data is marked
//! `TODO(UNCERTAIN)`.

use std::collections::HashMap;
use std::sync::Arc;

use crate::pose::{ClipPose, HumanPose};
use unity_data::anim::{CharacterAnimationData, Clip, Condition, ConditionMode, Controller, Interruption, Motion, Transition};

/// Index of a controller parameter (see [`Animator::param`]).
pub type ParamId = usize;

#[derive(Clone, Debug)]
enum ResolvedMotion {
    /// Missing clip or no motion; the state still exists and plays for one second per loop.
    None,
    Clip(usize),
    Blend { parameter: ParamId, children: Vec<BlendChild> },
}

#[derive(Clone, Copy, Debug)]
struct BlendChild {
    clip: usize,
    threshold: f32,
    time_scale: f32,
}

/// Immutable animation data shared by every character: the controller and its clips.
#[derive(Debug)]
pub struct AnimatorModel {
    controller: Controller,
    clips: Vec<Clip>,
    /// Humanoid curves of each clip, compiled for pose sampling.
    poses: Vec<ClipPose>,
    motions: Vec<ResolvedMotion>,
    human_scale: f32,
    /// Parameters that at least one clip animates through a parameter curve.
    animated_params: Vec<ParamId>,
}

impl AnimatorModel {
    /// Index of the controller's default state (`Idle`).
    pub fn default_state(&self) -> usize {
        self.controller.default_state
    }

    pub fn new(data: CharacterAnimationData) -> Arc<AnimatorModel> {
        let CharacterAnimationData { controller, clips: clip_map, human_scale } = data;
        let mut clips = Vec::new();
        let mut index_of: HashMap<String, usize> = HashMap::new();
        for (guid, clip) in clip_map {
            index_of.insert(guid, clips.len());
            clips.push(clip);
        }
        let param_of = |name: &str| controller.parameters.iter().position(|p| p.name == name);
        let motions: Vec<ResolvedMotion> = controller
            .states
            .iter()
            .map(|state| match &state.motion {
                Motion::None => ResolvedMotion::None,
                Motion::Clip(guid) => index_of.get(guid).map_or(ResolvedMotion::None, |&c| ResolvedMotion::Clip(c)),
                Motion::Blend { parameter, children } => {
                    let Some(parameter) = param_of(parameter) else { return ResolvedMotion::None };
                    let mut resolved: Vec<BlendChild> = children
                        .iter()
                        .filter_map(|c| {
                            Some(BlendChild { clip: *index_of.get(&c.clip_guid)?, threshold: c.threshold, time_scale: c.time_scale })
                        })
                        .collect();
                    resolved.sort_by(|a, b| a.threshold.total_cmp(&b.threshold));
                    if resolved.is_empty() { ResolvedMotion::None } else { ResolvedMotion::Blend { parameter, children: resolved } }
                }
            })
            .collect();
        let animated_params = controller
            .parameters
            .iter()
            .enumerate()
            .filter(|(_, p)| clips.iter().any(|c| c.parameter_curve(&p.name).is_some()))
            .map(|(i, _)| i)
            .collect();
        let poses = clips.iter().map(ClipPose::compile).collect();
        Arc::new(AnimatorModel { controller, clips, poses, motions, human_scale, animated_params })
    }

    pub fn param(&self, name: &str) -> Option<ParamId> {
        self.controller.parameters.iter().position(|p| p.name == name)
    }

    pub fn state_index(&self, name: &str) -> Option<usize> {
        self.controller.state_index(name)
    }

    pub fn state_name(&self, state: usize) -> &str {
        &self.controller.states[state].name
    }

    pub fn state_tag(&self, state: usize) -> &str {
        &self.controller.states[state].tag
    }

    pub fn human_scale(&self) -> f32 {
        self.human_scale
    }
}

/// Blend weights of a 1D blend tree for parameter value `p`: (child index, weight) pairs.
fn blend_weights(children: &[BlendChild], p: f32) -> [(usize, f32); 2] {
    let first = children[0].threshold;
    let last = children[children.len() - 1].threshold;
    let p = p.clamp(first, last);
    let hi = children.iter().position(|c| c.threshold >= p).unwrap_or(children.len() - 1);
    if hi == 0 {
        return [(0, 1.0), (0, 0.0)];
    }
    let lo = hi - 1;
    let span = children[hi].threshold - children[lo].threshold;
    let w_hi = if span > 0.0 { (p - children[lo].threshold) / span } else { 1.0 };
    [(lo, 1.0 - w_hi), (hi, w_hi)]
}

#[derive(Clone, Copy, Debug)]
struct Playhead {
    state: usize,
    /// Normalized time, unbounded (loops keep counting).
    time: f32,
}

#[derive(Clone, Copy, Debug)]
struct Fade {
    next: Playhead,
    /// State the running transition leaves.
    source_state: usize,
    interruption: Interruption,
    elapsed: f32,
    duration: f32,
}

/// What one animator update produced.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AnimatorStep {
    /// `Animator.deltaPosition` in the character's local space (x right, y up, z forward),
    /// already multiplied by the avatar's human scale.
    pub delta_position: [f32; 3],
}

/// Per-character animator instance.
#[derive(Clone, Debug)]
pub struct Animator {
    model: Arc<AnimatorModel>,
    params: Vec<f32>,
    current: Playhead,
    fade: Option<Fade>,
}

impl Animator {
    pub fn new(model: Arc<AnimatorModel>) -> Animator {
        let params = model.controller.parameters.iter().map(|p| p.default).collect();
        let current = Playhead { state: model.controller.default_state, time: 0.0 };
        Animator { model, params, current, fade: None }
    }

    pub fn model(&self) -> &AnimatorModel {
        &self.model
    }

    /// Fresh start, as after the owning object is deactivated and enabled again: default state,
    /// default parameters, no transition in flight.
    pub fn restart(&mut self) {
        *self = Animator::new(self.model.clone());
    }

    pub fn set_float(&mut self, id: ParamId, value: f32) {
        self.params[id] = value;
    }

    pub fn set_bool(&mut self, id: ParamId, value: bool) {
        self.params[id] = if value { 1.0 } else { 0.0 };
    }

    pub fn float(&self, id: ParamId) -> f32 {
        self.params[id]
    }

    pub fn current_state(&self) -> usize {
        self.current.state
    }

    /// Destination state while a cross-fade is running (`GetNextAnimatorStateInfo`).
    pub fn next_state(&self) -> Option<usize> {
        self.fade.map(|f| f.next.state)
    }

    pub fn current_tag(&self) -> &str {
        self.model.state_tag(self.current.state)
    }

    pub fn next_tag(&self) -> Option<&str> {
        self.next_state().map(|s| self.model.state_tag(s))
    }

    /// Whether `state` is current or the destination of the running transition.
    pub fn is_in_or_entering(&self, state: usize) -> bool {
        self.current.state == state || self.next_state() == Some(state)
    }

    /// Normalized time and state of the active playheads, for pose sampling by renderers:
    /// `(state, normalized time, weight)`.
    pub fn playheads(&self) -> [(usize, f32, f32); 2] {
        match self.fade {
            Some(f) => {
                let w = fade_weight(&f);
                [(self.current.state, self.current.time, 1.0 - w), (f.next.state, f.next.time, w)]
            }
            None => [(self.current.state, self.current.time, 1.0), (self.current.state, self.current.time, 0.0)],
        }
    }

    /// The humanoid pose of the current state or cross-fade: each playhead's motion sampled at
    /// its normalized time (blend trees mix their two neighbouring clips by parameter), then the
    /// two playheads mixed by the transition weight.
    pub fn pose(&self) -> HumanPose {
        let model = &self.model;
        let state_pose = |state: usize, time: f32| -> HumanPose {
            match &model.motions[state] {
                ResolvedMotion::None => HumanPose::default(),
                ResolvedMotion::Clip(c) => model.poses[*c].sample(time),
                ResolvedMotion::Blend { parameter, children } => {
                    let weights = blend_weights(children, self.params[*parameter]);
                    let lo = model.poses[children[weights[0].0].clip].sample(time);
                    let hi = model.poses[children[weights[1].0].clip].sample(time);
                    HumanPose::blend(&[(&lo, weights[0].1), (&hi, weights[1].1)])
                }
            }
        };
        let [(s0, t0, w0), (s1, t1, w1)] = self.playheads();
        let first = state_pose(s0, t0);
        if w1 > 0.0 {
            let second = state_pose(s1, t1);
            HumanPose::blend(&[(&first, w0), (&second, w1)])
        } else {
            first
        }
    }

    fn motion_duration(&self, state: usize) -> f32 {
        match &self.model.motions[state] {
            ResolvedMotion::None => 1.0,
            ResolvedMotion::Clip(c) => self.model.clips[*c].length,
            ResolvedMotion::Blend { parameter, children } => {
                // TODO(UNCERTAIN): Unity's blend-tree duration blending. Weighting each child's
                // duration by its blend weight is assumed; keyboard play only ever reaches
                // weights of exactly 0 or 1, where every plausible rule agrees.
                blend_weights(children, self.params[*parameter])
                    .iter()
                    .map(|&(i, w)| w * (self.model.clips[children[i].clip].length / children[i].time_scale))
                    .sum()
            }
        }
    }

    /// Normalized time per second.
    fn rate(&self, state: usize) -> f32 {
        let duration = self.motion_duration(state);
        if duration <= 0.0 { 0.0 } else { self.model.controller.states[state].speed / duration }
    }

    fn condition_holds(&self, c: &Condition) -> bool {
        let Some(id) = self.model.param(&c.parameter) else { return false };
        let v = self.params[id];
        match c.mode {
            ConditionMode::If => v != 0.0,
            ConditionMode::IfNot => v == 0.0,
            ConditionMode::Greater => v > c.threshold,
            ConditionMode::Less => v < c.threshold,
            ConditionMode::Equals => v == c.threshold,
            ConditionMode::NotEqual => v != c.threshold,
        }
    }

    /// Whether a transition fires during this update; returns the time left in the frame after it
    /// fires (`dt` when it fires immediately).
    fn transition_fires(&self, head: Playhead, t: &Transition, dt: f32) -> Option<f32> {
        if !t.conditions.iter().all(|c| self.condition_holds(c)) {
            return None;
        }
        if !t.has_exit_time {
            return Some(dt);
        }
        let rate = self.rate(head.state);
        let end = head.time + dt * rate;
        if end < t.exit_time {
            return None;
        }
        if head.time < t.exit_time && rate > 0.0 {
            Some(dt - (t.exit_time - head.time) / rate)
        } else {
            Some(dt)
        }
    }

    fn first_firing(&self, head: Playhead, dt: f32) -> Option<(usize, Transition, f32)> {
        let state = &self.model.controller.states[head.state];
        state
            .transitions
            .iter()
            .find_map(|t| self.transition_fires(head, t, dt).map(|left| (head.state, t.clone(), left)))
    }

    fn begin(&mut self, from: usize, t: &Transition) {
        // TODO(UNCERTAIN): non-fixed durations are taken as normalized time of the source state
        // (seconds = duration * length / speed). The serialized values (e.g. 0.326 on the 0.767 s
        // run cycle) are exactly 0.25 s when speed is ignored, so the editor may have authored
        // them without it; the difference only affects how long run→idle blending lasts.
        let source_duration = self.motion_duration(from);
        let seconds = if t.fixed_duration {
            t.duration
        } else {
            let speed = self.model.controller.states[from].speed;
            if speed > 0.0 { t.duration * source_duration / speed } else { 0.0 }
        };
        self.fade = Some(Fade {
            next: Playhead { state: t.destination, time: t.offset },
            source_state: from,
            interruption: t.interruption,
            elapsed: 0.0,
            duration: seconds,
        });
    }

    /// Advance the state machine by `dt` seconds and return the root motion produced.
    pub fn update(&mut self, dt: f32) -> AnimatorStep {
        // 1. Interruption of a running transition.
        if let Some(fade) = self.fade {
            self.try_interrupt(fade, dt);
        }
        // 2. Start a transition from the current state.
        let mut next_dt = 0.0;
        if self.fade.is_none() {
            if let Some((from, t, left)) = self.first_firing(self.current, dt) {
                self.begin(from, &t);
                next_dt = left;
            }
        } else {
            next_dt = dt;
        }

        // 3. Advance playheads, accumulating root motion.
        let model = Arc::clone(&self.model);
        let mut delta = [0.0f32; 3];
        let cur_rate = self.rate(self.current.state);
        let cur_before = self.current.time;
        let cur_after = cur_before + dt * cur_rate;
        self.current.time = cur_after;
        let mut cur_weight = 1.0;
        let mut next_info: Option<(Playhead, f32, f32)> = None; // (playhead after, before time, weight)
        if let Some(mut fade) = self.fade {
            let rate = self.rate(fade.next.state);
            let before = fade.next.time;
            fade.next.time += next_dt * rate;
            fade.elapsed += next_dt;
            let w = fade_weight(&fade);
            cur_weight = 1.0 - w;
            next_info = Some((fade.next, before, w));
            self.fade = Some(fade);
        }
        if cur_weight > 0.0 {
            let d = self.root_delta(self.current.state, cur_before, cur_after);
            for k in 0..3 {
                delta[k] += d[k] * cur_weight;
            }
        }
        if let Some((head, before, w)) = next_info {
            if w > 0.0 {
                let d = self.root_delta(head.state, before, head.time);
                for k in 0..3 {
                    delta[k] += d[k] * w;
                }
            }
        }
        // 4. Finish a completed fade.
        if let Some(f) = self.fade {
            if f.elapsed >= f.duration {
                self.current = f.next;
                self.fade = None;
            }
        }
        self.apply_animated_params();
        let scale = model.human_scale;
        AnimatorStep { delta_position: [delta[0] * scale, delta[1] * scale, delta[2] * scale] }
    }

    fn try_interrupt(&mut self, fade: Fade, dt: f32) {
        if fade.interruption == Interruption::None {
            return;
        }
        let source = Playhead { state: fade.source_state, time: self.current.time };
        let candidates: &[(Playhead, bool)] = match fade.interruption {
            Interruption::None => &[],
            Interruption::Source => &[(source, false)],
            Interruption::Destination => &[(fade.next, true)],
            Interruption::SourceThenDestination => &[(source, false), (fade.next, true)],
            Interruption::DestinationThenSource => &[(fade.next, true), (source, false)],
        };
        for &(head, _) in candidates {
            if let Some((from, t, _)) = self.first_firing(head, dt) {
                // TODO(UNCERTAIN): Unity snapshots the blended pose and fades out of it. Here the
                // interrupted fade collapses onto the playhead that owns the new transition,
                // which keeps its time continuous but drops the other side's weight instantly.
                self.current = head;
                self.fade = None;
                self.begin(from, &t);
                return;
            }
        }
    }

    /// Root translation delta (unscaled) of `state` between two normalized times.
    fn root_delta(&self, state: usize, n0: f32, n1: f32) -> [f32; 3] {
        let clips = &self.model.clips;
        let total = |clip: &Clip, n: f32| -> [f32; 3] {
            let Some(rm) = &clip.root_motion else { return [0.0; 3] };
            let at = |t: f32| [rm.x.evaluate(t), rm.y.evaluate(t), rm.z.evaluate(t)];
            if clip.looping {
                let cycles = n.floor();
                let frac = n - cycles;
                let (start, end, now) = (at(0.0), at(clip.length), at(frac * clip.length));
                [0, 1, 2].map(|k| cycles * (end[k] - start[k]) + now[k])
            } else {
                at(n.clamp(0.0, 1.0) * clip.length)
            }
        };
        let delta_of = |clip: &Clip| {
            let (a, b) = (total(clip, n0), total(clip, n1));
            [b[0] - a[0], b[1] - a[1], b[2] - a[2]]
        };
        match &self.model.motions[state] {
            ResolvedMotion::None => [0.0; 3],
            ResolvedMotion::Clip(c) => delta_of(&clips[*c]),
            ResolvedMotion::Blend { parameter, children } => {
                let mut out = [0.0; 3];
                for (i, w) in blend_weights(children, self.params[*parameter]) {
                    if w == 0.0 {
                        continue;
                    }
                    let d = delta_of(&clips[children[i].clip]);
                    for k in 0..3 {
                        out[k] += d[k] * w;
                    }
                }
                out
            }
        }
    }

    /// Parameters driven by clip curves take the blended curve value of the active states.
    /// TODO(UNCERTAIN): states whose clips lack the curve are assumed to contribute the
    /// parameter's default (1.0 for `Collider Height Fraction`), which is how Unity's
    /// curve-driven parameters are conventionally authored.
    fn apply_animated_params(&mut self) {
        let model = Arc::clone(&self.model);
        for &id in &model.animated_params {
            let name = &model.controller.parameters[id].name;
            let default = model.controller.parameters[id].default;
            let sample = |state: usize, time: f32| -> f32 {
                let value_of = |clip: &Clip| {
                    clip.parameter_curve(name).map_or(default, |curve| {
                        let n = if clip.looping { time - time.floor() } else { time.clamp(0.0, 1.0) };
                        curve.evaluate(n * clip.length)
                    })
                };
                match &model.motions[state] {
                    ResolvedMotion::None => default,
                    ResolvedMotion::Clip(c) => value_of(&model.clips[*c]),
                    ResolvedMotion::Blend { parameter, children } => blend_weights(children, self.params[*parameter])
                        .iter()
                        .map(|&(i, w)| w * value_of(&model.clips[children[i].clip]))
                        .sum(),
                }
            };
            let [(s0, t0, w0), (s1, t1, w1)] = self.playheads();
            self.params[id] = if w1 > 0.0 { sample(s0, t0) * w0 + sample(s1, t1) * w1 } else { sample(s0, t0) };
        }
    }
}

fn fade_weight(f: &Fade) -> f32 {
    if f.duration <= 0.0 { 1.0 } else { (f.elapsed / f.duration).clamp(0.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn model() -> Option<Arc<AnimatorModel>> {
        let root = std::env::var_os("STICK_EXPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
        let data = unity_data::anim::load_character_animation(&root.join("Assets")).ok()?;
        Some(AnimatorModel::new(data))
    }

    struct Rig {
        a: Animator,
        forward: ParamId,
        airborne: ParamId,
        duck: ParamId,
        height: ParamId,
        jump: ParamId,
    }

    fn rig() -> Option<Rig> {
        let m = model()?;
        let (forward, airborne, duck, height, jump) = (
            m.param("Forward Velocity")?,
            m.param("Airborne")?,
            m.param("Duck")?,
            m.param("Collider Height Fraction")?,
            m.param("Jump")?,
        );
        Some(Rig { a: Animator::new(m), forward, airborne, duck, height, jump })
    }

    const DT: f32 = 1.0 / 60.0;

    #[test]
    fn starts_idle_and_standing() {
        let Some(mut r) = rig() else { return };
        let idle = r.a.model().state_index("Idle").unwrap();
        assert_eq!(r.a.current_state(), idle);
        for _ in 0..10 {
            r.a.update(DT);
        }
        assert_eq!(r.a.current_state(), idle);
        assert_eq!(r.a.float(r.height), 1.0);
    }

    #[test]
    fn running_reaches_root_motion_speed_derived_from_the_clip() {
        let Some(mut r) = rig() else { return };
        r.a.set_float(r.forward, 1.0);
        let mut speeds = Vec::new();
        for frame in 0..240 {
            let step = r.a.update(DT);
            if frame >= 120 {
                speeds.push(step.delta_position[2].abs() * 1.301065 / DT);
            }
        }
        let run = r.a.model().state_index("Run Forwards").unwrap();
        assert_eq!(r.a.current_state(), run);
        // Average over whole cycles (0.7667 s / 1.4 speed = 0.5476 s ≈ 32.9 frames); use 120 frames.
        let mean = speeds.iter().sum::<f32>() / speeds.len() as f32;
        // Evidence: Run clip RootT.z advances 2.8723485 over its loop, scale 1.7563329, state
        // speed 1.4, controller constant 1.301065 → 11.985; designed top speed is 11.940628.
        assert!((mean - 11.985).abs() < 0.1, "mean speed {mean}");
        assert!((mean - 11.940_628).abs() / 11.940_628 < 0.006, "within 0.6% of designed top speed: {mean}");
    }

    #[test]
    fn root_motion_ramps_up_during_idle_to_run_crossfade() {
        let Some(mut r) = rig() else { return };
        r.a.set_float(r.forward, 1.0);
        let first = r.a.update(DT).delta_position[2].abs();
        let mut later = 0.0;
        for _ in 0..9 {
            later = r.a.update(DT).delta_position[2].abs();
        }
        assert!(first < later, "{first} !< {later}");
        let run = r.a.model().state_index("Run Forwards").unwrap();
        assert_eq!(r.a.next_state(), Some(run), "idle→run fade lasts 0.25 s (0.025 × 10 s idle clip)");
        for _ in 0..10 {
            r.a.update(DT);
        }
        assert_eq!(r.a.current_state(), run);
        assert_eq!(r.a.next_state(), None);
    }

    #[test]
    fn airborne_tag_follows_airborne_parameter_and_jump_flows_into_falling() {
        let Some(mut r) = rig() else { return };
        r.a.set_bool(r.airborne, true);
        r.a.set_bool(r.jump, true);
        r.a.update(DT);
        assert_eq!(r.a.next_tag(), Some("Airborne"), "idle jump begins immediately");
        for _ in 0..60 {
            r.a.update(DT);
        }
        let falling = r.a.model().state_index("Falling").unwrap();
        assert_eq!(r.a.current_tag(), "Airborne");
        assert!(r.a.is_in_or_entering(falling), "jump exits into falling by exit time");
    }

    #[test]
    fn ducking_shrinks_collider_height_fraction_and_restores_on_release() {
        let Some(mut r) = rig() else { return };
        let ducking = r.a.model().state_index("Ducking").unwrap();
        r.a.set_bool(r.duck, true);
        for _ in 0..60 {
            r.a.update(DT);
        }
        assert_eq!(r.a.current_state(), ducking);
        assert!((r.a.float(r.height) - 0.816_666_66).abs() < 1e-5, "{}", r.a.float(r.height));
        r.a.set_bool(r.duck, false);
        for _ in 0..60 {
            r.a.update(DT);
        }
        assert_eq!(r.a.float(r.height), 1.0);
    }

    #[test]
    fn landing_from_falling_returns_to_idle() {
        let Some(mut r) = rig() else { return };
        r.a.set_bool(r.airborne, true);
        for _ in 0..30 {
            r.a.update(DT);
        }
        assert_eq!(r.a.current_tag(), "Airborne");
        r.a.set_bool(r.airborne, false);
        for _ in 0..30 {
            r.a.update(DT);
        }
        let idle = r.a.model().state_index("Idle").unwrap();
        assert_eq!(r.a.current_state(), idle);
    }
}
