//! Melee effects (the Sword's "Sword Swish"): a kinematic body spawned by the launcher at its
//! launch point, turned to the fire direction (`Quaternion.AngleAxis(atan2(fire))`), moved to
//! the launch point every physics step (`FollowProjectileLauncher.FixedUpdate`:
//! `MovePosition`) and despawned when `SpawnExpiration` runs out.
//!
//! * `DamageOtherOnCollision.OnCollisionEnter2D`: every new contact between one of its colliders
//!   and a body on a layer its layer collides with damages that body, at most once per physics
//!   step (`damagedAlreadyThisFrame`).
//! * The "Deflector" (`DontExplodeOnCollisionWithMe`, "Sword Deflector" material: bounciness 1,
//!   friction 0): a projectile touching it does not explode but bounces off it (Box2D
//!   restitution against the moving kinematic body; none below Box2D's 1 m/s velocity
//!   threshold); `TakeResponsibilityForCollidingDamagers` then makes it unstoppable and puts it
//!   on the swinger's team (`SetResponsiblePlayer` → `Projectile.SetTeam`).
//!
//! TODO(UNCERTAIN): a projectile touching the "Damager" piece (not a deflector) would explode in
//! the original; only the deflector is tested against projectiles here (the damager sits behind
//! the deflector's front edge).

use unity_compat::Vec2;
use unity_data::projectile::MeleeSpec;

use crate::physics::{BodyShape, PhysicsRules, circle_convex_normal, convex_touches_body, merge_convex, triangulate};
use crate::weapons::{Shot, team_projectile_layer};

/// Box2D's `b2_velocityThreshold`: slower approaches do not bounce.
const VELOCITY_THRESHOLD: f32 = 1.0;

struct Piece {
    /// Convex, counter-clockwise parts in the swing's local space.
    convex: Vec<Vec<Vec2>>,
    deflector: bool,
    bounciness: f32,
}

/// A body the swing may hit.
pub struct SwingTarget<'a> {
    /// Caller's identity of the body (one `GameObject`).
    pub key: u64,
    pub position: Vec2,
    pub shape: &'a BodyShape,
    pub layer: u8,
}

/// One melee effect in flight.
pub struct Swing {
    pub position: Vec2,
    /// Velocity of the last `MovePosition` step (how the kinematic body moved).
    pub velocity: Vec2,
    /// Rotation about z (radians).
    pub angle: f32,
    pub layer: u8,
    pub team: i32,
    pub damage: f32,
    pub age: f32,
    lifetime: f32,
    takes_responsibility: bool,
    pieces: Vec<Piece>,
    /// (piece, body) contacts that existed after the last step.
    touching: Vec<(usize, u64)>,
}

impl Swing {
    /// The spawn: at `origin`, turned to `fire_direction`, on the team's projectile layer
    /// (`SetResponsiblePlayer` → `Projectile.SetTeam(gameObject, team)`).
    pub fn new(spec: &MeleeSpec, damage: f32, origin: Vec2, fire_direction: Vec2, team: i32) -> Swing {
        let pieces = spec
            .pieces
            .iter()
            .map(|p| {
                let path: Vec<Vec2> = p.path.iter().map(|q| Vec2::new(q[0], q[1])).collect();
                Piece { convex: merge_convex(triangulate(&path)), deflector: p.deflector, bounciness: p.bounciness }
            })
            .collect();
        Swing {
            position: origin,
            velocity: Vec2::ZERO,
            angle: fire_direction.y.atan2(fire_direction.x),
            layer: team_projectile_layer(team, false),
            team,
            damage,
            age: 0.0,
            lifetime: spec.lifetime,
            takes_responsibility: spec.takes_responsibility,
            pieces,
            touching: Vec::new(),
        }
    }

    fn world(&self, local: &[Vec2]) -> Vec<Vec2> {
        let (s, c) = self.angle.sin_cos();
        local.iter().map(|p| self.position + Vec2::new(c * p.x - s * p.y, s * p.x + c * p.y)).collect()
    }

    /// One physics step: move to `to` (`MovePosition`), then the contacts. Returns the keys of
    /// the bodies to damage this step (each once).
    pub fn fixed_step(&mut self, to: Vec2, dt: f32, targets: &[SwingTarget<'_>], rules: &PhysicsRules) -> Vec<u64> {
        if dt > 0.0 {
            self.velocity = (to - self.position) / dt;
        }
        self.position = to;
        let mut now = Vec::new();
        for (i, piece) in self.pieces.iter().enumerate() {
            let parts: Vec<Vec<Vec2>> = piece.convex.iter().map(|c| self.world(c)).collect();
            for t in targets {
                if !rules.layers_collide(self.layer, t.layer) {
                    continue;
                }
                if parts.iter().any(|poly| convex_touches_body(poly, t.position, t.shape)) {
                    now.push((i, t.key));
                }
            }
        }
        let mut damaged = Vec::new();
        for contact in &now {
            if !self.touching.contains(contact) && !damaged.contains(&contact.1) {
                damaged.push(contact.1);
            }
        }
        self.touching = now;
        damaged
    }

    /// `SpawnExpiration.Update` (scaled time): true once it has run out.
    pub fn update(&mut self, delta_time: f32) -> bool {
        self.age += delta_time;
        self.age >= self.lifetime
    }

    /// A projectile against the swing's other pieces (the blade's body, without
    /// `DontExplodeOnCollisionWithMe`): `TellOwnerToExplodeOnCollision` ends it there, and its
    /// pieces drift in the air where the sword met it. True when its circle touches one.
    pub fn strikes(&self, shot: &Shot, rules: &PhysicsRules) -> bool {
        if !rules.layers_collide(self.layer, shot.layer) {
            return false;
        }
        let (center, radius) = (shot.collider_center(), shot.radius());
        self.pieces.iter().filter(|p| !p.deflector).flat_map(|p| p.convex.iter()).any(|convex| circle_convex_normal(center, radius, &self.world(convex)).is_some())
    }

    /// A projectile against the deflectors: when its circle touches one while approaching it,
    /// it bounces off and, with `TakeResponsibilityForCollidingDamagers`, becomes an
    /// unstoppable projectile of the swing's team. True when it was deflected.
    pub fn deflect(&self, shot: &mut Shot, rules: &PhysicsRules) -> bool {
        if !rules.layers_collide(self.layer, shot.layer) {
            return false;
        }
        let center = shot.collider_center();
        let radius = shot.radius();
        for piece in self.pieces.iter().filter(|p| p.deflector) {
            for convex in &piece.convex {
                let poly = self.world(convex);
                let Some(normal) = circle_convex_normal(center, radius, &poly) else { continue };
                let relative = (shot.velocity - self.velocity).dot(normal);
                if relative >= 0.0 {
                    continue;
                }
                let restitution = if -relative < VELOCITY_THRESHOLD { 0.0 } else { piece.bounciness };
                shot.velocity -= normal * ((1.0 + restitution) * relative);
                if self.takes_responsibility {
                    shot.layer = team_projectile_layer(self.team, true);
                }
                return true;
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{BoxShape, CircleShape};
    use unity_data::projectile::MeleePiece;

    fn swish() -> MeleeSpec {
        MeleeSpec {
            name: "Sword Swish".into(),
            guid: String::new(),
            damage: 40.0,
            damage_scaling: None,
            lifetime: 0.25,
            takes_responsibility: true,
            pieces: vec![
                MeleePiece { path: vec![[1.5, 1.3], [-1.0, 0.0], [1.5, -1.3]], deflector: true, bounciness: 1.0 },
                MeleePiece { path: vec![[0.5, 1.3], [1.5, 0.0], [0.5, -1.3], [0.0, 0.0]], deflector: false, bounciness: 0.0 },
            ],
        }
    }

    fn rules() -> PhysicsRules {
        let mut matrix = [u32::MAX; 32];
        // Team 0 projectiles (12) do not touch team 0 characters (8).
        matrix[12] &= !(1 << 8);
        matrix[8] &= !(1 << 12);
        PhysicsRules { gravity: Vec2::new(0.0, -9.81), layer_matrix: matrix, raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 }
    }

    /// "Pistol Shot": not propelled, no gravity, radius 0.15, speed 21.
    fn bullet() -> unity_data::projectile::ProjectileSpec {
        unity_data::projectile::ProjectileSpec {
            name: "Pistol Shot".into(),
            guid: String::new(),
            layer: 12,
            spawn_z: -0.1,
            radius: 0.15,
            collider_offset: [0.0; 2],
            mass: 10.0,
            gravity_scale: 0.0,
            linear_drag: 0.0,
            angular_drag: 0.05,
            collision_detection: 1,
            damage: 20.0,
            damage_scaling: None,
            speed: 21.0,
            propelled: false,
            unstoppable: false,
            launch_angular_velocity: 0.0,
            rotates_to_face_velocity: false,
            lifetime: Some(2.0),
            despawns_on_expiration: true,
            explodes_on_expiration: false,
            explodes_on_collision: true,
            contact: unity_data::projectile::ContactExplosion::FirstContact,
            explosion: None,
            despawns_on_explode: false,
            despawns_on_explosion_finished: true,
            chargeable: false,
            speed_by_charge: None,
            seeker: None,
            spawns_on_explode: None,
            explosion_sound: None,
            lifetime_sound: None,
            lifetime_particles: None,
            sprites: Vec::new(),
        }
    }

    #[test]
    fn a_swing_hits_each_body_once() {
        let rules = rules();
        let enemy = BodyShape { circles: vec![CircleShape { offset: Vec2::new(0.0, 0.5), radius: 0.5 }], boxes: vec![BoxShape { offset: Vec2::new(0.0, 1.2), half_extents: Vec2::new(0.3, 0.6) }], polygons: vec![] };
        let me = enemy.clone();
        let targets = [
            SwingTarget { key: 1, position: Vec2::new(1.2, -0.5), shape: &enemy, layer: 9 },
            SwingTarget { key: 2, position: Vec2::new(-0.5, -0.5), shape: &me, layer: 8 },
        ];
        let mut swing = Swing::new(&swish(), 40.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0);
        assert_eq!(swing.fixed_step(Vec2::ZERO, 0.02, &targets, &rules), vec![1], "the swinger's own team is not hit");
        assert!(swing.fixed_step(Vec2::new(0.01, 0.0), 0.02, &targets, &rules).is_empty(), "still touching: no new contact");
        assert!(!swing.update(0.2));
        assert!(swing.update(0.05));
    }

    #[test]
    fn a_bullet_bounces_off_the_deflector_and_changes_sides() {
        let rules = rules();
        let swing = Swing::new(&swish(), 40.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0);
        let spec = bullet();
        let mut shot = Shot::launch(&spec, 20.0, Vec2::new(1.6, 0.0), Vec2::new(-1.0, 0.0), 0.0, 1, rules.gravity);
        assert!(swing.deflect(&mut shot, &rules));
        assert!(shot.velocity.x > 0.0, "{:?}", shot.velocity);
        assert_eq!(shot.layer, team_projectile_layer(0, true));
    }

    #[test]
    fn a_bullet_meeting_the_blade_body_is_stopped() {
        let rules = rules();
        let swing = Swing::new(&swish(), 40.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0);
        let spec = bullet();
        // Inside the body piece (x 0.5..1.5), clear of the deflector's front edge.
        let shot = Shot::launch(&spec, 20.0, Vec2::new(0.9, 0.0), Vec2::new(-1.0, 0.0), 0.0, 1, rules.gravity);
        assert!(swing.strikes(&shot, &rules));
        let far = Shot::launch(&spec, 20.0, Vec2::new(5.0, 0.0), Vec2::new(-1.0, 0.0), 0.0, 1, rules.gravity);
        assert!(!swing.strikes(&far, &rules));
    }
}
