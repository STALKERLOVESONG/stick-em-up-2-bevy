//! Weapons: aim rules, the launcher's fire timing, and projectile flight and hits.
//!
//! Recovered from `CharacterControl2D` (`CorrectAndSetAim`), `ProjectileLauncher`, `Projectile`,
//! `SpawnExpiration`, `DamageOtherOnCollision` and `DifficultyModifier`.

use unity_compat::{Quat, Vec2, Vec3};
use unity_data::projectile::ProjectileSpec;

use crate::physics::{BodyShape, PhysicsRules, World, circle_touches_body};

// ---------------------------------------------------------------------------------------------
// Aim
// ---------------------------------------------------------------------------------------------

/// `CharacterControl2D.aimingMinAngleToVertical`.
const AIMING_MIN_ANGLE_TO_VERTICAL: f32 = 0.2;
/// `CharacterControl2D.aimingMinDistance` and its square.
const AIMING_MIN_DISTANCE: f32 = 0.71;
const SQR_AIMING_MIN_DISTANCE: f32 = 0.504_099_97;

/// `CharacterControl2D.CorrectAndSetAim` without the `LookAt`: the aim vector the weapon really
/// points along. Near-vertical aims snap to a fixed almost-vertical direction, short aims are
/// stretched to a minimum length (an empty aim keeps `previous`), and the weapon never points
/// behind the facing direction.
pub fn corrected_aim(aim: Vec2, previous: Vec2, face_right: bool) -> Vec2 {
    let mut aim = aim;
    if aim.y > 0.0 {
        let angle = aim.x.atan2(aim.y);
        if angle.abs() < AIMING_MIN_ANGLE_TO_VERTICAL {
            aim = Vec2::new(AIMING_MIN_ANGLE_TO_VERTICAL.sin(), AIMING_MIN_ANGLE_TO_VERTICAL.cos());
        }
    }
    let sqr = aim.sqr_magnitude();
    if sqr < SQR_AIMING_MIN_DISTANCE {
        let length = sqr.sqrt();
        if length == 0.0 {
            aim = previous;
        } else {
            aim = aim * (AIMING_MIN_DISTANCE / length);
        }
    }
    aim.x = aim.x.abs() * if face_right { 1.0 } else { -1.0 };
    aim
}

/// The weapon pivot's world rotation after `LookAt(pivot + aim, up)`, with `up` pointing away
/// from the camera when facing right and towards it when facing left.
pub fn weapon_rotation(aim: Vec2, face_right: bool) -> Quat {
    let up = if face_right { Vec3::BACK } else { Vec3::FORWARD };
    Quat::look_rotation(Vec3::new(aim.x, aim.y, 0.0), up)
}

// ---------------------------------------------------------------------------------------------
// Weapon selection
// ---------------------------------------------------------------------------------------------

/// `CharacterControl2D.weaponNames` and `WeaponIndex`: the weapons a level keeps, in the order
/// of the `Weapon Pivots` children, and which one is up.
#[derive(Clone, Debug, PartialEq)]
pub struct Loadout {
    /// Indices into the rig's weapon list (`Rig::weapons`).
    weapons: Vec<usize>,
    index: usize,
}

impl Loadout {
    /// Keep the weapons `allowed` accepts (`AllowedWeapons.IsWeaponAllowed`) and start with the
    /// first (`Start` sets `WeaponIndex` to 0).
    pub fn new(names: &[&str], allowed: impl Fn(&str) -> bool) -> Loadout {
        Loadout { weapons: (0..names.len()).filter(|&i| allowed(names[i])).collect(), index: 0 }
    }

    /// The rig weapon that is up; `None` when the level allows no weapon at all.
    pub fn current(&self) -> Option<usize> {
        self.weapons.get(self.index).copied()
    }

    /// `input.WeaponChangingEnabled`.
    pub fn can_change(&self) -> bool {
        self.weapons.len() > 1
    }

    /// Development hook (not in the original): hold every rig weapon `0..count`, keeping the one
    /// that is up.
    pub fn unlock_all(&mut self, count: usize) {
        let current = self.current();
        self.weapons = (0..count).collect();
        self.index = current.and_then(|c| self.weapons.iter().position(|&w| w == c)).unwrap_or(0);
    }

    /// The rig weapons the loadout holds, in order (`weaponNames`).
    pub fn weapons(&self) -> &[usize] {
        &self.weapons
    }

    /// `weaponIndex` set to the loadout's entry for rig weapon `weapon` (`SelectedWeapon`); false
    /// when the loadout does not hold it.
    pub fn select(&mut self, weapon: usize) -> bool {
        match self.weapons.iter().position(|&w| w == weapon) {
            Some(index) => {
                self.index = index;
                true
            }
            None => false,
        }
    }

    /// `WeaponIndex += delta` with its wrap-around; true when a different weapon is now up.
    pub fn step(&mut self, delta: i32) -> bool {
        let count = self.weapons.len() as i32;
        if count == 0 {
            return false;
        }
        let mut value = self.index as i32 + delta;
        if value < 0 {
            value += count;
        }
        if value >= count {
            value -= count;
        }
        let value = value.clamp(0, count - 1) as usize;
        let changed = value != self.index;
        self.index = value;
        changed
    }
}

// ---------------------------------------------------------------------------------------------
// Launcher
// ---------------------------------------------------------------------------------------------

/// Fire timing of one `ProjectileLauncher`.
#[derive(Clone, Debug, PartialEq)]
pub struct Launcher {
    firing_rate_time: f32,
    fully_automatic: bool,
    chargeable: bool,
    charge_time: f32,
    next_fire_time: f32,
    waiting_for_release: bool,
}

/// A projectile the launcher wants spawned this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Launch {
    /// How far into the past the shot was due (`num2`): the projectile is advanced by this much
    /// flight time so that the fire rate does not depend on the frame rate.
    pub sub_frame_offset: f32,
    /// Seconds the trigger was held (charged weapons only).
    pub charge_time: f32,
}

impl Launcher {
    /// `ProjectileLauncher.firingRateTime`.
    pub fn firing_rate_time(&self) -> f32 {
        self.firing_rate_time
    }

    /// Development hook (not in the original): change the time between shots.
    pub fn set_firing_rate_time(&mut self, t: f32) {
        self.firing_rate_time = t;
    }

    pub fn new(firing_rate_time: f32, fully_automatic: bool, chargeable: bool) -> Launcher {
        Launcher { firing_rate_time, fully_automatic, chargeable, charge_time: 0.0, next_fire_time: 0.0, waiting_for_release: false }
    }

    /// `ProjectileLauncher.OnRespawned`.
    pub fn on_respawned(&mut self) {
        self.waiting_for_release = false;
    }

    /// `ProjectileLauncher.Update` up to the spawn: `time` is `Time.time`, `delta` is
    /// `Time.deltaTime`.
    pub fn update(&mut self, fire: bool, time: f32, delta: f32) -> Option<Launch> {
        let ready = if self.chargeable {
            if self.waiting_for_release {
                self.charge_time += delta;
                if !fire {
                    self.waiting_for_release = false;
                    true
                } else {
                    false
                }
            } else {
                if fire {
                    self.waiting_for_release = true;
                }
                false
            }
        } else if fire {
            self.fully_automatic || !self.waiting_for_release
        } else {
            self.waiting_for_release = false;
            false
        };
        if !ready {
            return None;
        }
        let late = time - self.next_fire_time;
        if !(late >= 0.0) {
            return None;
        }
        let sub_frame_offset = if late < delta { late } else { 0.0 };
        let launch = Launch { sub_frame_offset, charge_time: self.charge_time };
        self.next_fire_time = time - sub_frame_offset + self.firing_rate_time;
        if self.chargeable {
            self.charge_time = 0.0;
        } else {
            self.waiting_for_release = true;
        }
        Some(launch)
    }
}

// ---------------------------------------------------------------------------------------------
// Difficulty scaling
// ---------------------------------------------------------------------------------------------

/// `DifficultyModifier.Modify` for an authored `amount` (`"50%"` relative, `"x2"` multiplicative,
/// `"5"` additive). Campaign only; the caller skips it in other modes.
pub fn difficulty_modified(original: f32, amount: &str, affected_by_player_count: bool, player_count: i32, difficulty: i32) -> f32 {
    let mut text = amount;
    let relative = text.ends_with('%');
    if relative {
        text = &text[..text.len() - 1];
    }
    let multiple = text.starts_with('x');
    if multiple {
        text = &text[1..];
    }
    let mut modification: f32 = text.parse().unwrap_or(0.0);
    if relative {
        modification /= 100.0;
    }
    let mut steps = 0;
    if affected_by_player_count {
        steps += player_count - 1;
    }
    steps += difficulty;
    if multiple {
        original * modification.powi(steps)
    } else if relative {
        original + original * modification * steps as f32
    } else {
        original + modification * steps as f32
    }
}

// ---------------------------------------------------------------------------------------------
// Projectiles
// ---------------------------------------------------------------------------------------------

/// `Utility.Layers.TeamProjectiles`.
pub fn team_projectile_layer(team: i32, unstoppable: bool) -> u8 {
    (if unstoppable { 16 + team } else { 12 + team }) as u8
}

/// Whatever a projectile struck.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// A level collider (`StaticCollider::id`).
    Level(u32),
    /// The target at this index of the slice given to [`Shot::fixed_step`].
    Target(usize),
}

/// A body projectiles can hit: its collider shapes at a position, on a physics layer.
#[derive(Clone, Copy, Debug)]
pub struct ShotTarget<'a> {
    pub position: Vec2,
    pub shape: &'a BodyShape,
    pub layer: u8,
}

/// One projectile in flight.
#[derive(Clone, Debug, PartialEq)]
pub struct Shot {
    pub position: Vec2,
    pub velocity: Vec2,
    /// Rotation about z in degrees at launch (`Quaternion.AngleAxis(atan2(aim))`).
    pub angle_degrees: f32,
    /// `Rigidbody2D.rotation` now (degrees): the launch angle, turned by
    /// `launchAngularVelocity` (Shuriken) or `RotateToFaceVelocity` (Spear).
    pub rotation_degrees: f32,
    /// `TellOwnerToExplodeOnCollision.lastRotationBeforeContact`: the rotation in the last
    /// `Update` (see [`Shot::frame_update`]).
    pub rotation_at_last_update: f32,
    pub layer: u8,
    pub damage: f32,
    /// Where the last contact was made (the projectile's position in `OnCollisionEnter2D`).
    pub contact_point: Option<Vec2>,
    launch_velocity: Vec2,
    angular_velocity: f32,
    angular_drag: f32,
    rotates_to_face_velocity: bool,
    radius: f32,
    collider_offset: Vec2,
    gravity_scale: f32,
    propelled: bool,
    age: f32,
    lifetime: Option<f32>,
    /// Bodies the projectile is still touching: `OnCollisionEnter2D` fires once per contact.
    touching: Vec<Hit>,
}

/// `fireDirection.normalized` (zero stays zero below Unity's epsilon).
fn normalized(v: Vec2) -> Vec2 {
    let m = v.magnitude();
    if m > 1e-5 { v / m } else { Vec2::ZERO }
}

impl Shot {
    /// Centre of the projectile's circle collider.
    pub fn collider_center(&self) -> Vec2 {
        self.position + self.collider_offset
    }

    pub fn radius(&self) -> f32 {
        self.radius
    }

    /// `ProjectileLauncher.Update`'s spawn followed by `Projectile.SetTeam`: place the projectile
    /// at `origin`, advance it by the sub-frame offset, aim it along `fire_direction` and put it on
    /// its team's projectile layer. `damage` is the (already difficulty-scaled) hit damage.
    pub fn launch(spec: &ProjectileSpec, damage: f32, origin: Vec2, fire_direction: Vec2, sub_frame_offset: f32, team: i32, gravity: Vec2) -> Shot {
        Shot::launch_charged(spec, damage, origin, fire_direction, sub_frame_offset, team, gravity, None)
    }

    /// [`Shot::launch`] for a charged launch: `SetChargeTime(chargeTime)` reaches the prefab's
    /// `SetProjectileSpeedByCharge` before `SetTeam` reads `GetSpeedMultiplier()`. The launcher's
    /// sub-frame advance still uses the prefab's plain `speed`.
    #[allow(clippy::too_many_arguments)]
    pub fn launch_charged(
        spec: &ProjectileSpec,
        damage: f32,
        origin: Vec2,
        fire_direction: Vec2,
        sub_frame_offset: f32,
        team: i32,
        gravity: Vec2,
        charge_time: Option<f32>,
    ) -> Shot {
        let mut position = origin;
        if sub_frame_offset > 0.0 {
            let mut v = normalized(fire_direction) * spec.speed;
            if !spec.propelled {
                v += gravity * spec.gravity_scale;
            }
            position += v * sub_frame_offset;
        }
        // `Quaternion.AngleAxis(atan2(y, x) * 57.29578, forward)` then `TransformDirection(right)`.
        let angle_degrees = fire_direction.y.atan2(fire_direction.x) * 57.295_78;
        let q = Quat::axis_angle(Vec3::FORWARD, angle_degrees * 0.017_453_292);
        let right = q * Vec3::RIGHT;
        let mut speed = spec.speed;
        if let (Some(by_charge), Some(t)) = (spec.speed_by_charge, charge_time) {
            speed *= by_charge.multiplier(t);
        }
        let launch_velocity = Vec2::new(right.x, right.y) * speed;
        Shot {
            position,
            velocity: launch_velocity,
            angle_degrees,
            rotation_degrees: angle_degrees,
            rotation_at_last_update: angle_degrees,
            layer: team_projectile_layer(team, spec.unstoppable),
            damage,
            contact_point: None,
            launch_velocity,
            angular_velocity: spec.launch_angular_velocity,
            angular_drag: spec.angular_drag,
            rotates_to_face_velocity: spec.rotates_to_face_velocity,
            radius: spec.radius,
            collider_offset: Vec2::new(spec.collider_offset[0], spec.collider_offset[1]),
            gravity_scale: if spec.propelled { 0.0 } else { spec.gravity_scale },
            propelled: spec.propelled,
            age: 0.0,
            lifetime: spec.lifetime,
            touching: Vec::new(),
        }
    }

    /// `Seeker.FixedUpdate` (after `Projectile.FixedUpdate`, before the physics step): turn the
    /// body towards `target` by at most `max_degrees_per_second * dt` and fly at the
    /// projectile's speed along the new heading.
    /// TODO(UNCERTAIN): `Seeker` is taken to run after `Projectile` within the fixed step (script
    /// execution order is not set for either); the other order would only delay the turn of a
    /// propelled projectile by one step.
    pub fn seek(&mut self, target: Vec2, max_degrees_per_second: f32, dt: f32) {
        let to = target - self.position;
        let wanted = to.y.atan2(to.x) * 57.295_78;
        let delta = unity_compat::mathf::delta_angle(self.rotation_degrees, wanted);
        self.rotation_degrees += unity_compat::mathf::sign(delta) * delta.abs().min(max_degrees_per_second * dt);
        let radians = self.rotation_degrees * (std::f32::consts::PI / 180.0);
        let speed = self.launch_velocity.magnitude();
        let velocity = Vec2::new(radians.cos(), radians.sin()) * speed;
        // A propelled projectile's next `Projectile.FixedUpdate` keeps this velocity.
        self.launch_velocity = velocity;
        self.velocity = velocity;
    }

    /// `TellOwnerToExplodeOnCollision.Update` (before any contact): remember the rotation.
    pub fn frame_update(&mut self) {
        self.rotation_at_last_update = self.rotation_degrees;
    }

    /// `SpawnExpiration.Update`: ages by the frame time; true once the lifetime has run out.
    pub fn age_by(&mut self, delta: f32) -> bool {
        self.age += delta;
        self.lifetime.is_some_and(|lifetime| !(self.age < lifetime))
    }

    /// `Projectile.FixedUpdate`, `RotateToFaceVelocity.FixedUpdate` and the physics step: move
    /// along the velocity (gravity only for non-propelled projectiles) in sub-steps no longer
    /// than the projectile's radius, so thin colliders cannot be skipped. Returns the first new
    /// contact (`OnCollisionEnter2D`); [`Shot::contact_point`] says where it happened. A body the
    /// projectile keeps touching is not reported again.
    ///
    /// TODO(UNCERTAIN): Box2D's continuous collision (time of impact, then the contact solver) is
    /// approximated: the contact is located by bisection to where the circle first touches, and
    /// the projectile does not bounce off what it touched (only the Spear, whose explosion can be
    /// cancelled, keeps flying after a contact). `MoveRotation` on a dynamic body is taken to set
    /// the rotation for the step and leave the angular velocity alone.
    pub fn fixed_step(&mut self, world: &World, rules: &PhysicsRules, targets: &[ShotTarget<'_>], dt: f32) -> Option<Hit> {
        if self.propelled {
            self.velocity = self.launch_velocity;
        }
        if self.rotates_to_face_velocity {
            self.rotation_degrees = self.velocity.y.atan2(self.velocity.x) * 57.295_78;
        }
        if !self.propelled {
            self.velocity += rules.gravity * (self.gravity_scale * dt);
        }
        // Box2D: `w *= 1 / (1 + h * angularDamping)`, then `angle += h * w`.
        self.angular_velocity *= 1.0 / (1.0 + dt * self.angular_drag);
        self.rotation_degrees += dt * self.angular_velocity;
        let displacement = self.velocity * dt;
        let step_length = self.radius.max(0.01);
        let substeps = ((displacement.magnitude() / step_length).ceil() as usize).clamp(1, 512);
        let delta = displacement / substeps as f32;
        let mask = (0..32u8).filter(|&l| rules.layers_collide(self.layer, l)).fold(0u32, |m, l| m | 1 << l);
        for _ in 0..substeps {
            let before = self.position;
            self.position += delta;
            let overlapping = self.overlaps(world, rules, targets, mask, self.position);
            self.touching.retain(|h| overlapping.contains(h));
            let Some(&entered) = overlapping.iter().find(|h| !self.touching.contains(h)) else { continue };
            // Back off to where the circle first touches it.
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..12 {
                let mid = (lo + hi) * 0.5;
                let p = before + delta * mid;
                if self.overlaps(world, rules, targets, mask, p).contains(&entered) {
                    hi = mid;
                } else {
                    lo = mid;
                }
            }
            self.position = before + delta * hi;
            self.contact_point = Some(self.position);
            self.touching.extend(overlapping);
            return Some(entered);
        }
        None
    }

    /// A fixed step on layer 28 (`TellOwnerToExplodeOnCollision` after the contact): the body
    /// keeps moving but collides with nothing.
    pub fn coast(&mut self, gravity: Vec2, dt: f32) {
        if self.propelled {
            self.velocity = self.launch_velocity;
        } else {
            self.velocity += gravity * (self.gravity_scale * dt);
        }
        self.angular_velocity *= 1.0 / (1.0 + dt * self.angular_drag);
        self.rotation_degrees += dt * self.angular_velocity;
        self.position += self.velocity * dt;
    }

    /// Everything the projectile's circle overlaps at `at`: targets first (in order), then the
    /// level collider found there.
    fn overlaps(&self, world: &World, rules: &PhysicsRules, targets: &[ShotTarget<'_>], mask: u32, at: Vec2) -> Vec<Hit> {
        let centre = at + self.collider_offset;
        let mut out: Vec<Hit> = targets
            .iter()
            .enumerate()
            .filter(|(_, t)| rules.layers_collide(self.layer, t.layer) && circle_touches_body(centre, self.radius, t.position, t.shape))
            .map(|(i, _)| Hit::Target(i))
            .collect();
        if let Some(id) = world.overlap_circle_solid(centre, self.radius, mask) {
            out.push(Hit::Level(id));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{BoxShape, CircleShape, StaticCollider, StaticShape};

    fn staple() -> ProjectileSpec {
        ProjectileSpec {
            name: "Staple".into(),
            layer: 16,
            spawn_z: -0.1,
            radius: 0.05,
            collider_offset: [0.0; 2],
            mass: 10.0,
            gravity_scale: 0.0,
            linear_drag: 0.0,
            angular_drag: 0.05,
            collision_detection: 1,
            damage: 2.5,
            damage_scaling: None,
            speed: 25.0,
            propelled: true,
            unstoppable: true,
            launch_angular_velocity: 0.0,
            rotates_to_face_velocity: false,
            lifetime: Some(2.0),
            despawns_on_expiration: true,
            explodes_on_expiration: false,
            explodes_on_collision: true,
            contact: unity_data::projectile::ContactExplosion::FirstContact,
            explosion: Some(unity_data::projectile::ExplosionSpec { fade_wait_time: 2.0, sticks: true }),
            despawns_on_explode: false,
            despawns_on_explosion_finished: true,
            chargeable: false,
            speed_by_charge: None,
            seeker: None,
            spawns_on_explode: None,
            explosion_sound: None,
            lifetime_sound: None,
            lifetime_particles: None,
            guid: String::new(),
            sprites: Vec::new(),
        }
    }

    fn open_rules() -> PhysicsRules {
        PhysicsRules { gravity: Vec2::new(0.0, -16.076), layer_matrix: [u32::MAX; 32], raycasts_hit_triggers: true, box2d: Default::default(), velocity_iterations: 8, position_iterations: 3 }
    }

    fn wall_world(x: f32) -> World {
        let wall = vec![Vec2::new(x, -50.0), Vec2::new(x + 1.0, -50.0), Vec2::new(x + 1.0, 50.0), Vec2::new(x, 50.0)];
        World::build(vec![(StaticCollider { id: 4, layer: 0, ..Default::default() }, StaticShape::Polygon(wall))])
    }

    // --- aim ---------------------------------------------------------------------------------

    #[test]
    fn aim_never_points_behind_the_facing_direction() {
        let a = corrected_aim(Vec2::new(-3.0, 1.0), Vec2::new(1.0, 0.0), true);
        assert_eq!(a, Vec2::new(3.0, 1.0));
        let b = corrected_aim(Vec2::new(3.0, -1.0), Vec2::new(1.0, 0.0), false);
        assert_eq!(b, Vec2::new(-3.0, -1.0));
    }

    #[test]
    fn near_vertical_aims_snap_to_a_fixed_direction() {
        let a = corrected_aim(Vec2::new(0.05, 4.0), Vec2::new(1.0, 0.0), true);
        assert!((a.x - 0.2f32.sin()).abs() < 1e-7 && (a.y - 0.2f32.cos()).abs() < 1e-7);
        // Beyond the 0.2 rad cone the aim is untouched; downward aims never snap.
        assert_eq!(corrected_aim(Vec2::new(1.0, 4.0), Vec2::ZERO, true), Vec2::new(1.0, 4.0));
        assert_eq!(corrected_aim(Vec2::new(0.05, -4.0), Vec2::ZERO, true), Vec2::new(0.05, -4.0));
    }

    #[test]
    fn short_aims_are_stretched_and_an_empty_aim_keeps_the_last_one() {
        let a = corrected_aim(Vec2::new(0.1, 0.0), Vec2::new(1.0, 0.0), true);
        assert!((a.magnitude() - 0.71).abs() < 1e-6);
        let kept = corrected_aim(Vec2::ZERO, Vec2::new(0.6, 0.4), true);
        assert_eq!(kept, Vec2::new(0.6, 0.4));
    }

    #[test]
    fn the_weapon_points_along_the_aim_with_its_up_away_from_the_camera() {
        let q = weapon_rotation(Vec2::new(1.0, 0.0), true);
        let forward = q * Vec3::FORWARD;
        assert!((forward.x - 1.0).abs() < 1e-6 && forward.y.abs() < 1e-6);
        let up = q * Vec3::UP;
        assert!((up.z + 1.0).abs() < 1e-6, "local +y is world -z facing right: {up:?}");
        let left = weapon_rotation(Vec2::new(-1.0, 0.0), false);
        assert!(((left * Vec3::FORWARD).x + 1.0).abs() < 1e-6);
        assert!(((left * Vec3::UP).z - 1.0).abs() < 1e-6);
    }

    // --- selection ---------------------------------------------------------------------------

    #[test]
    fn the_loadout_keeps_allowed_weapons_in_rig_order_and_wraps_around() {
        let names = ["Assault Rifle", "Rocket Launcher", "Shuriken", "Pistol"];
        let mut loadout = Loadout::new(&names, |n| n == "Assault Rifle" || n == "Rocket Launcher");
        assert_eq!(loadout.current(), Some(0));
        assert!(loadout.can_change());
        assert!(loadout.select(1) && loadout.current() == Some(1));
        assert!(!loadout.select(2), "a weapon the level does not allow cannot be selected");
        loadout.select(0);
        assert!(loadout.step(1));
        assert_eq!(loadout.current(), Some(1));
        assert!(loadout.step(1), "past the end wraps to the first");
        assert_eq!(loadout.current(), Some(0));
        assert!(loadout.step(-1), "before the start wraps to the last");
        assert_eq!(loadout.current(), Some(1));
        let mut only = Loadout::new(&names, |n| n == "Pistol");
        assert!(!only.can_change());
        assert!(!only.step(1));
        assert_eq!(only.current(), Some(3));
        assert_eq!(Loadout::new(&names, |_| false).current(), None);
    }

    // --- launcher ----------------------------------------------------------------------------

    /// Run `frames` frames of `dt` with the trigger held and count the shots.
    fn shots_while_held(launcher: &mut Launcher, frames: usize, dt: f32) -> Vec<(f32, Launch)> {
        let mut out = Vec::new();
        let mut time = 0.0;
        for _ in 0..frames {
            time += dt;
            if let Some(l) = launcher.update(true, time, dt) {
                out.push((time, l));
            }
        }
        out
    }

    #[test]
    fn a_held_automatic_launcher_fires_at_its_rate_regardless_of_frame_time() {
        for dt in [1.0 / 144.0, 1.0 / 60.0, 1.0 / 30.0] {
            let mut launcher = Launcher::new(0.075, true, false);
            let seconds = 3.0;
            let shots = shots_while_held(&mut launcher, (seconds / dt) as usize, dt);
            // 0.075 s per shot → about 13.3 per second, including the one at the very start.
            let expected = (seconds / 0.075) as i32;
            assert!((shots.len() as i32 - expected).abs() <= 1, "dt {dt}: {} shots, expected ~{expected}", shots.len());
        }
    }

    #[test]
    fn late_shots_carry_the_time_they_were_overdue() {
        let mut launcher = Launcher::new(0.1, true, false);
        let first = launcher.update(true, 1.0, 0.02).unwrap();
        // Nothing was due before: more than a frame overdue, so no catch-up offset.
        assert_eq!(first.sub_frame_offset, 0.0);
        // next_fire_time = 1.1; at t = 1.11 (frame 0.02) the shot is 0.01 s overdue.
        assert!(launcher.update(true, 1.09, 0.02).is_none());
        let second = launcher.update(true, 1.11, 0.02).unwrap();
        assert!((second.sub_frame_offset - 0.01).abs() < 1e-6);
    }

    #[test]
    fn a_semi_automatic_launcher_needs_the_trigger_released_between_shots() {
        let mut launcher = Launcher::new(0.0, false, false);
        assert!(launcher.update(true, 1.0, 0.016).is_some());
        assert!(launcher.update(true, 1.016, 0.016).is_none(), "still held");
        assert!(launcher.update(false, 1.032, 0.016).is_none());
        assert!(launcher.update(true, 1.048, 0.016).is_some());
    }

    #[test]
    fn respawning_releases_a_held_trigger() {
        let mut launcher = Launcher::new(0.0, false, false);
        assert!(launcher.update(true, 1.0, 0.016).is_some());
        launcher.on_respawned();
        assert!(launcher.update(true, 1.016, 0.016).is_some());
    }

    #[test]
    fn a_chargeable_launcher_fires_on_release_with_the_held_time() {
        let mut launcher = Launcher::new(0.0, false, true);
        assert!(launcher.update(true, 1.0, 0.1).is_none(), "pressing starts charging");
        assert!(launcher.update(true, 1.1, 0.1).is_none());
        assert!(launcher.update(true, 1.2, 0.1).is_none());
        let shot = launcher.update(false, 1.3, 0.1).unwrap();
        assert!((shot.charge_time - 0.3).abs() < 1e-6, "{}", shot.charge_time);
        assert!(launcher.update(true, 1.4, 0.1).is_none());
    }

    // --- difficulty --------------------------------------------------------------------------

    #[test]
    fn difficulty_scaling_follows_the_authored_amount() {
        // Rocket: 30 damage, "50%": normal difficulty and one player leaves it unchanged.
        assert_eq!(difficulty_modified(30.0, "50%", true, 1, 0), 30.0);
        assert_eq!(difficulty_modified(30.0, "50%", true, 1, 1), 45.0);
        assert_eq!(difficulty_modified(30.0, "50%", true, 1, -1), 15.0);
        // Each extra player counts as another step.
        assert_eq!(difficulty_modified(30.0, "50%", true, 3, 0), 60.0);
        assert_eq!(difficulty_modified(30.0, "50%", false, 3, 0), 30.0);
        assert_eq!(difficulty_modified(10.0, "x2", true, 1, 2), 40.0);
        assert_eq!(difficulty_modified(10.0, "5", true, 1, 2), 20.0);
    }

    // --- flight ------------------------------------------------------------------------------

    #[test]
    fn a_staple_leaves_along_the_aim_at_its_speed_and_on_the_unstoppable_team_layer() {
        let shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 5.0), Vec2::new(3.0, 4.0), 0.0, 0, Vec2::new(0.0, -16.076));
        assert!((shot.velocity.x - 15.0).abs() < 1e-4 && (shot.velocity.y - 20.0).abs() < 1e-4, "{:?}", shot.velocity);
        assert_eq!(shot.layer, 16);
        assert!((shot.angle_degrees - 53.13).abs() < 0.01);
        let enemy = Shot::launch(&staple(), 2.5, Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 1, Vec2::ZERO);
        assert_eq!(enemy.layer, 17);
        let mut plain = staple();
        plain.unstoppable = false;
        assert_eq!(Shot::launch(&plain, 1.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO).layer, 12);
    }

    #[test]
    fn an_overdue_shot_is_advanced_along_its_flight() {
        let shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 0.0), Vec2::new(2.0, 0.0), 0.01, 0, Vec2::ZERO);
        assert!((shot.position.x - 0.25).abs() < 1e-5 && shot.position.y == 0.0);
    }

    #[test]
    fn propelled_shots_fly_straight_without_gravity() {
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 10.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let world = World::default();
        for _ in 0..30 {
            assert_eq!(shot.fixed_step(&world, &open_rules(), &[], 1.0 / 60.0), None);
        }
        assert!((shot.position.x - 12.5).abs() < 1e-3 && (shot.position.y - 10.0).abs() < 1e-5, "{:?}", shot.position);
    }

    #[test]
    fn an_unpropelled_shot_falls_under_gravity_scaled_by_its_rigidbody() {
        let mut spec = staple();
        spec.propelled = false;
        spec.gravity_scale = 1.0;
        let mut shot = Shot::launch(&spec, 1.0, Vec2::new(0.0, 100.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let world = World::default();
        for _ in 0..60 {
            shot.fixed_step(&world, &open_rules(), &[], 1.0 / 60.0);
        }
        assert!(shot.position.y < 100.0 - 7.0 && shot.position.y > 100.0 - 9.0, "{}", shot.position.y);
    }

    #[test]
    fn a_shot_stops_at_the_first_wall_even_when_a_step_would_cross_it() {
        let mut spec = staple();
        spec.speed = 500.0; // 8 units per 1/60 s step; the wall is 1 thick
        let mut shot = Shot::launch(&spec, 2.5, Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let world = wall_world(5.0);
        let hit = shot.fixed_step(&world, &open_rules(), &[], 1.0 / 60.0);
        assert_eq!(hit, Some(Hit::Level(4)));
        assert!(shot.position.x > 4.9 && shot.position.x < 5.2, "{}", shot.position.x);
    }

    #[test]
    fn a_shot_hits_a_body_on_a_colliding_layer_and_ignores_its_own_layer() {
        let shape = BodyShape {
            circles: vec![CircleShape { offset: Vec2::new(0.0, 0.6), radius: 0.6 }],
            boxes: vec![BoxShape { offset: Vec2::new(0.0, 1.5), half_extents: Vec2::new(0.575, 0.9) }], polygons: Vec::new()
        };
        let world = World::default();
        // Enemy man at x = 10 on layer 9; the shot was fired from team 0 (unstoppable layer 16).
        let enemy = ShotTarget { position: Vec2::new(10.0, 0.0), shape: &shape, layer: 9 };
        let mut rules = open_rules();
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 1.5), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let mut hit = None;
        for _ in 0..60 {
            hit = shot.fixed_step(&world, &rules, &[enemy], 1.0 / 60.0);
            if hit.is_some() {
                break;
            }
        }
        assert_eq!(hit, Some(Hit::Target(0)));
        assert!(shot.position.x > 9.3 && shot.position.x < 9.6, "stops at the box edge: {}", shot.position.x);

        // Switch the layer pair off in the matrix: the shot flies through.
        rules.layer_matrix[16] &= !(1 << 9);
        rules.layer_matrix[9] &= !(1 << 16);
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 1.5), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        for _ in 0..60 {
            assert_eq!(shot.fixed_step(&world, &rules, &[enemy], 1.0 / 60.0), None);
        }
        assert!(shot.position.x > 20.0);
    }

    #[test]
    fn a_shot_expires_after_its_lifetime() {
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let mut frames = 0;
        while !shot.age_by(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 1000);
        }
        assert!((frames as f32 / 60.0 - 2.0).abs() < 0.05, "{frames}");
    }

    #[test]
    fn the_contact_point_is_where_the_circle_first_touches_the_wall() {
        let world = wall_world(3.0);
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let mut hit = None;
        for _ in 0..30 {
            hit = shot.fixed_step(&world, &open_rules(), &[], 1.0 / 60.0);
            if hit.is_some() {
                break;
            }
        }
        assert_eq!(hit, Some(Hit::Level(4)));
        let p = shot.contact_point.unwrap();
        // Radius 0.05: the centre stops 0.05 short of the wall face at x = 3.
        assert!((p.x - 2.95).abs() < 1e-3, "contact at {p:?}");
    }

    #[test]
    fn spinning_and_velocity_facing_projectiles_turn_their_body() {
        let world = wall_world(1000.0);
        let mut shuriken = staple();
        shuriken.propelled = false;
        shuriken.gravity_scale = 1.0;
        shuriken.launch_angular_velocity = 360.0;
        let mut shot = Shot::launch(&shuriken, 25.0, Vec2::new(0.0, 50.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        shot.fixed_step(&world, &open_rules(), &[], 0.02);
        // w *= 1 / (1 + 0.02 * 0.05), then angle += 0.02 w.
        let w = 360.0 / (1.0 + 0.02 * 0.05);
        assert!((shot.rotation_degrees - 0.02 * w).abs() < 1e-3);
        assert_eq!(shot.rotation_at_last_update, 0.0, "the explosion uses the rotation from the last Update");
        shot.frame_update();
        assert_eq!(shot.rotation_at_last_update, shot.rotation_degrees);

        let mut spear = shuriken.clone();
        spear.launch_angular_velocity = 0.0;
        spear.rotates_to_face_velocity = true;
        let mut shot = Shot::launch(&spear, 120.0, Vec2::new(0.0, 50.0), Vec2::new(1.0, 1.0), 0.0, 0, Vec2::ZERO);
        for _ in 0..20 {
            shot.fixed_step(&world, &open_rules(), &[], 0.02);
        }
        // `MoveRotation(atan2(v))` reads the velocity before this step's gravity.
        let before_gravity = shot.velocity - open_rules().gravity * 0.02;
        let expected = before_gravity.y.atan2(before_gravity.x) * 57.295_78;
        assert!((shot.rotation_degrees - expected).abs() < 1e-3);
        assert!(shot.rotation_degrees < 45.0, "the spear noses down as it falls");
    }

    #[test]
    fn a_body_still_being_touched_is_not_entered_again() {
        let world = World::default();
        let shape = BodyShape { circles: Vec::new(), boxes: vec![BoxShape { offset: Vec2::new(0.0, 1.0), half_extents: Vec2::new(0.5, 1.0) }], polygons: Vec::new() };
        let enemy = ShotTarget { position: Vec2::new(1.0, 0.0), shape: &shape, layer: 9 };
        let mut shot = Shot::launch(&staple(), 2.5, Vec2::new(0.0, 1.0), Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO);
        let mut enters = 0;
        for _ in 0..10 {
            if shot.fixed_step(&world, &open_rules(), &[enemy], 1.0 / 120.0).is_some() {
                enters += 1;
            }
        }
        assert_eq!(enters, 1, "OnCollisionEnter2D once while passing through");
    }

    #[test]
    fn a_charged_spear_flies_at_its_charge_multiplier() {
        let mut spear = staple();
        spear.speed = 40.0;
        spear.speed_by_charge = Some(unity_data::projectile::SpeedByCharge { maximum_charge_time: 1.6, minimum_charge_time: 0.25 });
        let shot = Shot::launch_charged(&spear, 1.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO, Some(0.8));
        // min(1, 0.8 * 0.75 / 1.6 + 0.25) = 0.625
        assert!((shot.velocity.x - 25.0).abs() < 1e-4);
        let full = Shot::launch_charged(&spear, 1.0, Vec2::ZERO, Vec2::new(1.0, 0.0), 0.0, 0, Vec2::ZERO, Some(5.0));
        assert!((full.velocity.x - 40.0).abs() < 1e-4);
    }
}
