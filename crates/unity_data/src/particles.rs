//! Shuriken particle-system prefabs (Unity 5.1 `ParticleSystem` serialization) and the scripts
//! that draw them in this game.
//!
//! The game never renders particles with Unity's `ParticleSystemRenderer` (it is disabled on
//! every prefab): `ParticleSystemLines` reads the particles in `LateUpdate` and draws each one as
//! a line (size ≠ 0, along `-rotation`) or a dot (size 0) through `LinesRenderer` /
//! `DotsRenderer`, with a shadow, faded by remaining lifetime. Only the prefabs reached from
//! gameplay are read: `Rocket Trail` (`LifetimeParticleSystem`), `Rocket Explosion` and
//! `Firework Explosion 0..3` (`SpawnLocallyOnExplode`).

use std::path::Path;

use crate::anim::{Curve, Keyframe};
use crate::assets::GuidIndex;
use crate::scene::{Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// `ParticleSystem` class id.
pub const PARTICLE_SYSTEM: u32 = 198;

/// `MinMaxCurve` (`minMaxState`: 0 constant, 1 curve, 2 random between two curves, 3 random
/// between two constants).
///
/// Unity 5.1 serializes two constants as the first keys of `minCurve`/`maxCurve` scaled by
/// `scalar` (`Rocket Trail.startSpeed`: scalar 0.5, keys 0 and 1 → 0..0.5).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MinMaxCurve {
    pub state: i64,
    pub scalar: f32,
    pub min_curve: Curve,
    pub max_curve: Curve,
}

impl MinMaxCurve {
    pub fn constant(value: f32) -> MinMaxCurve {
        MinMaxCurve { state: 0, scalar: value, ..Default::default() }
    }

    /// Value at normalized time `t` (system time over duration for start values, particle age
    /// over lifetime for over-lifetime modules) with the random blend `random` in `[0, 1]`.
    pub fn evaluate(&self, t: f32, random: f32) -> f32 {
        match self.state {
            1 => self.scalar * self.max_curve.evaluate(t),
            2 => {
                let (a, b) = (self.min_curve.evaluate(t), self.max_curve.evaluate(t));
                self.scalar * (a + (b - a) * random)
            }
            3 => {
                let (a, b) = (self.min_curve.first_value(), self.max_curve.first_value());
                self.scalar * (a + (b - a) * random)
            }
            _ => self.scalar,
        }
    }

    /// True when the value depends on the random blend.
    pub fn is_random(&self) -> bool {
        matches!(self.state, 2 | 3)
    }
}

/// One `Gradient` (up to 8 colour and 8 alpha keys, times in 1/65535).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gradient {
    pub colors: Vec<(f32, [u8; 3])>,
    pub alphas: Vec<(f32, u8)>,
}

fn sample_keys<const N: usize>(keys: &[(f32, [u8; N])], t: f32) -> [f32; N] {
    let Some(first) = keys.first() else { return [255.0; N] };
    if t <= first.0 {
        return first.1.map(f32::from);
    }
    for w in keys.windows(2) {
        let ((t0, a), (t1, b)) = (w[0], w[1]);
        if t <= t1 {
            let s = if t1 > t0 { (t - t0) / (t1 - t0) } else { 1.0 };
            let mut out = [0.0; N];
            for i in 0..N {
                out[i] = f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * s;
            }
            return out;
        }
    }
    keys.last().unwrap().1.map(f32::from)
}

impl Gradient {
    /// `Gradient.Evaluate` (blend mode: linear), as 8-bit RGBA.
    pub fn evaluate(&self, t: f32) -> [u8; 4] {
        let c = sample_keys(&self.colors, t);
        let alphas: Vec<(f32, [u8; 1])> = self.alphas.iter().map(|&(t, a)| (t, [a])).collect();
        let a = sample_keys(&alphas, t);
        [c[0] as u8, c[1] as u8, c[2] as u8, a[0] as u8]
    }
}

/// `MinMaxGradient` (`minMaxState`: 0 colour, 1 gradient, 2 random between two colours, 3
/// random between two gradients).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MinMaxGradient {
    pub state: i64,
    pub min_color: [u8; 4],
    pub max_color: [u8; 4],
    pub min_gradient: Gradient,
    pub max_gradient: Gradient,
}

fn lerp_rgba(a: [u8; 4], b: [u8; 4], t: f32) -> [u8; 4] {
    let mut out = [0u8; 4];
    for i in 0..4 {
        // `Color32.Lerp`: `(byte)(a + (b - a) * t)`.
        out[i] = (f32::from(a[i]) + (f32::from(b[i]) - f32::from(a[i])) * t) as u8;
    }
    out
}

impl MinMaxGradient {
    pub fn evaluate(&self, t: f32, random: f32) -> [u8; 4] {
        match self.state {
            1 => self.max_gradient.evaluate(t),
            2 => lerp_rgba(self.min_color, self.max_color, random),
            3 => lerp_rgba(self.min_gradient.evaluate(t), self.max_gradient.evaluate(t), random),
            _ => self.max_color,
        }
    }

    pub fn is_random(&self) -> bool {
        matches!(self.state, 2 | 3)
    }
}

/// `ShapeModule.type` values of Unity 5.1 that the game's prefabs use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeKind {
    Sphere,
    SphereShell,
    Hemisphere,
    HemisphereShell,
    Cone,
    Box,
    /// Random point inside a disc in the local XY plane, moving away from the centre.
    Circle,
    /// Random point on the rim of that disc, moving outwards.
    CircleEdge,
    /// Anything else (mesh, cone variants, edge): emitted from the centre along +Z.
    Other(i64),
}

impl ShapeKind {
    fn from_type(t: i64) -> ShapeKind {
        match t {
            0 => ShapeKind::Sphere,
            1 => ShapeKind::SphereShell,
            2 => ShapeKind::Hemisphere,
            3 => ShapeKind::HemisphereShell,
            4 => ShapeKind::Cone,
            5 => ShapeKind::Box,
            10 => ShapeKind::Circle,
            11 => ShapeKind::CircleEdge,
            other => ShapeKind::Other(other),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShapeModule {
    pub enabled: bool,
    pub kind: ShapeKind,
    pub radius: f32,
    /// Degrees.
    pub angle: f32,
    /// Degrees.
    pub arc: f32,
    pub box_size: [f32; 3],
    pub random_direction: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EmissionModule {
    pub enabled: bool,
    /// Particles per second (`m_Type` 0: over time).
    pub rate: MinMaxCurve,
    /// `(time, count)` for the first `m_BurstCount` of `time0..3`/`cnt0..3`.
    pub bursts: Vec<(f32, u32)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ParticleSystemSpec {
    /// `lengthInSec`.
    pub duration: f32,
    pub start_delay: f32,
    /// `speed` (simulation speed).
    pub simulation_speed: f32,
    pub looping: bool,
    pub prewarm: bool,
    pub play_on_awake: bool,
    /// `moveWithTransform`: 1 = local simulation space, 0 = world.
    pub local_space: bool,
    pub start_lifetime: MinMaxCurve,
    pub start_speed: MinMaxCurve,
    pub start_size: MinMaxCurve,
    /// Radians.
    pub start_rotation: MinMaxCurve,
    pub start_color: MinMaxGradient,
    pub gravity_modifier: f32,
    pub max_particles: usize,
    pub shape: ShapeModule,
    pub emission: EmissionModule,
    /// `SizeModule` (multiplier over normalized age) when enabled.
    pub size_over_lifetime: Option<MinMaxCurve>,
    /// `RotationModule` (radians per second) when enabled.
    pub rotation_over_lifetime: Option<MinMaxCurve>,
    /// `ColorModule` (multiplies the start colour) when enabled.
    pub color_over_lifetime: Option<MinMaxGradient>,
}

/// `ParticleSystemLines`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinesDrawing {
    pub line_thickness: f32,
    pub fade_time: f32,
    pub shadow_plane_z: f32,
}

/// A prefab whose root is a particle system drawn by `ParticleSystemLines`.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleEffectSpec {
    pub name: String,
    pub guid: String,
    /// The prefab root's local z (`Pool.Spawn` overrides the position, so this is informative).
    pub prefab_z: f32,
    pub system: ParticleSystemSpec,
    pub lines: Option<LinesDrawing>,
    /// `ParticleRotationFollowsVelocity`.
    pub rotation_follows_velocity: bool,
    /// `DespawnOnParticleSystemDeath` (added at run time by `LifetimeParticleSystem` if missing).
    pub despawn_on_death: bool,
}

fn keyframes(v: Option<&Value>) -> Curve {
    let keys = v
        .and_then(|c| c.get("m_Curve"))
        .map(|seq| {
            seq.as_seq()
                .iter()
                .map(|k| Keyframe {
                    time: k.f32_of("time").unwrap_or(0.0),
                    value: k.f32_of("value").unwrap_or(0.0),
                    in_slope: k.f32_of("inSlope").unwrap_or(0.0),
                    out_slope: k.f32_of("outSlope").unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();
    Curve { keys }
}

fn min_max_curve(v: Option<&Value>) -> MinMaxCurve {
    let Some(v) = v else { return MinMaxCurve::default() };
    MinMaxCurve {
        state: v.i64_of("minMaxState").unwrap_or(0),
        scalar: v.f32_of("scalar").unwrap_or(0.0),
        min_curve: keyframes(v.get("minCurve")),
        max_curve: keyframes(v.get("maxCurve")),
    }
}

/// `rgba` packs a `Color32` little-endian: red in the low byte.
fn rgba(v: Option<&Value>) -> [u8; 4] {
    let n = v.and_then(|c| c.i64_of("rgba")).unwrap_or(0xFFFF_FFFF) as u32;
    n.to_le_bytes()
}

fn gradient(v: Option<&Value>) -> Gradient {
    let Some(v) = v else { return Gradient::default() };
    let colors = v.i64_of("m_NumColorKeys").unwrap_or(2).clamp(0, 8) as usize;
    let alphas = v.i64_of("m_NumAlphaKeys").unwrap_or(2).clamp(0, 8) as usize;
    let key = |i: usize| rgba(v.get(&format!("key{i}")));
    Gradient {
        colors: (0..colors)
            .map(|i| {
                let c = key(i);
                (v.f32_of(&format!("ctime{i}")).unwrap_or(0.0) / 65535.0, [c[0], c[1], c[2]])
            })
            .collect(),
        alphas: (0..alphas).map(|i| (v.f32_of(&format!("atime{i}")).unwrap_or(0.0) / 65535.0, key(i)[3])).collect(),
    }
}

fn min_max_gradient(v: Option<&Value>) -> MinMaxGradient {
    let Some(v) = v else { return MinMaxGradient { max_color: [255; 4], min_color: [255; 4], ..Default::default() } };
    MinMaxGradient {
        state: v.i64_of("minMaxState").unwrap_or(0),
        min_color: rgba(v.get("minColor")),
        max_color: rgba(v.get("maxColor")),
        min_gradient: gradient(v.get("minGradient")),
        max_gradient: gradient(v.get("maxGradient")),
    }
}

fn enabled(module: Option<&Value>) -> bool {
    module.and_then(|m| m.bool_of("enabled")).unwrap_or(false)
}

/// Read a serialized `ParticleSystem` component body.
pub fn parse_particle_system(body: &Value) -> ParticleSystemSpec {
    let initial = body.get("InitialModule");
    let shape = body.get("ShapeModule");
    let emission = body.get("EmissionModule");
    let burst_count = emission.and_then(|e| e.i64_of("m_BurstCount")).unwrap_or(0).clamp(0, 4) as usize;
    let bursts = (0..burst_count)
        .map(|i| {
            let e = emission.unwrap();
            (e.f32_of(&format!("time{i}")).unwrap_or(0.0), e.i64_of(&format!("cnt{i}")).unwrap_or(0).max(0) as u32)
        })
        .collect();
    let module_curve = |name: &str, key: &str| {
        let m = body.get(name);
        enabled(m).then(|| min_max_curve(m.and_then(|m| m.get(key))))
    };
    ParticleSystemSpec {
        duration: body.f32_of("lengthInSec").unwrap_or(5.0),
        start_delay: body.f32_of("startDelay").unwrap_or(0.0),
        simulation_speed: body.f32_of("speed").unwrap_or(1.0),
        looping: body.bool_of("looping").unwrap_or(true),
        prewarm: body.bool_of("prewarm").unwrap_or(false),
        play_on_awake: body.bool_of("playOnAwake").unwrap_or(true),
        local_space: body.bool_of("moveWithTransform").unwrap_or(true),
        start_lifetime: min_max_curve(initial.and_then(|m| m.get("startLifetime"))),
        start_speed: min_max_curve(initial.and_then(|m| m.get("startSpeed"))),
        start_size: min_max_curve(initial.and_then(|m| m.get("startSize"))),
        start_rotation: min_max_curve(initial.and_then(|m| m.get("startRotation"))),
        start_color: min_max_gradient(initial.and_then(|m| m.get("startColor"))),
        gravity_modifier: initial.and_then(|m| m.f32_of("gravityModifier")).unwrap_or(0.0),
        max_particles: initial.and_then(|m| m.i64_of("maxNumParticles")).unwrap_or(1000).max(0) as usize,
        shape: ShapeModule {
            enabled: enabled(shape),
            kind: ShapeKind::from_type(shape.and_then(|s| s.i64_of("type")).unwrap_or(4)),
            radius: shape.and_then(|s| s.f32_of("radius")).unwrap_or(1.0),
            angle: shape.and_then(|s| s.f32_of("angle")).unwrap_or(25.0),
            arc: shape.and_then(|s| s.f32_of("arc")).unwrap_or(360.0),
            box_size: ["boxX", "boxY", "boxZ"].map(|k| shape.and_then(|s| s.f32_of(k)).unwrap_or(1.0)),
            random_direction: shape.and_then(|s| s.bool_of("randomDirection")).unwrap_or(false),
        },
        emission: EmissionModule { enabled: enabled(emission), rate: min_max_curve(emission.and_then(|e| e.get("rate"))), bursts },
        size_over_lifetime: module_curve("SizeModule", "curve"),
        rotation_over_lifetime: module_curve("RotationModule", "curve"),
        color_over_lifetime: {
            let m = body.get("ColorModule");
            enabled(m).then(|| min_max_gradient(m.and_then(|m| m.get("gradient"))))
        },
    }
}

/// Parse an effect prefab whose root carries a `ParticleSystem`.
pub fn parse_particle_effect(scene: &Scene, scripts: &ScriptIndex) -> Option<ParticleEffectSpec> {
    let objects = scene.game_objects();
    let root = *objects.iter().find(|g| g.parent().is_none())?;
    particle_effect_of(&root, scripts)
}

/// The `ParticleSystem` (with its `ParticleSystemLines` drawing) on one object, in a prefab or a
/// level scene.
pub fn particle_effect_of(root: &crate::scene::GameObject<'_>, scripts: &ScriptIndex) -> Option<ParticleEffectSpec> {
    let system = root.components().into_iter().find(|c| c.class_id == PARTICLE_SYSTEM)?;
    let mut spec = ParticleEffectSpec {
        name: root.name().to_owned(),
        guid: String::new(),
        prefab_z: root.transform().and_then(|t| t.body.get("m_LocalPosition")).and_then(Value::as_vec3).map_or(0.0, |p| p.2),
        system: parse_particle_system(&system.body),
        lines: None,
        rotation_follows_velocity: false,
        despawn_on_death: false,
    };
    for component in root.components() {
        if component.class_id != class::MONO_BEHAVIOUR {
            continue;
        }
        let Some(guid) = component.body.get("m_Script").and_then(Value::guid) else { continue };
        let b = &component.body;
        match scripts.class_name(guid) {
            Some("ParticleSystemLines") => {
                spec.lines = Some(LinesDrawing {
                    line_thickness: b.f32_of("lineThickness").unwrap_or(0.3),
                    fade_time: b.f32_of("fadeTime").unwrap_or(0.0),
                    shadow_plane_z: b.f32_of("shadowPlaneZ").unwrap_or(0.0),
                });
            }
            Some("ParticleRotationFollowsVelocity") => spec.rotation_follows_velocity = true,
            Some("DespawnOnParticleSystemDeath") => spec.despawn_on_death = true,
            _ => {}
        }
    }
    Some(spec)
}

/// Load an effect prefab by GUID from `Assets/GameObject`; `Ok(None)` when it has no particle
/// system on its root.
pub fn load_particle_effect(assets_dir: &Path, guid: &str) -> std::io::Result<Option<ParticleEffectSpec>> {
    let prefabs = GuidIndex::scan(&assets_dir.join("GameObject"), "prefab");
    let path = prefabs.path(guid).ok_or_else(|| std::io::Error::other(format!("no prefab with guid {guid}")))?;
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    let scene = Scene::load(path)?;
    Ok(parse_particle_effect(&scene, &scripts).map(|mut spec| {
        spec.guid = guid.to_owned();
        spec
    }))
}
