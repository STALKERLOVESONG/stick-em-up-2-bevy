//! The Shuriken particle systems the game uses (`Rocket Trail`, `Rocket Explosion`,
//! `Firework Explosion 0..3`) and how `ParticleSystemLines` draws them.
//!
//! * Simulation (`ParticleSystem.Update`, after every `LateUpdate` of the frame): particles age
//!   by the frame time, die at zero remaining lifetime, fall with `gravityModifier`, move with
//!   their velocity; then the system emits: bursts at their times within each loop, plus
//!   `rate × dt` over time (fractions carried over). A new particle gets its start lifetime,
//!   speed, size, rotation and colour, a position and direction from the shape, and is advanced
//!   by the part of the frame that followed its emission. A world-space system emits along the
//!   path its transform took during the frame.
//! * Drawing (`ParticleSystemLines.LateUpdate`): each particle becomes a line of `size` along
//!   `-rotation` (or a dot of `lineThickness` when its size is 0), `lineThickness` wide, with its
//!   colour and `alpha = clamp01(remaining / fadeTime)` (`(byte)(x * 256)`, 255 at 1), positions
//!   relative to the system's transform and the depth of the transform; a shadow copy per
//!   particle (offset and scale from the light for the transform, `shadowPlaneZ`), shadows first.
//!   Since the simulation runs after `LateUpdate`, what is drawn in a frame is the state the
//!   previous frame's simulation left.
//!
//! TODO(UNCERTAIN): Unity's particle random stream (`randomSeed` 0: reseeded on every play) and
//! the number/order of draws per particle are not recoverable; [`GameRng`] is used. Points in a
//! `Circle` shape are taken uniformly over the disc's area. Emission along a moving world-space
//! emitter is interpolated linearly. `ParticleRotationFollowsVelocity` and `ParticleSystemLines`
//! share an execution order; the rotation is taken from the velocity when the lines are read.

use unity_compat::{Vec2, Vec3};
use unity_data::particles::{LinesDrawing, ParticleEffectSpec, ParticleSystemSpec, ShapeKind};

use crate::draw::{BACK, Geometry, color32};
use crate::rng::GameRng;
use crate::visual::{LINE_TRIANGLES, StickLine, line_vertices, shadow_offset_and_scale};

/// `LinesRenderer` / `DotsRenderer`: shadows sit 0.003 behind.
const SHADOW_Z: f32 = 0.003;

/// One live particle (`ParticleSystem.Particle`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Particle {
    /// Local to the system for local-space systems, world otherwise.
    pub position: Vec3,
    pub velocity: Vec3,
    /// `Particle.lifetime`: seconds left.
    pub remaining: f32,
    pub start_lifetime: f32,
    pub size: f32,
    /// Degrees (`Particle.rotation`).
    pub rotation: f32,
    pub color: [u8; 4],
}

/// A running particle system.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleSystem {
    pub particles: Vec<Particle>,
    /// Seconds since `Play` (simulation time, before the start delay).
    time: f32,
    /// Fractional particles owed by rate emission.
    owed: f32,
    /// Completed loops (bursts fire once per loop).
    loops: u32,
    emitting: bool,
    /// Where the transform was at the previous simulation (world-space emission path).
    last_emitter: Option<Vec3>,
}

/// A dot drawn by `DotsRenderer` (a quad over the whole `Line` sprite).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dot {
    pub position: Vec3,
    pub width: f32,
    pub color: [u8; 4],
}

/// What `ParticleSystemLines` hands its renderers in one `LateUpdate`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParticleDrawing {
    pub shadow_lines: Vec<StickLine>,
    pub lines: Vec<StickLine>,
    pub shadow_dots: Vec<Dot>,
    pub dots: Vec<Dot>,
}

impl ParticleSystem {
    /// `ParticleSystem.Play` (all effect prefabs `playOnAwake`; `LifetimeParticleSystem` also calls it).
    pub fn play() -> ParticleSystem {
        ParticleSystem { particles: Vec::new(), time: 0.0, owed: 0.0, loops: 0, emitting: true, last_emitter: None }
    }

    /// `ParticleSystem.Stop`: no more emission; live particles finish their lives.
    pub fn stop(&mut self) {
        self.emitting = false;
    }

    pub fn is_emitting(&self) -> bool {
        self.emitting
    }

    /// `IsAlive(false)`: still emitting, or particles left.
    pub fn is_alive(&self) -> bool {
        self.emitting || !self.particles.is_empty()
    }

    /// One simulation step of `delta` seconds with the transform at `emitter` (world).
    pub fn update(&mut self, spec: &ParticleSystemSpec, delta: f32, emitter: Vec3, gravity: Vec2, rng: &mut GameRng) {
        self.update_oriented(spec, delta, emitter, unity_compat::Quat::IDENTITY, gravity, rng);
    }

    /// [`Self::update`] for a transform turned by `rotation`: new particles leave along the
    /// shape's directions turned with it (world-space simulation).
    pub fn update_oriented(&mut self, spec: &ParticleSystemSpec, delta: f32, emitter: Vec3, rotation: unity_compat::Quat, gravity: Vec2, rng: &mut GameRng) {
        let dt = delta * spec.simulation_speed;
        let fall = Vec3::new(gravity.x, gravity.y, 0.0) * (spec.gravity_modifier * dt);
        self.particles.retain_mut(|p| {
            p.remaining -= dt;
            if p.remaining <= 0.0 {
                return false;
            }
            p.velocity = p.velocity + fall;
            p.position = p.position + p.velocity * dt;
            true
        });
        let from = self.last_emitter.unwrap_or(emitter);
        self.last_emitter = Some(emitter);
        if !self.emitting || dt <= 0.0 {
            return;
        }
        let t0 = self.time;
        let t1 = t0 + dt;
        self.time = t1;
        let duration = spec.duration.max(1e-6);
        // Emission times inside this step, as offsets from `t0`.
        let mut emits: Vec<f32> = Vec::new();
        if spec.emission.enabled {
            // Bursts: once per loop, when the loop time passes the burst time.
            let mut loop_start = self.loops as f32 * duration + spec.start_delay;
            loop {
                for &(at, count) in &spec.emission.bursts {
                    let when = loop_start + at;
                    let first_frame_zero = t0 == 0.0 && when == 0.0;
                    if (when > t0 || first_frame_zero) && when <= t1 {
                        emits.extend(std::iter::repeat_n(when - t0, count as usize));
                    }
                }
                if !spec.looping || loop_start + duration > t1 {
                    break;
                }
                loop_start += duration;
            }
            // Rate over time, spread over the part of the step after the start delay.
            let active_from = t0.max(spec.start_delay);
            if t1 > active_from {
                let norm = ((active_from - spec.start_delay) % duration) / duration;
                let rate = spec.emission.rate.evaluate(norm, rng_value_if(spec.emission.rate.is_random(), rng));
                let span = t1 - active_from;
                let before = self.owed;
                self.owed += rate * span;
                let whole = self.owed.floor();
                let n = whole as usize;
                for i in 0..n {
                    // Particle k (1-based) of the running count is due when `owed` reaches k.
                    let k = (before.floor() + (i + 1) as f32 - before) / (self.owed - before).max(1e-12);
                    emits.push((active_from - t0) + span * k.clamp(0.0, 1.0));
                }
                self.owed -= whole;
            }
        }
        for offset in emits {
            if self.particles.len() >= spec.max_particles {
                break;
            }
            let age = dt - offset;
            let norm = (((t0 + offset - spec.start_delay).max(0.0)) % duration) / duration;
            let fraction = if dt > 0.0 { offset / dt } else { 1.0 };
            let origin = if spec.local_space { Vec3::ZERO } else { Vec3::lerp(from, emitter, fraction) };
            let mut p = spawn(spec, norm, rng);
            p.position = origin + rotation * p.position;
            p.velocity = rotation * p.velocity;
            if age > 0.0 {
                p.velocity = p.velocity + Vec3::new(gravity.x, gravity.y, 0.0) * (spec.gravity_modifier * age);
                p.position = p.position + p.velocity * age;
                p.remaining -= age;
            }
            if p.remaining > 0.0 {
                self.particles.push(p);
            }
        }
        if t1 - spec.start_delay >= (self.loops + 1) as f32 * duration {
            if spec.looping {
                self.loops += 1;
            } else {
                self.emitting = false;
            }
        }
    }

    /// `ParticleSystemLines.LateUpdate` for a system whose transform is at `emitter`.
    pub fn drawing(&self, spec: &ParticleSystemSpec, lines: &LinesDrawing, rotation_follows_velocity: bool, emitter: Vec3, light: Option<Vec3>) -> ParticleDrawing {
        let mut out = ParticleDrawing::default();
        let (delta, scale) = match light {
            Some(light) => shadow_offset_and_scale(light, emitter, lines.shadow_plane_z),
            None => ([0.0, 0.0], 1.0),
        };
        for p in &self.particles {
            let mut color = p.color;
            let f = (p.remaining / lines.fade_time).clamp(0.0, 1.0);
            color[3] = if f != 1.0 { (f * 256.0) as u8 } else { 255 };
            // `Vector2 vector2 = particle.position` (minus the transform for world space), drawn
            // by a renderer placed at the transform's position.
            let rel = if spec.local_space { Vec2::new(p.position.x, p.position.y) } else { Vec2::new(p.position.x - emitter.x, p.position.y - emitter.y) };
            let at = Vec3::new(emitter.x + rel.x, emitter.y + rel.y, emitter.z);
            let rotation = if rotation_follows_velocity { -(p.velocity.y.atan2(p.velocity.x)) * 57.295_78 } else { p.rotation };
            let shadow_color = [0, 0, 0, color[3]];
            if p.size != 0.0 {
                let f = -rotation * (std::f32::consts::PI / 180.0);
                let half = p.size * 0.5;
                let d = Vec3::new(half * f.cos(), half * f.sin(), 0.0);
                let (a, b) = (at - d, at + d);
                out.lines.push(StickLine { a, b, width: lines.line_thickness, color, shadow_plane_z: lines.shadow_plane_z });
                let shift = Vec3::new(delta[0], delta[1], SHADOW_Z);
                out.shadow_lines.push(StickLine { a: a + shift, b: b + shift, width: lines.line_thickness * scale, color: shadow_color, shadow_plane_z: lines.shadow_plane_z });
            } else {
                out.dots.push(Dot { position: at, width: lines.line_thickness, color });
                out.shadow_dots.push(Dot { position: Vec3::new(at.x + delta[0], at.y + delta[1], at.z + SHADOW_Z), width: lines.line_thickness * scale, color: shadow_color });
            }
        }
        out
    }
}

impl ParticleDrawing {
    /// The two meshes `ParticleSystemLines` fills: the `LinesRenderer` (every shadow line, then
    /// every line) and the `DotsRenderer` (shadow dots, then dots; a quad over the whole `Line`
    /// sprite: `uvPrototype` (0,1) (0,0) (1,1) (1,0), triangles 0 3 1 3 0 2). `line_uvs` are the
    /// line capsule UVs of the `Line` sprite ([`crate::visual::VisualArt::line_uvs`]).
    pub fn geometry(&self, line_uvs: [[f32; 2]; 8]) -> [Geometry; 2] {
        let mut lines = Geometry::default();
        for line in self.shadow_lines.iter().chain(&self.lines) {
            let Some(vertices) = line_vertices(line) else { continue };
            let base = lines.positions.len() as u32;
            lines.positions.extend(vertices.iter().map(|v| [v.x, v.y, v.z]));
            lines.normals.extend([BACK; 8]);
            lines.uvs.extend(line_uvs);
            lines.colors.extend([color32(line.color.map(|v| f32::from(v) / 255.0)); 8]);
            lines.indices.extend(LINE_TRIANGLES.iter().map(|i| base + i));
        }
        let dot_uvs = [line_uvs[0], line_uvs[1], line_uvs[6], line_uvs[7]];
        let mut dots = Geometry::default();
        for dot in self.shadow_dots.iter().chain(&self.dots) {
            let (p, h) = (dot.position, dot.width * 0.5);
            let base = dots.positions.len() as u32;
            dots.positions.extend([[p.x - h, p.y - h, p.z], [p.x + h, p.y - h, p.z], [p.x - h, p.y + h, p.z], [p.x + h, p.y + h, p.z]]);
            dots.normals.extend([BACK; 4]);
            dots.uvs.extend(dot_uvs);
            dots.colors.extend([color32(dot.color.map(|v| f32::from(v) / 255.0)); 4]);
            dots.indices.extend([0, 3, 1, 3, 0, 2].map(|i| base + i));
        }
        [lines, dots]
    }
}

fn rng_value_if(random: bool, rng: &mut GameRng) -> f32 {
    if random { rng.value() } else { 0.0 }
}

/// A new particle at the shape's local position, moving along the shape's direction.
fn spawn(spec: &ParticleSystemSpec, norm: f32, rng: &mut GameRng) -> Particle {
    let lifetime = spec.start_lifetime.evaluate(norm, rng_value_if(spec.start_lifetime.is_random(), rng));
    let speed = spec.start_speed.evaluate(norm, rng_value_if(spec.start_speed.is_random(), rng));
    let size = spec.start_size.evaluate(norm, rng_value_if(spec.start_size.is_random(), rng));
    let rotation = spec.start_rotation.evaluate(norm, rng_value_if(spec.start_rotation.is_random(), rng)).to_degrees();
    let color = spec.start_color.evaluate(norm, rng_value_if(spec.start_color.is_random(), rng));
    let shape = &spec.shape;
    let (position, direction) = if !shape.enabled {
        (Vec3::ZERO, Vec3::FORWARD)
    } else {
        match shape.kind {
            ShapeKind::Circle | ShapeKind::CircleEdge => {
                let angle = rng.value() * shape.arc.to_radians();
                let r = if shape.kind == ShapeKind::Circle { shape.radius * rng.value().sqrt() } else { shape.radius };
                let dir = Vec3::new(angle.cos(), angle.sin(), 0.0);
                (dir * r, dir)
            }
            ShapeKind::Sphere | ShapeKind::SphereShell | ShapeKind::Hemisphere | ShapeKind::HemisphereShell => {
                let z = rng.range_f32(-1.0, 1.0);
                let a = rng.value() * std::f32::consts::TAU;
                let s = (1.0 - z * z).max(0.0).sqrt();
                let mut dir = Vec3::new(s * a.cos(), s * a.sin(), z);
                if matches!(shape.kind, ShapeKind::Hemisphere | ShapeKind::HemisphereShell) && dir.z < 0.0 {
                    dir.z = -dir.z;
                }
                let r = if matches!(shape.kind, ShapeKind::Sphere | ShapeKind::Hemisphere) { shape.radius * rng.value().cbrt() } else { shape.radius };
                (dir * r, dir)
            }
            ShapeKind::Cone => {
                // A point on the base disc; the direction leans out with the distance from the
                // centre, by `angle` at the rim (along local +z at the centre).
                // TODO(UNCERTAIN): Unity's exact cone distribution (it also emits from the base
                // only, as here, with "Base" emission).
                let a = rng.value() * shape.arc.max(0.0).min(360.0).to_radians().max(1e-6);
                let d = rng.value().sqrt();
                let (sin, cos) = shape.angle.clamp(0.0, 90.0).to_radians().sin_cos();
                let radial = Vec3::new(a.cos(), a.sin(), 0.0);
                let dir = Vec3::new(radial.x * d * sin, radial.y * d * sin, cos + (1.0 - d) * (1.0 - cos)).normalized();
                (radial * (shape.radius * d), dir)
            }
            _ => (Vec3::ZERO, Vec3::FORWARD),
        }
    };
    let direction = if shape.random_direction {
        let (x, y) = rng.inside_unit_circle();
        Vec3::new(x, y, 0.0).normalized()
    } else {
        direction
    };
    Particle { position, velocity: direction * speed, remaining: lifetime, start_lifetime: lifetime, size, rotation, color }
}

/// A spawned effect prefab: the system plus where its transform is.
#[derive(Clone, Debug, PartialEq)]
pub struct ParticleEffect {
    pub system: ParticleSystem,
    pub position: Vec3,
}

impl ParticleEffect {
    pub fn spawn(position: Vec3) -> ParticleEffect {
        ParticleEffect { system: ParticleSystem::play(), position }
    }

    /// What the effect draws this frame (`None` without `ParticleSystemLines`).
    pub fn drawing(&self, spec: &ParticleEffectSpec, light: Option<Vec3>) -> Option<ParticleDrawing> {
        let lines = spec.lines.as_ref()?;
        Some(self.system.drawing(&spec.system, lines, spec.rotation_follows_velocity, self.position, light))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use unity_data::particles::{EmissionModule, MinMaxCurve, MinMaxGradient, ShapeModule};

    fn burst_spec() -> ParticleSystemSpec {
        // `Rocket Explosion`.
        ParticleSystemSpec {
            duration: 0.1,
            start_delay: 0.0,
            simulation_speed: 1.0,
            looping: false,
            prewarm: false,
            play_on_awake: true,
            local_space: true,
            start_lifetime: MinMaxCurve::constant(0.4),
            start_speed: MinMaxCurve::constant(15.0),
            start_size: MinMaxCurve::constant(0.2),
            start_rotation: MinMaxCurve::constant(0.0),
            start_color: MinMaxGradient { state: 0, max_color: [128, 128, 128, 255], ..Default::default() },
            gravity_modifier: 0.0,
            max_particles: 10,
            shape: ShapeModule { enabled: true, kind: ShapeKind::CircleEdge, radius: 0.1, angle: 90.0, arc: 360.0, box_size: [1.0; 3], random_direction: false },
            emission: EmissionModule { enabled: true, rate: MinMaxCurve::constant(0.0), bursts: vec![(0.0, 10)] },
            size_over_lifetime: None,
            rotation_over_lifetime: None,
            color_over_lifetime: None,
        }
    }

    const LINES: LinesDrawing = LinesDrawing { line_thickness: 0.3, fade_time: 0.4, shadow_plane_z: 0.0 };

    #[test]
    fn a_burst_flies_out_radially_fades_and_dies() {
        let spec = burst_spec();
        let mut rng = GameRng::from_seed(7);
        let mut ps = ParticleSystem::play();
        let at = Vec3::new(5.0, 2.0, -0.1);
        // Nothing to draw before the first simulation.
        assert!(ps.drawing(&spec, &LINES, true, at, None).lines.is_empty());
        ps.update(&spec, 0.02, at, Vec2::new(0.0, -16.0), &mut rng);
        assert_eq!(ps.particles.len(), 10);
        for p in &ps.particles {
            // Emitted on the rim at radius 0.1 and advanced by the whole first frame.
            let r = Vec2::new(p.position.x, p.position.y).magnitude();
            assert!((r - (0.1 + 15.0 * 0.02)).abs() < 1e-4, "radius {r}");
            assert!((p.velocity.magnitude() - 15.0).abs() < 1e-4);
            assert!((p.remaining - 0.38).abs() < 1e-6);
        }
        let d = ps.drawing(&spec, &LINES, true, at, None);
        assert_eq!((d.lines.len(), d.shadow_lines.len()), (10, 10));
        let line = d.lines[0];
        // A 0.2-long line along the velocity, around the particle, at the emitter's depth.
        assert!(((line.b - line.a).magnitude() - 0.2).abs() < 1e-4);
        let p = ps.particles[0];
        let along = (line.b - line.a).normalized();
        assert!((along.dot(p.velocity.normalized()).abs() - 1.0).abs() < 1e-4);
        assert_eq!(line.a.z, -0.1);
        // 0.38 / 0.4 = 0.95 → (byte)(0.95 * 256) = 243.
        assert_eq!(line.color, [128, 128, 128, 243]);
        assert!(ps.is_alive());
        for _ in 0..19 {
            ps.update(&spec, 0.02, at, Vec2::ZERO, &mut rng);
        }
        assert!(ps.particles.is_empty(), "0.4 s lifetime");
        assert!(!ps.is_alive(), "non-looping: done once duration passed and particles died");
    }

    #[test]
    fn a_looping_trail_emits_its_rate_along_the_emitters_path_in_world_space() {
        let mut spec = burst_spec();
        spec.looping = true;
        spec.duration = 5.0;
        spec.local_space = false;
        spec.max_particles = 28;
        spec.start_lifetime = MinMaxCurve::constant(0.93);
        spec.start_speed = MinMaxCurve::constant(0.0);
        spec.start_size = MinMaxCurve::constant(0.0);
        spec.shape.radius = 0.0;
        spec.emission = EmissionModule { enabled: true, rate: MinMaxCurve::constant(30.0), bursts: Vec::new() };
        let mut rng = GameRng::from_seed(8);
        let mut ps = ParticleSystem::play();
        let mut x = 0.0;
        ps.update(&spec, 0.0001, Vec3::new(x, 0.0, 0.0), Vec2::ZERO, &mut rng);
        for _ in 0..30 {
            x += 0.25;
            ps.update(&spec, 1.0 / 30.0, Vec3::new(x, 0.0, 0.0), Vec2::ZERO, &mut rng);
        }
        // ~30 per second, capped by maxParticles and the 0.93 s lifetime.
        assert!((26..=28).contains(&ps.particles.len()), "{}", ps.particles.len());
        // Each particle stays where it was emitted along the path.
        let mut xs: Vec<f32> = ps.particles.iter().map(|p| p.position.x).collect();
        xs.sort_by(f32::total_cmp);
        assert!(xs.windows(2).all(|w| w[1] - w[0] > 0.1), "spread along the path: {xs:?}");
        let d = ps.drawing(&spec, &LinesDrawing { line_thickness: 0.3, fade_time: 2.0 / 3.0, shadow_plane_z: 0.0 }, false, Vec3::new(x, 0.0, -0.1), None);
        assert!(d.lines.is_empty() && d.dots.len() == ps.particles.len(), "size 0: dots");
        assert!(d.dots.iter().all(|dot| dot.position.z == -0.1 && dot.width == 0.3));
        ps.stop();
        assert!(ps.is_alive());
        for _ in 0..60 {
            ps.update(&spec, 1.0 / 30.0, Vec3::new(x, 0.0, 0.0), Vec2::ZERO, &mut rng);
        }
        assert!(!ps.is_alive(), "stopped and every particle died");
    }

    #[test]
    fn alpha_is_full_until_the_fade_time_remains() {
        let spec = burst_spec();
        let mut ps = ParticleSystem::play();
        ps.particles.push(Particle { position: Vec3::ZERO, velocity: Vec3::RIGHT, remaining: 1.0, start_lifetime: 1.0, size: 0.2, rotation: 0.0, color: [255; 4] });
        let lines = LinesDrawing { line_thickness: 0.3, fade_time: 0.5, shadow_plane_z: 0.0 };
        assert_eq!(ps.drawing(&spec, &lines, false, Vec3::ZERO, None).lines[0].color[3], 255);
        ps.particles[0].remaining = 0.25;
        assert_eq!(ps.drawing(&spec, &lines, false, Vec3::ZERO, None).lines[0].color[3], 128);
    }
}
