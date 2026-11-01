//! Helicopters (Skyline 2, Corruption 2): `HelicopterControl` bodies flying under the Box2D
//! integration of `sticks_sim::rigid_body`, enemy helicopters' controller (targeting,
//! `HelicopterMoveToFirstTargetHeight`, `HelicopterFireAtFirstTarget`, guns turned to the target
//! and firing their launchers, `DeactivateOffScreen`), damage with the fuselage hit flash, the
//! sprites of the "Visuals" tree moving with the body, and the rotors' blade lines.
//!
//! Players board, fly and leave these through [`crate::vehicles`] (`ControlSlot`); this module
//! keeps the slot, the pilot's input, the single-player gun, the camera influencer and the
//! engine state the boarding systems and the audio read.
//!
//! H7: hulls collide with the level and each other through `sticks_sim::contact` (Box2D's
//! contact solver) and push characters as moving obstacles.
//!
//! H4: when its `Health` runs out (`TellOwnerToExplodeOnHealthExhausted` → `Explodable.Explode`)
//! the controllers are ejected (`EjectControllersOnExplode`, `crate::vehicles`), its sound plays
//! (`PlaySoundOnExplode`) and its sprites fly apart (`ExplodeSpritesAndLines`; the object
//! switches off). When the explosion is over (`ExplosionFinished`) a player helicopter comes
//! back where it started with full health (`RespawnerInstantiator` → `Respawner`,
//! `RespawnAtStartPosition`) and an enemy one is gone (`DestroyRootOnRootExplosionFinished`).
//! TODO(UNCERTAIN): the rotor blades are not among the exploding parts, and a respawned
//! helicopter starts at rest.
//!
//! The seated occupants (`VehicleOccupant`) are drawn with [`ManArt::occupant_renderers`]; a
//! gunner seat's occupant shows its gunner (`HelicopterGunnerControl`, boarded in `vehicles`).
//! TODO(UNCERTAIN): `HelicopterInput` and the AI behaviours share execution order 0; the input is
//! taken to copy the processor before the behaviours run (component order), so the flight acts on
//! the previous frame's decision, as for enemy soldiers.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::ai::{Candidate, OffScreenSleep, Targeter};
use sticks_sim::draw::{BACK, Geometry, color32, scene_sprite_geometry, shadow_offset_and_scale, sprite_shadow_geometry};
use sticks_sim::helicopter::{HelicopterControl, back_rotor_blades, gun_angle_degrees, move_to_first_target_height, top_rotor_blades};
use sticks_sim::life::Health;
use sticks_sim::contact::{DynamicBody, Polygon, WarmStart, mass_properties};
use sticks_sim::physics::{BodyShape, StaticCollider, merge_convex, triangulate};
use sticks_sim::rigid_body::RigidBody;
use sticks_sim::visual::{LINE_TRIANGLES, StickLine, line_vertices};
use sticks_sim::vehicles::{ControlSlot, ExitLatch, OCCUPIED_ENGINE_VOLUME, UNOCCUPIED_ENGINE_VOLUME, helicopter_engine_pitch, team_vehicles_layer};
use sticks_sim::weapons::{Launcher, Shot, difficulty_modified};
use unity_compat::{Quat as SimQuat, Vec2 as SimVec2, Vec3 as SimVec3};
use unity_data::math::Affine;
use unity_data::sprites::{SpriteDef, SpriteLibrary, TiledLayout};
use unity_data::vehicles::{HelicopterSpawn, LoopingSound};

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::effects::PlaySound;
use crate::enemy::ENEMY_TEAM;
use crate::figure::ManArt;
use crate::level::{ActiveLevel, Collision};
use crate::life::Mortal;
use crate::player::{Avatar, ControlledBy, FrameClock};
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, TextureLoad, UnityLitMaterial, UnityTextures, camera_view};
use crate::weapon::{Projectile, ProjectileLibrary};

/// `CharacterControl2D`'s target height above the feet (the `Target` child), as enemies use it.
const CAMERA_TARGET_HEIGHT: f32 = 1.8;
/// `LinesRenderer.AddLineWithShadow`: shadow lines sit 0.003 behind.
const LINE_SHADOW_Z: f32 = 0.003;

/// A sprite under the helicopter root and its `SpriteShadower` copy.
struct HeliSprite {
    node: usize,
    def: SpriteDef,
    tiled: Option<TiledLayout>,
    scale: [f32; 3],
    color: [f32; 4],
    texture: u32,
    shadow: Option<(SpriteDef, u32, f32)>,
    /// The gun this sprite belongs to (its `HelicopterGunnerColor` rocket launcher).
    gun: Option<usize>,
}

/// One gun: launcher, its current world angle, whether it is switched on.
struct Gun {
    launcher: Launcher,
    angle: f32,
    active: bool,
    /// Its gunner seat (`HelicopterGunnerControl` with a `ControlSlot`, players' helicopters).
    pub(crate) seat: Option<GunnerSeat>,
}

/// A gunner seat: who sits in it and their `HelicopterGunnerInput` this frame.
pub(crate) struct GunnerSeat {
    pub(crate) slot: ControlSlot<Entity>,
    pub(crate) exit_latch: ExitLatch,
    pub(crate) exit: bool,
    pub(crate) fire: bool,
    pub(crate) aim: Option<HeliAim>,
    /// The gunner's team (their shots' layer).
    pub(crate) team: u8,
}

impl GunnerSeat {
    pub(crate) fn gunner(&self) -> Option<Entity> {
        self.slot.controller()
    }
}

#[derive(Component)]
pub struct Helicopter {
    pub(crate) spec: Arc<HelicopterSpawn>,
    pub body: RigidBody,
    control: HelicopterControl,
    /// `HelicopterInput` (copied from the processor) and the processor itself
    /// (`EnemyHelicopterInput`): movement, fire, absolute aim.
    pub(crate) input: (SimVec2, bool, Option<HeliAim>),
    processor: (SimVec2, bool, Option<SimVec2>),
    /// Somebody (or the AI) controls it (`HelicopterControl.Input != null`).
    controlled: bool,
    /// The root `ControlSlot` of a player's helicopter; the controller is the player slot entity.
    pub(crate) slot: Option<ControlSlot<Entity>>,
    /// `HelicopterControl.controllerJustSet`.
    pub(crate) exit_latch: ExitLatch,
    /// `HelicopterInput.Exit` of the pilot this frame.
    pub(crate) exit: bool,
    /// The team of the player flying it (`Player.TeamIndex`).
    pilot_team: u8,
    /// `IsPilotAlsoAGunner` when the pilot took control (one active player): the single-player
    /// gun gets the pilot's input.
    pilot_is_gunner: bool,
    /// `FixWorldOffsetFromTransform` on the "Camera Influencer": its world offset from the root
    /// (taken in `Awake`) and its position (moved in `FixedUpdate`).
    camera_offset: Option<SimVec2>,
    camera_point: SimVec2,
    /// `HelicopterControl.engineSound.volume`.
    engine_volume: f32,
    guns: Vec<Gun>,
    rotor_angles: Vec<f32>,
    health: Health,
    /// `HelicopterColor`: the standard fuselage colour and the remaining hit-flash time.
    fuselage_color: [f32; 4],
    flash: f32,
    targeter: Option<Targeter>,
    sleeper: Option<OffScreenSleep>,
    asleep: Option<SimVec2>,
    /// Collider pieces (counter-clockwise triangles) in the root's local space.
    hull: Vec<Vec<SimVec2>>,
    /// The hull as Box2D polygons in body space (for the contact solver).
    pieces: Vec<Polygon>,
    /// The hull as projectiles see it (rotated to the body's angle, relative to its position).
    pub shape: BodyShape,
    pub layer: u8,
    sprites: Vec<HeliSprite>,
    node_active: Vec<bool>,
    root_z: f32,
    root_scale: [f32; 3],
    /// Exhausted, not exploded yet (the explosion is built where the drawing state is).
    explode_pending: bool,
    /// `ExplodeSpritesAndLines`: the parts in flight and the art they are drawn with.
    explosion: Option<(sticks_sim::explosion::Explosion, sticks_sim::visual::VisualArt)>,
    /// `RespawnAtStartPosition`: where the body started.
    start: (SimVec2, f32),
}

impl Helicopter {
    fn root_affine(&self) -> Affine {
        let q = SimQuat::about_z(self.body.angle);
        Affine::from_trs([self.body.position.x, self.body.position.y, self.root_z], (q.x, q.y, q.z, q.w), self.root_scale)
    }

    /// World transform of a node; guns face their world angle (`transform.eulerAngles`).
    fn node_world(&self, i: usize, root: &Affine) -> Affine {
        let n = &self.spec.nodes[i];
        let Some(parent) = n.parent else { return *root };
        let p = self.node_world(parent, root);
        let r = n.local_rotation;
        let mut w = p.mul(&Affine::from_trs(n.local_position, (r[0], r[1], r[2], r[3]), n.local_scale));
        if let Some(g) = self.spec.guns.iter().position(|g| g.node == i) {
            let s = w.lossy_scale();
            let q = SimQuat::about_z(self.guns[g].angle.to_radians());
            w = Affine::from_trs(w.t, (q.x, q.y, q.z, q.w), s);
        }
        w
    }

    fn active_in_hierarchy(&self, node: usize) -> bool {
        let mut current = Some(node);
        while let Some(c) = current {
            if !self.node_active.get(c).copied().unwrap_or(false) {
                return false;
            }
            current = self.spec.nodes[c].parent;
        }
        true
    }

    fn node_rotation(&self, i: usize) -> SimQuat {
        let mut q = SimQuat::IDENTITY;
        let mut current = Some(i);
        let mut chain = Vec::new();
        while let Some(c) = current {
            chain.push(c);
            current = self.spec.nodes[c].parent;
        }
        for &c in chain.iter().rev() {
            if let Some(g) = self.spec.guns.iter().position(|g| g.node == c) {
                q = SimQuat::about_z(self.guns[g].angle.to_radians());
            } else if self.spec.nodes[c].parent.is_none() {
                q = SimQuat::about_z(self.body.angle);
            } else {
                q = q * SimQuat::from_array(self.spec.nodes[c].local_rotation);
            }
        }
        q
    }

    fn rebuild_shape(&mut self) {
        let (s, c) = self.body.angle.sin_cos();
        let scale = self.root_scale;
        self.shape.polygons = self
            .hull
            .iter()
            .map(|piece| piece.iter().map(|p| SimVec2::new(c * p.x * scale[0] - s * p.y * scale[1], s * p.x * scale[0] + c * p.y * scale[1])).collect())
            .collect();
    }

    pub(crate) fn alive(&self) -> bool {
        !self.health.is_exhausted()
    }

    /// Projectiles can hit it: awake and not destroyed.
    pub fn is_target(&self) -> bool {
        self.asleep.is_none() && self.alive()
    }

    /// Its health is not exhausted (it has not exploded).
    pub fn is_alive(&self) -> bool {
        self.alive()
    }

    /// The root object is active (`DeactivateOffScreen` deactivates it while asleep).
    pub fn is_active_object(&self) -> bool {
        self.asleep.is_none() && self.alive()
    }

    /// Development hook (not in the original): full health again.
    pub(crate) fn refill_health(&mut self) {
        if self.alive() {
            let _ = self.health.reset();
        }
    }

    /// `Health.startingHealth` (difficulty-modified).
    pub fn starting_health(&self) -> f32 {
        self.health.starting()
    }

    /// An enemy helicopter (`EnemyHelicopterInput`).
    pub fn is_enemy(&self) -> bool {
        self.spec.enemy.is_some()
    }

    /// Health left over starting health (`Health.Normalized`, what the pilot's HUD bar shows).
    pub fn health_normalized(&self) -> f32 {
        self.health.normalized()
    }

    /// The root `ControlSlot`'s transform position.
    pub fn slot_position(&self) -> SimVec3 {
        SimVec3::new(self.body.position.x, self.body.position.y, self.root_z)
    }

    /// The "Disembarkment Point" (`Player.SetControlled` looks it up under the root).
    pub fn disembarkment_point(&self) -> Option<SimVec3> {
        let node = self.spec.disembarkment_point?;
        let p = self.node_world(node, &self.root_affine()).t;
        Some(SimVec3::new(p[0], p[1], p[2]))
    }

    /// `SetController` (the player's `Player.SetUp` puts the tree on the team's vehicle layer).
    pub(crate) fn take_control(&mut self, player: Entity, team: u8, active_players: usize) {
        let Some(slot) = self.slot.as_mut() else { return };
        slot.set_controller(player);
        // `if (IsPilotAlsoAGunner()) singlePlayerGunnerControl.ProjectileLauncherInput = ...`.
        self.pilot_is_gunner = active_players == 1;
        self.exit_latch.on_set_controller();
        self.controlled = true;
        self.pilot_team = team;
        self.engine_volume = OCCUPIED_ENGINE_VOLUME;
        self.layer = team_vehicles_layer(team);
    }

    /// `ControlSlot.RelinquishControl` → `ClearController`: the input goes (the last movement
    /// stays in `movementControl`), the gun loses its input, the engine quietens. The layer is
    /// left as `SetUp` set it.
    pub(crate) fn relinquish(&mut self) -> Option<Entity> {
        let player = self.slot.as_mut()?.clear_controller();
        self.controlled = false;
        self.exit = false;
        self.engine_volume = UNOCCUPIED_ENGINE_VOLUME;
        player
    }

    /// The player slot flying it.
    pub fn pilot(&self) -> Option<Entity> {
        self.slot.and_then(|s| s.controller())
    }

    /// Whether gun `g`'s `ProjectileLauncher` has an input: enemies' `EnemyHelicopterInput` gives
    /// every gunner control its input; a player's helicopter only gives the pilot's input to
    /// `singlePlayerGunnerControl`, and only when the pilot is also a gunner
    /// (`IsPilotAlsoAGunner`: one active player when control was taken). The other guns' own
    /// `ControlSlot`s are gunner seats for further players (their own input aims and fires it).
    fn gun_has_input(&self, g: usize) -> bool {
        if self.spec.enemy.is_some() {
            return self.controlled;
        }
        self.gunner(g).is_some() || (self.pilot().is_some() && self.pilot_is_gunner && self.spec.single_player_gun == Some(g))
    }

    /// The player in gun `g`'s seat.
    pub(crate) fn gunner(&self, g: usize) -> Option<Entity> {
        self.guns.get(g).and_then(|gun| gun.seat.as_ref()).and_then(GunnerSeat::gunner)
    }

    /// The gunner seats (gun index, seat), for boarding and input.
    pub(crate) fn seats_mut(&mut self) -> impl Iterator<Item = (usize, &mut GunnerSeat)> {
        self.guns.iter_mut().enumerate().filter_map(|(g, gun)| gun.seat.as_mut().map(|s| (g, s)))
    }

    pub(crate) fn seats(&self) -> impl Iterator<Item = (usize, &GunnerSeat)> {
        self.guns.iter().enumerate().filter_map(|(g, gun)| gun.seat.as_ref().map(|s| (g, s)))
    }

    /// `HelicopterGunnerControl.SetController`: `player` takes gun `g`'s seat; its launchers follow
    /// their `HelicopterGunnerInput` and their shots go on the gunner's team layer.
    pub(crate) fn take_seat(&mut self, g: usize, player: Entity, team: u8) {
        let Some(seat) = self.guns.get_mut(g).and_then(|gun| gun.seat.as_mut()) else { return };
        seat.slot.set_controller(player);
        seat.exit_latch.on_set_controller();
        seat.team = team;
        seat.fire = false;
        seat.exit = false;
    }

    /// `HelicopterGunnerControl.ClearController`: the launchers lose the gunner's input.
    pub(crate) fn leave_seat(&mut self, g: usize) -> Option<Entity> {
        let seat = self.guns.get_mut(g).and_then(|gun| gun.seat.as_mut())?;
        seat.fire = false;
        seat.exit = false;
        seat.aim = None;
        seat.slot.clear_controller()
    }

    /// Where gun `g`'s seat is (its `ControlSlot` sits on the gun).
    pub(crate) fn seat_position(&self, g: usize) -> SimVec3 {
        let p = self.node_world(self.spec.guns[g].node, &self.root_affine()).t;
        SimVec3::new(p[0], p[1], p[2])
    }

    /// The aim and trigger gun `g` follows: its gunner's, else the pilot's.
    fn gun_input(&self, g: usize) -> (bool, Option<HeliAim>) {
        match self.guns.get(g).and_then(|gun| gun.seat.as_ref()).filter(|s| s.gunner().is_some()) {
            Some(seat) => (seat.fire, seat.aim),
            None => (self.input.1, self.input.2),
        }
    }

    /// `CameraPositionInfluencer` on the "Camera Influencer", enabled while the root slot has a
    /// controller (`LinkBehaviourEnablementToControlSlotControlledness`).
    pub fn camera_influence(&self) -> Option<SimVec2> {
        (self.camera_offset.is_some() && self.pilot().is_some() && self.is_active_object()).then_some(self.camera_point)
    }

    /// The "Target" `Targetable` while the helicopter is active; `controlled` is what
    /// `IsRootControlledTargetQualifier` checks (the root slot's controller).
    pub fn target_candidate(&self) -> Option<Candidate> {
        let node = self.spec.target?;
        if !self.is_active_object() {
            return None;
        }
        let p = self.node_world(node, &self.root_affine()).t;
        Some(Candidate { position: SimVec3::new(p[0], p[1], p[2]), controlled: self.pilot().is_some() })
    }

    /// The engine `AudioSource` while the object is active: its settings, position, the volume
    /// `HelicopterControl` set and the pitch it derives from `movementControl`.
    pub fn engine(&self) -> Option<(&LoopingSound, SimVec3, f32, f32)> {
        let sound = self.spec.engine_sound.as_ref()?;
        self.is_active_object().then(|| (sound, self.slot_position(), self.engine_volume, helicopter_engine_pitch(self.control.movement_control)))
    }

    /// `Health.Remaining`.
    pub fn health_remaining(&self) -> f32 {
        self.health.remaining()
    }

    /// The owner's streams: `HelicopterInput.OnSerializeNetworkView` and the hull's
    /// `SynchronizeRigidBody2D`.
    pub(crate) fn net_state(&self, id: sticks_net::NetId) -> sticks_net::HelicopterState {
        sticks_net::HelicopterState {
            id,
            movement: [self.input.0.x, self.input.0.y],
            fire: self.input.1,
            aim: self.input.2.map(|a| match a {
                HeliAim::Absolute(p) => sticks_net::WireAim::Absolute([p.x, p.y]),
                HeliAim::Relative(d) => sticks_net::WireAim::Relative([d.x, d.y]),
            }),
            exit: self.exit,
            position: [self.body.position.x, self.body.position.y],
            velocity: [self.body.linear_velocity.x, self.body.linear_velocity.y],
            rotation: self.body.angle,
            angular_velocity: self.body.angular_velocity,
            active: self.asleep.is_none(),
        }
    }

    /// The owner's input (every frame).
    pub(crate) fn apply_net_input(&mut self, s: &sticks_net::HelicopterState, exit: bool) {
        let aim = s.aim.map(|a| match a {
            sticks_net::WireAim::Absolute(p) => HeliAim::Absolute(SimVec2::new(p[0], p[1])),
            sticks_net::WireAim::Relative(d) => HeliAim::Relative(SimVec2::new(d[0], d[1])),
        });
        self.input = (SimVec2::new(s.movement[0], s.movement[1]), s.fire, aim);
        if self.pilot().is_some() {
            self.exit = exit;
        }
    }

    /// The owner's hull: `SynchronizeRigidBody2D` takes the owner's velocities and moves the body
    /// towards the owner's position (jumping there when more than 10 units, or 45°, off).
    /// TODO(UNCERTAIN): the original also extrapolates by the message's age and smooths only the
    /// visuals; a tenth of the way per update is used here, as for characters.
    pub(crate) fn apply_net_body(&mut self, s: &sticks_net::HelicopterState) {
        let target = SimVec2::new(s.position[0], s.position[1]);
        let off = target - self.body.position;
        self.body.position = if off.sqr_magnitude() < 100.0 { self.body.position + off * 0.1 } else { target };
        let turn = unity_compat::mathf::delta_angle(self.body.angle.to_degrees(), s.rotation.to_degrees());
        self.body.angle = if turn.abs() < 45.0 { self.body.angle + turn.to_radians() * 0.1 } else { s.rotation };
        self.body.linear_velocity = SimVec2::new(s.velocity[0], s.velocity[1]);
        self.body.angular_velocity = s.angular_velocity;
        self.asleep = if s.active { None } else { Some(target) };
    }

    /// `SyncServerDamageToClients.ChangedRemotely` for the hull's `Health`.
    pub(crate) fn receive_health(&mut self, remaining: f32) {
        let current = self.health.remaining();
        if remaining < current {
            let was_alive = self.alive();
            self.health.damage(current - remaining);
            if self.alive() {
                self.flash = 0.1;
            } else if was_alive {
                info!("{} destroyed", self.spec.name);
                self.explode_pending = true;
            }
        } else if remaining == self.health.starting() {
            self.health.reset();
        }
    }

    /// The root transform, for explosion parts pinned to the helicopter.
    pub fn anchor(&self) -> sticks_sim::explosion::Anchor {
        sticks_sim::explosion::Anchor {
            position: unity_compat::Vec3::new(self.body.position.x, self.body.position.y, self.root_z),
            rotation: SimQuat::about_z(self.body.angle),
            scale: unity_compat::Vec3::new(self.root_scale[0], self.root_scale[1], self.root_scale[2]),
        }
    }
}

/// A projectile struck a helicopter (`DamageOtherOnCollision` → `Health.Damage`).
#[derive(Message, Debug, Clone, Copy)]
pub struct HelicopterHit {
    pub helicopter: Entity,
    pub amount: f32,
    /// The player slot credited with the damage (`DamageOtherOnCollision.responsiblePlayer`).
    pub responsible: Option<Entity>,
}

pub struct HelicopterPlugin;

impl Plugin for HelicopterPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<HelicopterHit>()
            .add_systems(OnEnter(AppState::InGame), spawn_helicopters.after(crate::figure::load_man_art))
            .add_systems(
                Update,
                (
                    copy_helicopter_input.in_set(crate::FrameSet::RoleInput),
                    publish_hulls.in_set(crate::FrameSet::Input),
                    fly_helicopters.in_set(crate::FrameSet::FixedSteps).before(crate::weapon::step_projectiles),
                    hit_helicopters.in_set(crate::FrameSet::FixedSteps).after(crate::weapon::step_projectiles),
                    explode_helicopters.in_set(crate::FrameSet::Characters),
                    (sleep_helicopters.run_if(crate::net::decides), think_helicopters, fire_helicopter_guns).chain().in_set(crate::FrameSet::Characters),
                ),
            )
            .add_systems(Update, queue_helicopters.in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_helicopters(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if level.0.helicopters.is_empty() {
        return;
    }
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut libraries: HashMap<String, SpriteLibrary> = HashMap::new();
    let modified = |value: f32, scaling: &Option<unity_data::projectile::DifficultyScaling>| match scaling {
        Some(s) if session.mode == GameMode::Campaign => difficulty_modified(value, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
        _ => value,
    };
    for spec in &level.0.helicopters {
        let spec = Arc::new(spec.clone());
        // Which objects are active (`EnemyHelicopterGunnerCount.Awake` switches gunners/guns).
        let mut node_active: Vec<bool> = spec.nodes.iter().map(|n| n.active_self).collect();
        if let Some((count, gunners, launchers)) = spec.enemy.as_ref().and_then(|e| e.gunner_count.clone()) {
            for i in 0..3 {
                for list in [&gunners, &launchers] {
                    if let Some(&n) = list.get(i) {
                        node_active[n] = i < count;
                    }
                }
            }
        }
        let in_hierarchy = |i: usize| {
            let mut current = Some(i);
            while let Some(c) = current {
                if !node_active[c] {
                    return false;
                }
                current = spec.nodes[c].parent;
            }
            true
        };
        // Sprites under the root and their shadow copies.
        let mut sprites = Vec::new();
        let body_color = spec.enemy.as_ref().map(|e| e.body_color.map(|v| f32::from(v) / 255.0));
        for (index, sprite) in level.0.sprites.iter().enumerate() {
            if sprite.rigid_root != Some(spec.root) || sprite.shadow_of.is_some() {
                continue;
            }
            let Some(node) = spec.node_of(sprite.object) else { continue };
            if !libraries.contains_key(&sprite.collection) {
                match SpriteLibrary::load(&assets_dir, &sprite.collection) {
                    Ok(lib) => {
                        libraries.insert(sprite.collection.clone(), lib);
                    }
                    Err(e) => {
                        warn!("helicopter sprite collection {}: {e}", sprite.collection);
                        continue;
                    }
                }
            }
            let lib = &libraries[&sprite.collection];
            let Some(def) = lib.collection.sprites.get(sprite.sprite_id) else { continue };
            let Some(path) = lib.texture_of(def) else { continue };
            let texture = textures.id(path, &mut load);
            let shadow = level.0.sprites.iter().find(|s| s.shadow_of.is_some_and(|(c, _)| c == index)).and_then(|s| {
                let plane = s.shadow_of.map_or(0.0, |(_, p)| p);
                // `SpriteShadowPositioner.Awake`: "<name> shadow" from the caster's collection
                // unless the serialized sprite is already in it.
                let def = if s.collection == sprite.collection { lib.collection.sprites.get(s.sprite_id) } else { lib.collection.sprite(&format!("{} shadow", def.name)).or(Some(def)) }?;
                let path = lib.texture_of(def)?;
                Some((def.clone(), textures.id(path, &mut load), plane))
            });
            // `HelicopterColorSetter` / `HelicopterGunnerColor`: the fuselage and the guns take
            // the body colour (enemies); `HelicopterColor` paints an uncontrolled fuselage white.
            let mut color = sprite.color;
            let gun = spec.guns.iter().position(|g| {
                let mut current = Some(node);
                while let Some(c) = current {
                    if c == g.node {
                        return true;
                    }
                    current = spec.nodes[c].parent;
                }
                false
            });
            let in_gun = gun.is_some();
            if in_gun || Some(node) == spec.fuselage {
                color = body_color.unwrap_or([1.0; 4]);
            }
            sprites.push(HeliSprite { node, def: def.clone(), tiled: sprite.tiled, scale: sprite.scale, color, texture, shadow, gun });
        }
        let guns = spec
            .guns
            .iter()
            .map(|g| Gun {
                launcher: Launcher::new(modified(g.firing_rate_time, &g.firing_rate_scaling), g.fully_automatic, false),
                angle: 0.0,
                active: in_hierarchy(g.node),
                seat: (g.control_slot && spec.enemy.is_none()).then(|| GunnerSeat { slot: ControlSlot::new(g.control_radius), exit_latch: ExitLatch::default(), exit: false, fire: false, aim: None, team: 0 }),
            })
            .collect();
        let hull: Vec<Vec<SimVec2>> = spec
            .collider
            .iter()
            .flat_map(|path| merge_convex(triangulate(&path.iter().map(|p| SimVec2::new(p[0], p[1])).collect::<Vec<_>>())))
            .collect();
        // Unity decomposes the `PolygonCollider2D` into convex Box2D polygons in body space
        // (the root's scale applied).
        let root_scale = spec.world.lossy_scale();
        let pieces: Vec<Polygon> = hull.iter().map(|piece| Polygon::ccw(piece.iter().map(|p| SimVec2::new(p.x * root_scale[0], p.y * root_scale[1])).collect())).collect();
        let starting_health = spec.enemy.as_ref().and_then(|e| e.settings.health_scaling.as_ref()).or(spec.health_scaling.as_ref()).map_or(spec.starting_health, |s| {
            if session.mode == GameMode::Campaign {
                difficulty_modified(spec.starting_health, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty)
            } else {
                spec.starting_health
            }
        });
        let mut health = Health::new(starting_health);
        health.reset();
        let rotation_z = {
            let m = &spec.world.m;
            m[1][0].atan2(m[0][0]).to_degrees()
        };
        let mut body = RigidBody::new(SimVec2::new(spec.world.t[0], spec.world.t[1]), rotation_z, spec.body.mass, spec.body.linear_drag, spec.body.angular_drag, spec.body.gravity_scale);
        // `Rigidbody2D.mass` is set explicitly: Box2D's mass data from the hull, scaled to it
        // (centre of mass and rotational inertia). `RigidbodyConstraints2D.FreezeRotation` (4)
        // and kinematic bodies take no rotation (or motion) from contacts.
        let (local_center, inertia) = mass_properties(&pieces, spec.body.mass);
        body.local_center = local_center;
        body.inertia = inertia;
        body.inv_inertia = if inertia > 0.0 && spec.body.constraints & 4 == 0 && !spec.body.kinematic { 1.0 / inertia } else { 0.0 };
        if spec.body.kinematic {
            body.inv_mass = 0.0;
        }
        let enemy = spec.enemy.is_some();
        // `FixWorldOffsetFromTransform.Awake`: the offset is taken in world space once.
        let camera_offset = spec.camera_influencer.map(|i| {
            let p = spec.world_of(i, &spec.world, &|_| None).t;
            SimVec2::new(p[0] - spec.world.t[0], p[1] - spec.world.t[1])
        });
        if enemy && spec.control_slot {
            warn!("{}: an enemy helicopter with a usable ControlSlot; players could board it", spec.name);
        }
        let start = (body.position, body.angle);
        let mut helicopter = Helicopter {
            body,
            control: HelicopterControl::default(),
            input: (SimVec2::ZERO, false, None),
            processor: (SimVec2::ZERO, false, None),
            controlled: enemy,
            slot: spec.control_slot.then(|| ControlSlot::new(spec.control_radius)),
            exit_latch: ExitLatch::default(),
            exit: false,
            pilot_team: 0,
            pilot_is_gunner: false,
            camera_point: camera_offset.map_or(SimVec2::ZERO, |o| SimVec2::new(spec.world.t[0] + o.x, spec.world.t[1] + o.y)),
            camera_offset,
            engine_volume: UNOCCUPIED_ENGINE_VOLUME,
            guns,
            rotor_angles: vec![0.0; spec.rotors.len()],
            health,
            fuselage_color: body_color.unwrap_or([1.0; 4]),
            flash: 0.0,
            targeter: spec.enemy.as_ref().map(|e| Targeter::of_enemy(&e.settings, |name| warn!("helicopter targeter qualifier {name} is not implemented"))),
            sleeper: spec.enemy.as_ref().and_then(|e| e.settings.deactivate_off_screen).map(|r| {
                let mut s = OffScreenSleep::new(r);
                s.on_loaded_level_everywhere();
                s
            }),
            asleep: None,
            hull,
            pieces,
            shape: BodyShape::default(),
            // `SetEnemyLayer` puts an enemy helicopter's whole tree on layer 9.
            layer: if enemy { (8 + ENEMY_TEAM) as u8 } else { spec.layer },
            sprites,
            node_active,
            root_z: spec.world.t[2],
            root_scale: spec.world.lossy_scale(),
            explode_pending: false,
            explosion: None,
            start,
            spec,
        };
        helicopter.rebuild_shape();
        commands.spawn((Name::new(helicopter.spec.name.clone()), helicopter));
    }
    info!("{} helicopters", level.0.helicopters.len());
}

/// `HelicopterInput.OwnerOnlyUpdate`: copy the processor (enemies' `EnemyHelicopterInput`).
pub(crate) fn copy_helicopter_input(mut helicopters: Query<&mut Helicopter>) {
    for mut h in &mut helicopters {
        if h.asleep.is_none() && h.spec.enemy.is_some() {
            h.input = (h.processor.0, h.processor.1, h.processor.2.map(HeliAim::Absolute));
        }
    }
}

/// Ids of helicopter hulls in the collision world's moving colliders (clear of the level's
/// collider indices and of the letters' ids).
const HULL_COLLIDER_BASE: u32 = 1 << 25;

/// `HelicopterControl.FixedUpdate` and the physics step, for every fixed step of the frame: all
/// active helicopters step together (Box2D's island), colliding with the level's solid
/// colliders and with each other on the layers the collision matrix pairs (`crate::contact`).
fn fly_helicopters(clock: Res<FrameClock>, collision: Res<Collision>, mut helicopters: Query<(Entity, &mut Helicopter)>, mut warm: Local<WarmStart>) {
    let rules = &collision.rules;
    for &dt in &clock.steps {
        let mut active: Vec<(Entity, Mut<Helicopter>)> = helicopters.iter_mut().filter(|(_, h)| h.asleep.is_none() && h.alive()).collect();
        if active.is_empty() {
            return;
        }
        for (_, h) in active.iter_mut() {
            let input = if h.controlled { Some(h.input.0) } else { None };
            let h = &mut **h;
            // `FixWorldOffsetFromTransform.FixedUpdate` runs before the physics step.
            if let Some(offset) = h.camera_offset {
                h.camera_point = h.body.position + offset;
            }
            h.control.fixed_update(&mut h.body, input, dt);
        }
        let mut bodies: Vec<DynamicBody> = active.iter().map(|(e, h)| DynamicBody { key: e.to_bits(), body: h.body, pieces: h.pieces.clone(), layer: h.layer }).collect();
        sticks_sim::contact::step(&mut bodies, &collision.world, rules, dt, rules.velocity_iterations, rules.position_iterations, &mut warm);
        for ((_, h), b) in active.iter_mut().zip(bodies) {
            h.body = b.body;
        }
    }
    for (_, mut h) in &mut helicopters {
        h.rebuild_shape();
    }
}

/// The helicopters' hulls as obstacles for the characters' bodies (Box2D collides them with
/// the characters' rigid bodies; the characters here use their own controller physics).
/// Published at the start of the frame, so characters meet each hull where the previous frame
/// left it.
/// TODO(UNCERTAIN): the characters push nothing back (their mass against the helicopter's is
/// ignored), and a hull is not interleaved step by step with the characters' fixed steps.
fn publish_hulls(mut collision: ResMut<Collision>, helicopters: Query<&Helicopter>, cars: Query<&crate::cars::Car>, whales: Option<Res<crate::scenery::Whales>>) {
    let mut items: Vec<_> = helicopters
        .iter()
        .filter(|h| h.is_active_object())
        .enumerate()
        .map(|(i, h)| {
            let collider = StaticCollider { id: HULL_COLLIDER_BASE + i as u32, layer: h.layer, ..Default::default() };
            let pieces = h.shape.polygons.iter().map(|piece| piece.iter().map(|p| *p + h.body.position).collect()).collect();
            (collider, pieces)
        })
        .collect();
    // The cars' chassis and wheels too (`crate::cars`).
    items.extend(crate::cars::car_obstacles(&cars));
    // And Pirates' whales (`crate::scenery`).
    if let Some(whales) = whales {
        items.extend(whales.obstacles());
    }
    collision.world.set_moving(items);
}

/// `Health.Damage` from projectiles; `HelicopterColor` flashes the fuselage; an exhausted hull
/// explodes (`explode_helicopters`).
fn hit_helicopters(
    mut hits: MessageReader<HelicopterHit>,
    mut helicopters: Query<(&mut Helicopter, Option<&crate::net::NetObject>)>,
    mut net: ResMut<crate::net::Net>,
    mut scores: Query<(&crate::player::Player, &mut crate::life::PlayerScore)>,
) {
    // In a network game only the host's hits count; the others take the host's health.
    if net.is_client() {
        hits.clear();
        let updates = std::mem::take(&mut net.helicopter_health);
        for (id, remaining) in updates {
            for (mut h, object) in &mut helicopters {
                if object.is_some_and(|o| o.id == id) {
                    h.receive_health(remaining);
                }
            }
        }
        return;
    }
    for hit in hits.read() {
        let Ok((mut h, _)) = helicopters.get_mut(hit.helicopter) else { continue };
        if !h.alive() {
            continue;
        }
        let before = h.health.remaining();
        h.health.damage(hit.amount);
        // `DamageOtherOnCollision`: the shooter gets the health taken; destroying a flown
        // helicopter frags its pilot.
        let victim = h.pilot().and_then(|p| scores.get(p).ok()).map(|(p, _)| p.index);
        if let Some((_, mut score)) = hit.responsible.and_then(|r| scores.get_mut(r).ok()) {
            score.0.damage_dealt += before - h.health.remaining();
            if let (false, Some(victim)) = (h.alive(), victim) {
                score.0.fragged(victim);
            }
        }
        if h.alive() {
            h.flash = 0.1;
        } else {
            info!("{} destroyed", h.spec.name);
            h.explode_pending = true;
        }
    }
}

/// `Explodable.Explode` of an exhausted helicopter (`PlaySoundOnExplode`,
/// `ExplodeSpritesAndLines` from what was drawn), `Explosion.Update`, and `ExplosionFinished`:
/// a player helicopter respawns at its start (`Respawner`, `RespawnAtStartPosition`), an enemy
/// one is destroyed.
#[allow(clippy::too_many_arguments)]
fn explode_helicopters(
    mut commands: Commands,
    art: Option<Res<ManArt>>,
    light: Res<SceneLight>,
    clock: Res<FrameClock>,
    mut rng: ResMut<crate::life::DeathRng>,
    mut sounds: MessageWriter<PlaySound>,
    mut helicopters: Query<(Entity, &mut Helicopter)>,
) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut h) in &mut helicopters {
        if h.explode_pending {
            h.explode_pending = false;
            let root = h.root_affine();
            let at = SimVec3::new(root.t[0], root.t[1], root.t[2]);
            if let Some(sound) = &h.spec.explode_sound {
                sounds.write(PlaySound { group: sound.clone(), at: Some(at), owner: None });
            }
            match (h.spec.explosion_fade_wait, art.as_ref()) {
                (Some(fade_wait), Some(art)) => {
                    let mut parts_art = sticks_sim::visual::VisualArt::empty(art.art.line_uvs());
                    let mut parts = Vec::new();
                    for s in &h.sprites {
                        let mut active = true;
                        let mut current = Some(s.node);
                        while let Some(c) = current {
                            active &= h.node_active[c];
                            current = h.spec.nodes[c].parent;
                        }
                        if !active || s.tiled.is_some() {
                            continue;
                        }
                        parts_art.add_def(&s.def, s.shadow.as_ref().map(|(d, _, _)| d), s.texture);
                        let world = h.node_world(s.node, &root);
                        let position = SimVec3::new(world.t[0], world.t[1], world.t[2]);
                        let plane = s.shadow.as_ref().map_or(0.0, |(_, _, p)| *p);
                        let (shadow_offset, shadow_scale) = match light.for_shadows {
                            Some(l) => shadow_offset_and_scale(l, position, plane),
                            None => ([0.0, 0.0], 1.0),
                        };
                        let q = h.node_rotation(s.node);
                        let lossy = world.lossy_scale();
                        let color = if Some(s.node) == h.spec.fuselage { h.fuselage_color } else { s.color };
                        parts.push(sticks_sim::visual::FlatSpriteInstance {
                            bone: 0,
                            sprite_name: s.def.name.clone(),
                            position,
                            flipped: false,
                            roll_degrees: 0.0,
                            tilt: [q.x, q.y, q.z, q.w],
                            scale: [s.scale[0] * lossy[0], s.scale[1] * lossy[1], s.scale[2] * lossy[2]],
                            color,
                            shadow_plane_z: plane,
                            shadow_offset,
                            shadow_scale,
                        });
                    }
                    let visual = sticks_sim::visual::CharacterVisual { shadows: Vec::new(), lines: Vec::new(), sprites: parts };
                    let explosion = sticks_sim::explosion::Explosion::go(&visual, at, at, fade_wait, &mut rng.0, clock.last_step_delta);
                    h.explosion = Some((explosion, parts_art));
                }
                // No explosion object: `Explodable.Finished` at once.
                _ => h.explosion = None,
            }
            if h.explosion.is_none() {
                finish_explosion(&mut commands, entity, &mut *h);
            }
            continue;
        }
        let finished = h.explosion.as_mut().is_some_and(|(e, _)| e.update(delta));
        if finished {
            h.explosion = None;
            finish_explosion(&mut commands, entity, &mut *h);
        }
    }
}

/// `Explodable.ExplosionFinished`.
fn finish_explosion(commands: &mut Commands, entity: Entity, h: &mut Helicopter) {
    if h.spec.respawns && h.spec.enemy.is_none() {
        // `Respawner`: `health.Reset()`, `SetActive(true)`, `OnRespawned` → `RespawnAtStartPosition`.
        let _ = h.health.reset();
        h.body.position = h.start.0;
        h.body.angle = h.start.1;
        h.body.linear_velocity = SimVec2::ZERO;
        h.body.angular_velocity = 0.0;
        h.flash = 0.0;
        h.rebuild_shape();
        info!("{} respawned", h.spec.name);
    } else {
        commands.entity(entity).despawn();
    }
}

/// `DeactivateOffScreen` / `OffScreenReactivator` for enemy helicopters.
fn sleep_helicopters(windows: Query<&Window, With<PrimaryWindow>>, rigs: Query<&Rig, With<LevelCamera>>, mut helicopters: Query<&mut Helicopter>) {
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    let Ok(rig) = rigs.single() else { return };
    let view = rig.view_bounds(aspect);
    for mut h in &mut helicopters {
        if let Some(position) = h.asleep {
            let radius = h.sleeper.map_or(0.0, |s| s.radius);
            if view.is_on_screen(position, radius) {
                h.asleep = None;
            }
            continue;
        }
        if !h.alive() {
            continue;
        }
        let Some(enemy) = h.spec.enemy.as_ref().map(|e| e.node) else { continue };
        let root = h.root_affine();
        let p = h.node_world(enemy, &root).t;
        let position = SimVec2::new(p[0], p[1]);
        if let Some(mut sleeper) = h.sleeper {
            if sleeper.update(position, &view) {
                h.asleep = Some(position);
            }
            h.sleeper = Some(sleeper);
        }
    }
}

/// The enemy controller's behaviours: `Targeter.Targets`, `HelicopterMoveToFirstTargetHeight`,
/// `HelicopterFireAtFirstTarget` write the processor; `HelicopterGunnerControl.Update` turns the
/// guns towards the aim.
pub(crate) fn think_helicopters(collision: Res<Collision>, players: Query<(&Avatar, &Mortal), With<ControlledBy>>, mut helicopters: Query<&mut Helicopter>) {
    // The scene's helicopter "Target"s registered before the players' avatars were spawned.
    // TODO(UNCERTAIN): `Targetable` registration order (only matters for equal distances).
    let mut candidates: Vec<Candidate> = helicopters.iter().filter_map(Helicopter::target_candidate).collect();
    candidates.extend(players
        .iter()
        .filter(|(_, mortal)| mortal.avatar_active())
        .map(|(avatar, _)| {
            let p = avatar.position();
            Candidate { position: SimVec3::new(p.x, p.y + CAMERA_TARGET_HEIGHT, sticks_sim::character::DEFAULT_Z), controlled: true }
        }));
    let hit_triggers = collision.rules.raycasts_hit_triggers;
    let obstructed = |from: SimVec2, to: SimVec2, layers: u32| collision.world.linecast(from, to, layers, hit_triggers);
    for mut h in &mut helicopters {
        if h.asleep.is_some() || !h.alive() {
            continue;
        }
        let root = h.root_affine();
        if let (Some(enemy), Some(targeter)) = (h.spec.enemy.clone(), h.targeter.clone()) {
            let p = h.node_world(enemy.node, &root).t;
            let origin = SimVec3::new(p[0], p[1], p[2]);
            let targets = targeter.targets(origin, &candidates, obstructed);
            let first = targets.first();
            if enemy.move_to_first_target_height {
                h.processor.0 = move_to_first_target_height(h.processor.0, SimVec2::new(p[0], p[1]), first.map(|t| t.position.y), h.body.linear_velocity);
            }
            if enemy.fire_at_first_target {
                match first {
                    Some(t) => {
                        h.processor.2 = Some(SimVec2::new(t.position.x, t.position.y));
                        h.processor.1 = true;
                    }
                    None => h.processor.1 = false,
                }
            }
        }
        // `HelicopterGunnerControl.Update`: a gun with an input faces `RelativeAim(gun position)`
        // (its gunner's aim, or the pilot's).
        for g in 0..h.guns.len() {
            if !h.guns[g].active || !h.gun_has_input(g) {
                continue;
            }
            if h.gunner(g).is_none() && !h.controlled {
                continue;
            }
            let Some(aim) = h.gun_input(g).1 else { continue };
            let node = h.spec.guns[g].node;
            let p = h.node_world(node, &root).t;
            h.guns[g].angle = gun_angle_degrees(aim.relative_to(p));
        }
    }
}

/// `HelicopterInput`'s aim: the keyboard and mouse give a point (`AbsoluteAim`), a gamepad a
/// direction (`RelativeAim`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum HeliAim {
    Absolute(SimVec2),
    Relative(SimVec2),
}

impl HeliAim {
    /// `CharacterInput.RelativeAim(relativeAim, absoluteAim, origin)`.
    pub(crate) fn relative_to(self, origin: [f32; 3]) -> SimVec2 {
        match self {
            HeliAim::Absolute(p) => SimVec2::new(p.x - origin[0], p.y - origin[1]),
            HeliAim::Relative(d) => d,
        }
    }
}

/// `ProjectileLauncher.Update` for each gun: `FireDirection = Input.RelativeAim(pivot.position)`
/// (the gun), the shot leaves from the launcher's own position.
fn fire_helicopter_guns(
    mut commands: Commands,
    clock: Res<FrameClock>,
    session: Res<Session>,
    collision: Res<Collision>,
    library: Option<Res<ProjectileLibrary>>,
    mut helicopters: Query<&mut Helicopter>,
    mut sounds: MessageWriter<PlaySound>,
) {
    let Some(library) = library else { return };
    let delta = clock.stepper.frame().delta;
    for mut h in &mut helicopters {
        if h.asleep.is_some() || !h.alive() {
            continue;
        }
        let root = h.root_affine();
        for g in 0..h.guns.len() {
            // `ProjectileLauncher.Update` returns at once without an input.
            if !h.guns[g].active || !h.gun_has_input(g) {
                continue;
            }
            let gunner = h.gunner(g);
            if gunner.is_none() && !h.controlled {
                continue;
            }
            let (fire, aim) = h.gun_input(g);
            let Some(launch) = h.guns[g].launcher.update(fire, clock.time, delta) else { continue };
            let spec_gun = &h.spec.guns[g];
            let Some(spec) = library.0.get(&spec_gun.projectile) else { continue };
            let pivot = h.node_world(spec_gun.node, &root).t;
            let muzzle = h.node_world(spec_gun.launch_point, &root).t;
            let Some(aim) = aim else { continue };
            let direction = aim.relative_to(pivot);
            let damage = match &spec.damage_scaling {
                Some(s) if session.mode == GameMode::Campaign => difficulty_modified(spec.damage, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
                _ => spec.damage,
            };
            // A player's input object is the `Player`: `SetResponsiblePlayer` credits it (and
            // gives its team); an enemy's input object is not, so the shot is team 1.
            let (team, responsible) = match (h.spec.enemy.is_some(), gunner) {
                (true, _) => (ENEMY_TEAM, None),
                (false, Some(gunner)) => (h.guns[g].seat.as_ref().map_or(0, |s| s.team).into(), Some(gunner)),
                (false, None) => (h.pilot_team.into(), h.pilot()),
            };
            let shot = Shot::launch(spec, damage, SimVec2::new(muzzle[0], muzzle[1]), direction, launch.sub_frame_offset, team, collision.rules.gravity);
            commands.spawn((Name::new(spec.name.clone()), Projectile::new(shot, spec.clone(), responsible)));
            // `MasterAudio.PlaySound3DAtTransformAndForget(firingSound, launcher)`.
            if !spec_gun.firing_sound.is_empty() {
                sounds.write(PlaySound { group: spec_gun.firing_sound.clone(), at: Some(SimVec3::new(muzzle[0], muzzle[1], muzzle[2])), owner: None });
            }
        }
    }
}

/// Lines with their shadows (`LinesRenderer` at `origin`), as one renderer.
/// `LinesRenderer` lines with their shadows cast from `origin` onto each line's plane.
pub(crate) fn lines_geometry(lines: &[StickLine], origin: SimVec3, light: Option<SimVec3>, line_uvs: [[f32; 2]; 8]) -> Geometry {
    let mut shadows = Vec::new();
    for line in lines {
        let (delta, k) = light.map_or(([0.0, 0.0], 1.0), |l| shadow_offset_and_scale(l, origin, line.shadow_plane_z));
        shadows.push(StickLine {
            a: SimVec3::new(line.a.x + delta[0], line.a.y + delta[1], line.a.z + LINE_SHADOW_Z),
            b: SimVec3::new(line.b.x + delta[0], line.b.y + delta[1], line.b.z + LINE_SHADOW_Z),
            width: line.width * k,
            color: [0, 0, 0, line.color[3]],
            shadow_plane_z: line.shadow_plane_z,
        });
    }
    let mut g = Geometry::default();
    for line in shadows.iter().chain(lines) {
        let Some(v) = line_vertices(line) else { continue };
        let base = g.positions.len() as u32;
        g.positions.extend(v.iter().map(|p| [p.x, p.y, p.z]));
        g.normals.extend([BACK; 8]);
        g.uvs.extend(line_uvs);
        g.colors.extend([color32(line.color.map(|c| f32::from(c) / 255.0)); 8]);
        g.indices.extend(LINE_TRIANGLES.iter().map(|i| base + i));
    }
    g
}

/// The helicopters' sprites (moved with the body, guns turned), their shadows and the rotors.
#[allow(clippy::too_many_arguments)]
fn queue_helicopters(
    art: Option<Res<ManArt>>,
    light: Res<SceneLight>,
    clock: Res<FrameClock>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LevelCamera>>,
    animation: Option<Res<crate::level::CharacterAnimation>>,
    mut helicopters: Query<(Entity, &mut Helicopter)>,
    pilots: Query<(&crate::vehicles::Piloting, &ControlledBy, Option<&crate::figure::Palette>), With<Disabled>>,
    mut queue: ResMut<FrameQueue>,
) {
    let Some(art) = art else { return };
    let Ok(rig) = rigs.single() else { return };
    let view = camera_view(rig, &windows);
    let camera = view.camera;
    let light = light.for_shadows;
    let delta = clock.stepper.frame().delta;
    for (entity, mut h) in &mut helicopters {
        if let Some((explosion, parts_art)) = &h.explosion {
            for (texture, g) in explosion.renderers_textured(parts_art, light) {
                if !g.is_empty() {
                    queue.0.push_frame(texture.unwrap_or(art.texture), g);
                }
            }
        }
        if h.asleep.is_some() || !h.alive() {
            continue;
        }
        // `HelicopterColor`: the fuselage flashes `+ (0.2, 0.2, 0.2)` for 0.1 s after a hit.
        h.flash = (h.flash - delta).max(0.0);
        let root = h.root_affine();
        let mut renderers: Vec<(u32, Geometry)> = Vec::new();
        for s in &h.sprites {
            if !(0..h.spec.nodes.len()).contains(&s.node) {
                continue;
            }
            let mut active = true;
            let mut current = Some(s.node);
            while let Some(c) = current {
                active &= h.node_active[c];
                current = h.spec.nodes[c].parent;
            }
            if !active {
                continue;
            }
            let world = h.node_world(s.node, &root);
            let mut color = s.color;
            // `HelicopterGunnerColor.SetController` / `ClearController`: a gunner seat's rocket
            // launcher in its gunner's body colour, white while the seat is free.
            if let Some(g) = s.gun.filter(|&g| h.guns[g].seat.is_some()) {
                color = match h.gunner(g) {
                    Some(gunner) => {
                        let body = pilots.iter().find(|(p, c, _)| p.vehicle == entity && c.0 == gunner).and_then(|(_, _, p)| p.map(|p| p.0.body)).unwrap_or_else(|| art.body_color());
                        [f32::from(body[0]) / 255.0, f32::from(body[1]) / 255.0, f32::from(body[2]) / 255.0, f32::from(body[3]) / 255.0]
                    }
                    None => [1.0; 4],
                };
            }
            if Some(s.node) == h.spec.fuselage {
                color = h.fuselage_color;
                if h.flash > 0.0 {
                    color = [color[0] + 0.2, color[1] + 0.2, color[2] + 0.2, color[3] + 1.0];
                }
            }
            renderers.push((s.texture, scene_sprite_geometry(&s.def, s.tiled.as_ref(), s.scale, color, &world)));
            if let (Some((def, texture, plane)), Some(light)) = (&s.shadow, light) {
                let rotation = h.node_rotation(s.node);
                renderers.push((*texture, sprite_shadow_geometry(def, s.tiled.as_ref(), &world, rotation, s.scale, color[3], *plane, light)));
            }
        }
        // Rotors (`LinesRenderer`s at the rotor's position, world axes).
        for (r, rotor) in h.spec.rotors.clone().iter().enumerate() {
            let hub_affine = h.node_world(rotor.node, &root);
            let hub = SimVec3::new(hub_affine.t[0], hub_affine.t[1], hub_affine.t[2]);
            let mut angle = h.rotor_angles[r];
            let lines: Vec<StickLine> = if rotor.top {
                // `camera.WorldToViewportPoint` then back at the hub's depth: a perspective
                // camera looking along +z scales the offset from the camera by the depth ratio.
                let reproject = |p: SimVec3| {
                    let k = (hub.z - camera.z) / (p.z - camera.z);
                    SimVec3::new(camera.x + (p.x - camera.x) * k, camera.y + (p.y - camera.y) * k, hub.z)
                };
                let to_world = |p: SimVec3| {
                    let w = hub_affine.transform_point([p.x, p.y, p.z]);
                    SimVec3::new(w[0], w[1], w[2])
                };
                top_rotor_blades(&mut angle, rotor.angular_velocity, rotor.radius, delta, &to_world, hub, &reproject)
                    .into_iter()
                    .map(|(d, foreground)| {
                        let z = hub.z + if foreground { -0.1 } else { 0.0 };
                        StickLine { a: SimVec3::new(hub.x, hub.y, z), b: SimVec3::new(hub.x + d.x, hub.y + d.y, z), width: rotor.line_thickness, color: rotor.color, shadow_plane_z: rotor.shadow_plane_z }
                    })
                    .collect()
            } else {
                back_rotor_blades(&mut angle, rotor.angular_velocity, rotor.radius, delta)
                    .into_iter()
                    .map(|b| StickLine { a: hub + b.a, b: hub + b.b, width: rotor.line_thickness, color: rotor.color, shadow_plane_z: 0.0 })
                    .collect()
            };
            h.rotor_angles[r] = angle;
            renderers.push((art.texture, lines_geometry(&lines, hub, light, art.art.line_uvs())));
        }
        // `VehicleOccupant`s: the pilot while a player flies (`HelicopterControl.SetController`,
        // in the player's colours); an enemy's pilot and any passenger as the scene enables them.
        // TODO(UNCERTAIN): occupants not coloured by a player use the `Man` prefab's colours
        // (the occupant prefab's helmet is the same red).
        if let Some(animation) = animation.as_ref() {
            let piloted = h.pilot().is_some();
            // The gunner seats' occupants show their gunners (`HelicopterGunnerControl`).
            let gunner_of = |node: usize| h.spec.guns.iter().position(|g| g.occupant == Some(node)).map(|g| h.gunner(g));
            let seats: Vec<(&unity_data::vehicles::OccupantSeat, bool, Option<Entity>)> = h
                .spec
                .pilot
                .iter()
                .map(|seat| (seat, piloted || (h.spec.enemy.is_some() && seat.enabled), if piloted { h.pilot() } else { None }))
                .chain(h.spec.passengers.iter().map(|seat| match gunner_of(seat.node) {
                    Some(gunner) => (seat, gunner.is_some(), gunner),
                    None => (seat, seat.enabled, None),
                }))
                .collect();
            for (seat, shown, controller) in seats {
                if !shown || !h.active_in_hierarchy(seat.node) {
                    continue;
                }
                let palette = controller
                    .and_then(|player| pilots.iter().find(|(p, c, _)| p.vehicle == entity && c.0 == player).and_then(|(_, _, p)| p.map(|p| p.0)))
                    .unwrap_or_else(|| art.prefab_palette());
                let world = h.node_world(seat.node, &root);
                let at = SimVec3::new(world.t[0], world.t[1], world.t[2]);
                renderers.extend(art.occupant_renderers(&animation.0, &seat.animation_state, at, h.node_rotation(seat.node), palette, light, None));
            }
        }
        for (texture, g) in renderers {
            if !g.is_empty() && view.may_see(g.bounds_center(), crate::unity_render::extent(&g)) {
                queue.0.push_frame(texture, g);
            }
        }
    }
}
