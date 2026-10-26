//! `Explosion` (built by `ExplodeSpritesAndLines`): when an object explodes, the lines its
//! `LinesRenderer`s drew last frame and its sprites fly apart, spin, wait `fadeWaitTime`, fade to
//! transparent black over `fadeFadeTime` and are gone.
//!
//! * `Go`: the explosion object takes the exploding object's position (its own
//!   `LinesRenderer` sits there, so the lines keep their root-relative coordinates);
//!   `PrepareOrReadyLines` turns every last line into a part (midpoint, half length, angle, width,
//!   colour, shadow plane) with `RandomizeVelocities(5, π)` (angular speed in radians per second);
//!   then every sprite part that is active (`FlatSprite` copies, then `SpriteShadower` sprites)
//!   starts at its source's world position and rotation with `RandomizeVelocities(5, 180)`
//!   (degrees per second); then `Update()` runs once.
//! * `Update`: `runTime += deltaTime`; `num = max(runTime - fadeWaitTime, 0) / 1`; past 1 it
//!   finishes (nothing more is drawn). Lines: `midPoint += v dt`, `angle += w dt`, redrawn with
//!   `AddLineWithShadow(mid ∓ (cos, sin) halfLength, width, Color32.Lerp(color, clear, num))`.
//!   Sprites: `localPosition += v dt`, `Rotate(Vector3.back, w dt)` (local axes),
//!   `color = Color.Lerp(startColor, clear, num)`; their `SpriteShadower` copies follow.
//!
//! * Projectiles (`Explosion::go_projectile`): their parts are the `SpriteShadower` sprites. A
//!   collision explosion puts every part at the collision point (the projectile's position in
//!   `OnCollisionEnter2D`) with the rotation the projectile had in its last `Update` before the
//!   contact — not at the sprite's own offset. With `sticks` (Staple, Spear) and a `collidedWith`
//!   the part is pinned to that transform by its local pose (`transform.parent = collidedWith`,
//!   read `localPosition`/`localRotation`, unparent) and every `Update` re-derives its world pose
//!   from the anchor's current transform (see [`Anchor`]).
//!
//! TODO(UNCERTAIN): `UnityEngine.Random` is replaced by [`GameRng`] (Unity's stream is
//! clock-seeded); `Prepare` also draws velocities for the lines one frame after `Start`, which is
//! not reproduced (only the stream position would differ).

use unity_compat::{Quat, Vec3};

use crate::draw::{BACK, Geometry, SHADOW_DEPTH, color32, sprite_geometry};
use crate::rng::GameRng;
use crate::visual::{CharacterVisual, LINE_TRIANGLES, StickLine, VisualArt, line_vertices, shadow_offset_and_scale};

/// `Explosion.maxPartSpeed`.
pub const MAX_PART_SPEED: f32 = 5.0;
/// `Explosion.maxPartAngularSpeed` (degrees per second, for sprites).
pub const MAX_PART_ANGULAR_SPEED: f32 = 180.0;
/// `Explosion.fadeFadeTime`.
pub const FADE_FADE_TIME: f32 = 1.0;
/// `LinesRenderer.AddLineWithShadow`: shadow lines sit 0.003 behind.
const LINE_SHADOW_Z: f32 = 0.003;

/// `Explosion.PartMotion`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PartMotion {
    pub linear: Vec3,
    pub angular: f32,
}

impl PartMotion {
    /// `RandomizeVelocities`: `Random.insideUnitCircle * maxLinearSpeed` (z 0),
    /// `Random.Range(-1, 1) * maxAngularSpeed`.
    pub fn randomized(rng: &mut GameRng, max_linear: f32, max_angular: f32) -> PartMotion {
        let (x, y) = rng.inside_unit_circle();
        let linear = Vec3::new(x * max_linear, y * max_linear, 0.0);
        PartMotion { linear, angular: rng.range_f32(-1.0, 1.0) * max_angular }
    }
}

/// `Explosion.LinePart`, in world space (the explosion's renderer never moves).
#[derive(Clone, Debug, PartialEq)]
pub struct LinePart {
    pub mid_point: Vec3,
    pub half_length: f32,
    pub angle_rad: f32,
    pub width: f32,
    pub color: [u8; 4],
    pub shadow_plane_z: f32,
    pub motion: PartMotion,
}

/// `Explosion.SpritePart`: a copy of a sprite flying on its own (or pinned, see [`Stuck`]).
#[derive(Clone, Debug, PartialEq)]
pub struct SpritePart {
    pub sprite_name: String,
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: [f32; 3],
    pub start_color: [f32; 4],
    pub color: [f32; 4],
    pub shadow_plane_z: f32,
    pub motion: PartMotion,
    /// `PartMotion.stuck/stuckTo/stuckLocalPosition/stuckLocalRotation` (projectile parts only).
    pub stuck: Option<Stuck>,
}

/// Identifies, for the caller, an object explosion parts can be pinned to (`collidedWith`).
pub type AnchorId = u64;

/// World transform of a `collidedWith`, as `Transform.TransformPoint` and `rotation` see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Anchor {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Anchor {
    /// A static object at the origin: world and local coordinates coincide.
    pub const IDENTITY: Anchor = Anchor { position: Vec3::ZERO, rotation: Quat::IDENTITY, scale: Vec3::new(1.0, 1.0, 1.0) };

    pub fn transform_point(&self, local: Vec3) -> Vec3 {
        self.position + self.rotation * Vec3::new(local.x * self.scale.x, local.y * self.scale.y, local.z * self.scale.z)
    }

    pub fn inverse_transform_point(&self, world: Vec3) -> Vec3 {
        let v = self.rotation.conjugate() * (world - self.position);
        Vec3::new(v.x / self.scale.x, v.y / self.scale.y, v.z / self.scale.z)
    }
}

/// How a projectile part is pinned.
///
/// `Explosion.Update` reads the motion into a local copy and never writes it back, so `stuck`
/// is re-evaluated every frame from the value `Go` stored: while the anchor is inactive or
/// destroyed the part flies with its random velocity from wherever it is, and if the anchor
/// becomes active again (a respawned avatar) the part snaps back onto it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stuck {
    /// `motion.stuck` as `Go` set it (`ExplodeSpritesAndLines.sticks`).
    pub stuck: bool,
    /// `motion.stuckTo`; `None` when `Go` had no `collidedWith`.
    pub to: Option<AnchorId>,
    pub local_position: Vec3,
    pub local_rotation: Quat,
}

/// One sprite of an exploding projectile, as `Explosion.Go` finds it (`SpriteShadower` copies
/// made by `Prepare`).
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectilePartSource {
    pub sprite_name: String,
    /// The source sprite's world transform (used when the explosion was not a collision).
    pub position: Vec3,
    pub rotation: Quat,
    /// `tk2dSprite.scale` / `color` (the copy keeps them) and the shadower's plane.
    pub scale: [f32; 3],
    pub color: [f32; 4],
    pub shadow_plane_z: f32,
    /// `sourceSprite.gameObject.activeInHierarchy` at `Go`.
    pub active: bool,
}

/// `causedByCollision`, `collisionPoint`, `lastRotationBeforeCollision`, `collidedWith`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CollisionData {
    pub point: Vec3,
    pub last_rotation: Quat,
    /// The anchor and its transform at the moment of the collision.
    pub collided_with: Option<(AnchorId, Anchor)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Explosion {
    /// Position of the explosion object (its `LinesRenderer` computes shadows from here).
    pub origin: Vec3,
    pub lines: Vec<LinePart>,
    pub sprites: Vec<SpritePart>,
    pub run_time: f32,
    pub fade_wait_time: f32,
    /// `num` of the last `Update`.
    pub fade: f32,
}

/// `Color32.Lerp(a, transparent black, t)`: per channel `(byte)(a + (0 - a) * t)`.
fn lerp_to_clear32(c: [u8; 4], t: f32) -> [u8; 4] {
    let t = t.clamp(0.0, 1.0);
    c.map(|v| (f32::from(v) + (0.0 - f32::from(v)) * t) as u8)
}

impl Explosion {
    /// `Explosion.Go` for an object that drew `last` last frame with its root at `last_root` and
    /// whose root is now at `root`. `delta_time` is `Time.deltaTime` where `Explode` was called.
    pub fn go(last: &CharacterVisual, last_root: Vec3, root: Vec3, fade_wait_time: f32, rng: &mut GameRng, delta_time: f32) -> Explosion {
        let mut lines = Vec::with_capacity(last.lines.len());
        for line in &last.lines {
            // `GetLastLines` are root-relative; the explosion's renderer sits at the new root.
            let a = line.a - last_root + root;
            let b = line.b - last_root + root;
            let vector = b - a;
            let half = vector * 0.5;
            lines.push(LinePart {
                mid_point: a + half,
                angle_rad: vector.y.atan2(vector.x),
                half_length: half.magnitude(),
                width: line.width,
                color: line.color,
                shadow_plane_z: line.shadow_plane_z,
                motion: PartMotion::randomized(rng, MAX_PART_SPEED, std::f32::consts::PI),
            });
        }
        let mut sprites = Vec::with_capacity(last.sprites.len());
        for sprite in &last.sprites {
            // `FlatSprite.LateUpdate`: `localEulerAngles = (0, flip ? 180 : 0, roll)`.
            let rotation = Quat::euler_y_degrees(if sprite.flipped { 180.0 } else { 0.0 }) * Quat::about_z(sprite.roll_degrees.to_radians());
            sprites.push(SpritePart {
                sprite_name: sprite.sprite_name.clone(),
                position: sprite.position,
                rotation,
                scale: sprite.scale,
                start_color: sprite.color,
                color: sprite.color,
                shadow_plane_z: sprite.shadow_plane_z,
                motion: PartMotion::randomized(rng, MAX_PART_SPEED, MAX_PART_ANGULAR_SPEED),
                stuck: None,
            });
        }
        let mut explosion = Explosion { origin: root, lines, sprites, run_time: 0.0, fade_wait_time, fade: 0.0 };
        explosion.update(delta_time);
        explosion
    }

    /// `Explosion.Go` for a projectile (`ExplodeSpritesAndLines` with no `LinesRenderer`): `root`
    /// is the projectile's position, `collision` is set for `causedByCollision`, `sticks` is
    /// `ExplodeSpritesAndLines.sticks`. Inactive sources draw no random numbers and stay hidden.
    /// `anchors` gives the current transform of an active anchor (for the `Update` that `Go`
    /// runs).
    #[allow(clippy::too_many_arguments)]
    pub fn go_projectile(
        parts: &[ProjectilePartSource],
        root: Vec3,
        collision: Option<&CollisionData>,
        sticks: bool,
        fade_wait_time: f32,
        rng: &mut GameRng,
        delta_time: f32,
        anchors: &dyn Fn(AnchorId) -> Option<Anchor>,
    ) -> Explosion {
        let mut sprites = Vec::with_capacity(parts.len());
        for part in parts.iter().filter(|p| p.active) {
            let motion = PartMotion::randomized(rng, MAX_PART_SPEED, MAX_PART_ANGULAR_SPEED);
            let (position, rotation, stuck) = match collision {
                Some(c) => {
                    let pinned = c.collided_with.map(|(id, anchor)| Stuck {
                        stuck: sticks,
                        to: Some(id),
                        // `transform.parent = collidedWith` keeps the world pose.
                        local_position: anchor.inverse_transform_point(c.point),
                        local_rotation: anchor.rotation.conjugate() * c.last_rotation,
                    });
                    let stuck = pinned.unwrap_or(Stuck { stuck: sticks, to: None, local_position: Vec3::ZERO, local_rotation: Quat::IDENTITY });
                    (c.point, c.last_rotation, stuck)
                }
                None => (part.position, part.rotation, Stuck { stuck: sticks, to: None, local_position: Vec3::ZERO, local_rotation: Quat::IDENTITY }),
            };
            sprites.push(SpritePart {
                sprite_name: part.sprite_name.clone(),
                position,
                rotation,
                scale: part.scale,
                start_color: part.color,
                color: part.color,
                shadow_plane_z: part.shadow_plane_z,
                motion,
                stuck: Some(stuck),
            });
        }
        let mut explosion = Explosion { origin: root, lines: Vec::new(), sprites, run_time: 0.0, fade_wait_time, fade: 0.0 };
        explosion.update_anchored(delta_time, anchors);
        explosion
    }

    /// `Explosion.Update`; true once it has finished (and draws nothing more).
    pub fn update(&mut self, delta_time: f32) -> bool {
        self.update_anchored(delta_time, &|_| None)
    }

    /// `Explosion.Update` with the anchors of stuck parts: `anchors(id)` is the anchor's current
    /// world transform while it is active, `None` once it is inactive or destroyed.
    pub fn update_anchored(&mut self, delta_time: f32, anchors: &dyn Fn(AnchorId) -> Option<Anchor>) -> bool {
        self.run_time += delta_time;
        let num = (self.run_time - self.fade_wait_time).max(0.0) / FADE_FADE_TIME;
        if num > 1.0 {
            return true;
        }
        self.fade = num;
        for line in &mut self.lines {
            line.mid_point = line.mid_point + line.motion.linear * delta_time;
            line.angle_rad += line.motion.angular * delta_time;
        }
        for sprite in &mut self.sprites {
            // `stuckTo != null && activeInHierarchy`, and the local copy of `motion.stuck`.
            let anchor = sprite.stuck.and_then(|s| s.to).and_then(anchors);
            let mut stuck_now = sprite.stuck.is_some_and(|s| s.stuck);
            if sprite.stuck.is_some_and(|s| s.to.is_some()) && anchor.is_none() {
                stuck_now = false;
            }
            if stuck_now {
                if let (Some(a), Some(s)) = (anchor, sprite.stuck) {
                    sprite.position = a.transform_point(s.local_position);
                    sprite.rotation = a.rotation * s.local_rotation;
                }
            } else {
                sprite.position = sprite.position + sprite.motion.linear * delta_time;
                // `Transform.Rotate(Vector3.back, angle)`: about the local back axis.
                sprite.rotation = sprite.rotation * Quat::about_z((-(sprite.motion.angular * delta_time)).to_radians());
            }
            sprite.color = sprite.start_color.map(|c| c + (0.0 - c) * num.clamp(0.0, 1.0));
        }
        false
    }

    /// The renderers this frame: the explosion's `LinesRenderer` (every shadow line, then every
    /// line), then each sprite and its `SpriteShadower` copy.
    pub fn renderers(&self, art: &VisualArt, light: Option<Vec3>) -> Vec<Geometry> {
        let mut out = Vec::new();
        let mut shadows = Vec::with_capacity(self.lines.len());
        let mut lines = Vec::with_capacity(self.lines.len());
        // `AddLineWithShadow` recomputes the offset whenever the plane changes.
        let mut plane = f32::NAN;
        let (mut delta, mut scale) = ([0.0, 0.0], 1.0);
        for part in &self.lines {
            let (s, c) = part.angle_rad.sin_cos();
            let offset = Vec3::new(c * part.half_length, s * part.half_length, 0.0);
            let (a, b) = (part.mid_point - offset, part.mid_point + offset);
            let color = lerp_to_clear32(part.color, self.fade);
            if part.shadow_plane_z != plane {
                if let Some(light) = light {
                    (delta, scale) = shadow_offset_and_scale(light, self.origin, part.shadow_plane_z);
                    plane = part.shadow_plane_z;
                }
            }
            lines.push(StickLine { a, b, width: part.width, color, shadow_plane_z: part.shadow_plane_z });
            shadows.push(StickLine {
                a: Vec3::new(a.x + delta[0], a.y + delta[1], a.z + LINE_SHADOW_Z),
                b: Vec3::new(b.x + delta[0], b.y + delta[1], b.z + LINE_SHADOW_Z),
                width: part.width * scale,
                color: [0, 0, 0, color[3]],
                shadow_plane_z: part.shadow_plane_z,
            });
        }
        let mut g = Geometry::default();
        for line in shadows.iter().chain(&lines) {
            let Some(vertices) = line_vertices(line) else { continue };
            let base = g.positions.len() as u32;
            g.positions.extend(vertices.iter().map(|v| [v.x, v.y, v.z]));
            g.normals.extend([BACK; 8]);
            g.uvs.extend(art.line_uvs());
            g.colors.extend([color32(line.color.map(|v| f32::from(v) / 255.0)); 8]);
            g.indices.extend(LINE_TRIANGLES.iter().map(|i| base + i));
        }
        out.push(g);
        for sprite in &self.sprites {
            let Some(sprite_art) = art.sprite(&sprite.sprite_name) else { continue };
            let r = sprite.rotation;
            let p = sprite.position;
            let world = unity_data::math::Affine::from_trs([p.x, p.y, p.z], (r.x, r.y, r.z, r.w), [1.0; 3]);
            out.push(sprite_geometry(&sprite_art.def, sprite.scale, sprite.color, &world));
            if let Some(shadow) = &sprite_art.shadow_def {
                let (offset, k) = match light {
                    Some(light) => shadow_offset_and_scale(light, p, sprite.shadow_plane_z),
                    None => ([0.0, 0.0], 1.0),
                };
                let at = [p.x + offset[0], p.y + offset[1], p.z + SHADOW_DEPTH];
                let world = unity_data::math::Affine::from_trs(at, (r.x, r.y, r.z, r.w), [1.0; 3]);
                out.push(sprite_geometry(shadow, [sprite.scale[0] * k, sprite.scale[1] * k, 1.0], [0.0, 0.0, 0.0, sprite.color[3]], &world));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visual::FlatSpriteInstance;

    fn last_frame() -> CharacterVisual {
        let line = StickLine { a: Vec3::new(9.0, 1.0, -0.1), b: Vec3::new(11.0, 1.0, -0.1), width: 0.2, color: [200, 100, 50, 255], shadow_plane_z: 0.0 };
        let sprite = FlatSpriteInstance {
            bone: 0,
            sprite_name: "Helmet".into(),
            position: Vec3::new(10.0, 2.0, -0.2),
            flipped: false,
            roll_degrees: 0.0,
            scale: [1.0; 3],
            color: [1.0, 0.5, 0.25, 1.0],
            shadow_plane_z: 0.0,
            shadow_offset: [0.0, 0.0],
            shadow_scale: 1.0,
        };
        CharacterVisual { shadows: vec![line], lines: vec![line], sprites: vec![sprite] }
    }

    #[test]
    fn parts_keep_their_shape_fly_and_fade_on_the_original_timeline() {
        let mut rng = GameRng::from_seed(3);
        let root = Vec3::new(10.0, 0.0, -0.1);
        // The root moved by (1, 0) since the last frame was drawn: the parts move with it.
        let mut e = Explosion::go(&last_frame(), root - Vec3::new(1.0, 0.0, 0.0), root, 2.0, &mut rng, 0.02);
        assert_eq!(e.lines.len(), 1);
        assert_eq!(e.sprites.len(), 1);
        let line = &e.lines[0];
        assert!((line.half_length - 1.0).abs() < 1e-6 && (line.angle_rad - line.motion.angular * 0.02).abs() < 1e-6);
        let v = line.motion.linear;
        assert!(v.magnitude() <= MAX_PART_SPEED + 1e-4 && v.z == 0.0);
        assert!(line.motion.angular.abs() <= std::f32::consts::PI);
        // `Go` already ran one `Update` of 0.02 s.
        let expected = Vec3::new(11.0, 1.0, -0.1) + v * 0.02;
        assert!((line.mid_point - expected).magnitude() < 1e-5);
        assert!(e.sprites[0].motion.angular.abs() <= MAX_PART_ANGULAR_SPEED);
        // Drifting: no fade until fadeWaitTime, then a one-second fade, then finished.
        assert!(!e.update(1.0));
        assert_eq!(e.fade, 0.0);
        assert!(!e.update(1.48));
        assert!((e.fade - 0.5).abs() < 1e-5);
        assert!((e.sprites[0].color[0] - 0.5).abs() < 1e-5 && (e.sprites[0].color[3] - 0.5).abs() < 1e-5);
        assert_eq!(lerp_to_clear32([200, 100, 50, 255], e.fade), [100, 50, 25, 127]);
        assert!(!e.update(0.5));
        assert!(e.update(0.01), "past runTime = fadeWaitTime + 1 it finishes");
    }

    fn staple_part() -> ProjectilePartSource {
        ProjectilePartSource {
            sprite_name: "Staple".into(),
            // The sprite sits at its own offset; a collision explosion ignores it.
            position: Vec3::new(5.3, 1.0, -0.1),
            rotation: Quat::IDENTITY,
            scale: [0.35, 0.35, 1.0],
            color: [1.0; 4],
            shadow_plane_z: 0.0,
            active: true,
        }
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).magnitude() < 1e-5
    }

    #[test]
    fn a_staple_sticks_where_it_hit_a_wall_then_fades() {
        let mut rng = GameRng::from_seed(1);
        let point = Vec3::new(5.0, 1.0, -0.1);
        let launch = Quat::about_z(30f32.to_radians());
        let hit = CollisionData { point, last_rotation: launch, collided_with: Some((7, Anchor::IDENTITY)) };
        let anchors = |id: AnchorId| (id == 7).then_some(Anchor::IDENTITY);
        let mut e = Explosion::go_projectile(&[staple_part()], point, Some(&hit), true, 2.0, &mut rng, 0.02, &anchors);
        assert_eq!(e.sprites.len(), 1);
        assert!(close(e.sprites[0].position, point), "at the collision point, not the sprite's offset");
        for _ in 0..99 {
            assert!(!e.update_anchored(0.02, &anchors));
        }
        let part = &e.sprites[0];
        assert!(close(part.position, point), "pinned parts do not drift");
        assert!((part.rotation.dot(launch).abs() - 1.0).abs() < 1e-5, "it keeps the last rotation before the contact");
        assert_eq!(part.color, [1.0; 4], "no fade before fadeWaitTime");
        assert!(!e.update_anchored(0.5, &anchors));
        assert!((e.sprites[0].color[3] - 0.5).abs() < 1e-4);
        assert!(e.update_anchored(0.51, &anchors));
    }

    #[test]
    fn a_staple_in_a_man_follows_his_root_and_mirrors_when_he_turns() {
        let mut rng = GameRng::from_seed(2);
        // `CharacterControl2D`: the root is turned ±90° about Y for the facing.
        let facing_right = Anchor { position: Vec3::new(10.0, 0.0, -0.05), rotation: Quat::euler_y_degrees(90.0), scale: Vec3::new(1.0, 1.0, 1.0) };
        let point = Vec3::new(9.6, 1.2, -0.1);
        let hit = CollisionData { point, last_rotation: Quat::IDENTITY, collided_with: Some((1, facing_right)) };
        let mut anchor = facing_right;
        let mut e = Explosion::go_projectile(&[staple_part()], point, Some(&hit), true, 2.0, &mut rng, 0.02, &|_| Some(facing_right));
        // He walks one unit right: the staple comes along.
        anchor.position = Vec3::new(11.0, 0.0, -0.05);
        e.update_anchored(0.02, &|_| Some(anchor));
        assert!(close(e.sprites[0].position, Vec3::new(10.6, 1.2, -0.1)));
        // He turns left: the offset from the root is mirrored in x and z.
        anchor.rotation = Quat::euler_y_degrees(-90.0);
        e.update_anchored(0.02, &|_| Some(anchor));
        assert!(close(e.sprites[0].position, Vec3::new(11.4, 1.2, 0.0)));
        let turned = e.sprites[0].rotation * Vec3::RIGHT;
        assert!(close(turned, Vec3::new(-1.0, 0.0, 0.0)), "the sprite is turned with him");
    }

    #[test]
    fn a_part_flies_off_while_its_anchor_is_inactive_and_snaps_back_when_it_returns() {
        let mut rng = GameRng::from_seed(3);
        let a = Anchor { position: Vec3::new(0.0, 0.0, 0.0), rotation: Quat::IDENTITY, scale: Vec3::new(1.0, 1.0, 1.0) };
        let point = Vec3::new(0.5, 1.0, -0.1);
        let hit = CollisionData { point, last_rotation: Quat::IDENTITY, collided_with: Some((4, a)) };
        let mut e = Explosion::go_projectile(&[staple_part()], point, Some(&hit), true, 2.0, &mut rng, 0.02, &|_| Some(a));
        let v = e.sprites[0].motion.linear;
        // The avatar exploded: the staple flies with its random velocity.
        e.update_anchored(0.1, &|_| None);
        assert!(close(e.sprites[0].position, point + v * 0.1));
        e.update_anchored(0.1, &|_| None);
        assert!(close(e.sprites[0].position, point + v * 0.2));
        // Respawned within the explosion's lifetime: back on its local pose.
        e.update_anchored(0.1, &|_| Some(a));
        assert!(close(e.sprites[0].position, point));
    }

    #[test]
    fn non_sticking_parts_ricochet_from_the_collision_point() {
        let mut rng = GameRng::from_seed(4);
        let point = Vec3::new(2.0, 2.0, -0.1);
        let hit = CollisionData { point, last_rotation: Quat::IDENTITY, collided_with: Some((9, Anchor::IDENTITY)) };
        let mut e = Explosion::go_projectile(&[staple_part()], point, Some(&hit), false, 2.0, &mut rng, 0.0, &|_| Some(Anchor::IDENTITY));
        let v = e.sprites[0].motion.linear;
        e.update_anchored(0.25, &|_| Some(Anchor::IDENTITY));
        assert!(close(e.sprites[0].position, point + v * 0.25));
    }

    #[test]
    fn a_sticking_explosion_without_a_collision_freezes_its_parts_where_they_were() {
        let mut rng = GameRng::from_seed(5);
        let part = staple_part();
        let mut e = Explosion::go_projectile(std::slice::from_ref(&part), Vec3::ZERO, None, true, 2.0, &mut rng, 0.02, &|_| None);
        e.update_anchored(0.5, &|_| None);
        assert!(close(e.sprites[0].position, part.position), "stuck with no stuckTo: neither pinned nor moving");
        let mut inactive = staple_part();
        inactive.active = false;
        let e = Explosion::go_projectile(&[inactive], Vec3::ZERO, None, true, 2.0, &mut rng, 0.02, &|_| None);
        assert!(e.sprites.is_empty(), "inactive sources are not shown");
    }
}
