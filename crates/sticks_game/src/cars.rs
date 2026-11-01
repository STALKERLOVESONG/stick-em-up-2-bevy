//! Cars (the moon rovers of Moon, Moon Race and Corruption 2): `CarControl` roots with two
//! wheels on `WheelJoint2D`s, stepped by `sticks_sim::contact::step_with_joints` and driven by
//! `sticks_sim::car::CarDrive`.
//!
//! * Players board and leave them like helicopters ([`crate::vehicles`]): the driver's
//!   `KeyboardAndMouseCarInput` (W/S speed, A/D clockwise rotation, E leaves) or a gamepad's
//!   left stick (the helicopter processors' movement).
//! * The chassis sprites move with the body; the wheel sprites (`CopyLocalTransform`) with
//!   their wheels. While driven the "Camera Influencer" (a fixed world offset from the body,
//!   `FixWorldOffsetFromTransform`) frames the camera and `Rigidbody2DPathCameraInfluencer` adds
//!   where the car's ballistic path would hit the ground as a distance-only influence.
//! * Projectiles hit the chassis (`Health`, 5 in every level); an exhausted car ejects its
//!   driver, plays its sound and its sprites fly apart (no respawner: it is gone).
//!
//! TODO(UNCERTAIN): cars do not collide with each other or with helicopters; the wheels'
//! `PhysicsMaterial2D` is not read (default friction 0.4); a `CopyLocalTransform` sprite is
//! matched to the wheel it started on.
//! The lunar module (Moon) is a car without wheels driven by `RocketControl` (thrust along its up
//! axis from the engine, A/D steer its spin), with three `PassengerControl` seats other players
//! can take once someone pilots it.
//!
//! TODO: the lunar module's exhaust particles, engine sound and `GroundRaycastCameraInfluencer`;
//! `Dock` at Moon's end (the module pulled onto the command module); the gunner seat (`CarGunnerControl`), the "rover 720/1440" goals
//! ([`sticks_sim::car::FlipGoal`] is computed, not recorded), the engine sound, enemies targeting
//! cars.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::car::CarDrive;
use sticks_sim::contact::{DynamicBody, Polygon, Touch, WarmStart, WheelJoint, mass_properties, step_with_joints};
use sticks_sim::draw::{Geometry, scene_sprite_geometry, shadow_offset_and_scale, sprite_shadow_geometry};
use sticks_sim::life::Health;
use sticks_sim::physics::{BodyShape, StaticCollider, merge_convex, triangulate};
use sticks_sim::rigid_body::RigidBody;
use sticks_sim::vehicles::{ControlSlot, ExitLatch, team_vehicles_layer};
use sticks_sim::weapons::{Launcher, Shot, corrected_aim_with, difficulty_modified};
use unity_compat::{Quat as SimQuat, Vec2 as SimVec2, Vec3 as SimVec3};
use unity_data::math::Affine;
use unity_data::sprites::{SpriteDef, SpriteLibrary, TiledLayout};
use unity_data::vehicles::CarSpawn;

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::effects::PlaySound;
use crate::figure::{ManArt, Palette};
use crate::level::{ActiveLevel, CharacterAnimation, Collision};
use crate::helicopter::HeliAim;
use crate::vehicles::Piloting;
use crate::player::{ControlledBy, FrameClock};
use crate::state::{AppState, GameMode, Session};
use crate::weapon::{Projectile, ProjectileLibrary};
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, TextureLoad, UnityLitMaterial, UnityTextures, camera_view};

/// Collider ids of car hulls published as moving obstacles (after the helicopters').
pub const CAR_COLLIDER_BASE: u32 = 0x7F00_0000;

struct CarSprite {
    node: usize,
    /// The wheel this sprite follows (`CopyLocalTransform`), if any.
    wheel: Option<usize>,
    def: SpriteDef,
    tiled: Option<TiledLayout>,
    scale: [f32; 3],
    color: [f32; 4],
    texture: u32,
    shadow: Option<(SpriteDef, u32, f32)>,
}

/// A projectile struck a car's chassis.
#[derive(Message, Clone, Copy, Debug)]
pub struct CarHit {
    pub car: Entity,
    pub amount: f32,
}

#[derive(Component)]
pub struct Car {
    pub(crate) spec: Arc<CarSpawn>,
    /// The chassis (0) and the wheels.
    bodies: Vec<DynamicBody>,
    joints: Vec<WheelJoint>,
    warm: WarmStart,
    drive: CarDrive,
    /// `RocketControl` (the lunar module) instead of `CarControl`, with the engine's position in
    /// the body's frame.
    rocket: Option<(sticks_sim::car::RocketDrive, SimVec2)>,
    /// `PassengerControl` seats (the lunar module's), each its own `ControlSlot`.
    pub(crate) passengers: Vec<Passenger>,
    /// `CarGunnerControl` seats (the rover's gunner), index-matched with `spec.gunners`.
    pub(crate) gunners: Vec<CarGunnerSeat>,
    pub(crate) slot: Option<ControlSlot<Entity>>,
    pub(crate) exit_latch: ExitLatch,
    /// `CarInput.Exit` of the driver this frame.
    pub(crate) exit: bool,
    /// `CarInput.DesiredSpeed` / `ClockwiseRotation`.
    pub(crate) input: (f32, f32),
    health: Health,
    sprites: Vec<CarSprite>,
    node_active: Vec<bool>,
    root_z: f32,
    root_scale: [f32; 3],
    camera_offset: Option<SimVec2>,
    /// `Rigidbody2DPathCameraInfluencer`: where the path meets the ground (while driven).
    path_point: Option<SimVec2>,
    wheels_grounded: Vec<bool>,
    body_touching: bool,
    /// `layerWhenUnoccupied` and the team layer waiting for no other car to be near.
    layer_unoccupied: u8,
    layer_to_set: Option<u8>,
    pub layer: u8,
    /// The chassis as projectiles see it (rotated, relative to the body position).
    pub shape: BodyShape,
    hull: Vec<Vec<SimVec2>>,
    explode_pending: bool,
    explosion: Option<(sticks_sim::explosion::Explosion, sticks_sim::visual::VisualArt)>,
    /// `RocketControl.engineParticles` (the lunar module's "Emissions").
    exhaust: Option<Exhaust>,
    /// `engineSound.volume`: the scene's, 1 from `SetController`, 0 from `ClearController`
    /// (`RocketControl` sets the throttle every frame instead).
    engine_volume: f32,
}

/// A rocket's engine particles: `emissionRate = desiredSpeedControl × engineParticleRate`.
struct Exhaust {
    node: usize,
    spec: unity_data::particles::ParticleEffectSpec,
    system: sticks_sim::particles::ParticleSystem,
}

/// A passenger seat: who sits in it and their exit input.
pub(crate) struct Passenger {
    pub(crate) slot: ControlSlot<Entity>,
    pub(crate) exit_latch: ExitLatch,
    /// `PassengerInput.Exit` of the passenger this frame.
    pub(crate) exit: bool,
}

/// A `CarGunnerControl` seat: who sits in it, their `HelicopterGunnerInput` this frame and the
/// occupant's weapon.
pub(crate) struct CarGunnerSeat {
    pub(crate) slot: ControlSlot<Entity>,
    pub(crate) exit_latch: ExitLatch,
    pub(crate) exit: bool,
    pub(crate) fire: bool,
    pub(crate) aim: Option<HeliAim>,
    /// The gunner's team (their shots' layer).
    team: u8,
    /// `correctedAim` (starts at (1, 0)).
    corrected: SimVec2,
    /// The rig weapon `SetUpNewWeapon(weaponName)` put on the occupant and its
    /// `ProjectileLauncher`, set up on the first `Update` (as the original does lazily).
    weapon: Option<(usize, Launcher)>,
}

impl CarGunnerSeat {
    pub(crate) fn gunner(&self) -> Option<Entity> {
        self.slot.controller()
    }
}

impl Car {
    /// `CarGunnerControl.SetController`.
    pub(crate) fn take_gun(&mut self, g: usize, player: Entity, team: u8) {
        let Some(seat) = self.gunners.get_mut(g) else { return };
        seat.slot.set_controller(player);
        seat.exit_latch.on_set_controller();
        seat.team = team;
        seat.fire = false;
        seat.exit = false;
    }

    /// `CarGunnerControl.ClearController`: the launcher loses its input.
    pub(crate) fn leave_gun(&mut self, g: usize) -> Option<Entity> {
        let seat = self.gunners.get_mut(g)?;
        seat.fire = false;
        seat.exit = false;
        seat.aim = None;
        seat.slot.clear_controller()
    }

    /// Where gun seat `g`'s `ControlSlot` is (on the "Gunner" occupant).
    pub(crate) fn gunner_position(&self, g: usize) -> SimVec3 {
        let p = self.node_world(self.spec.gunners[g].seat.node, &self.root_affine()).t;
        SimVec3::new(p[0], p[1], p[2])
    }

    fn root_affine(&self) -> Affine {
        let b = &self.bodies[0].body;
        let q = SimQuat::about_z(b.angle);
        Affine::from_trs([b.position.x, b.position.y, self.root_z], (q.x, q.y, q.z, q.w), self.root_scale)
    }

    fn node_world(&self, i: usize, root: &Affine) -> Affine {
        let n = &self.spec.nodes[i];
        let Some(parent) = n.parent else { return *root };
        let p = self.node_world(parent, root);
        let r = n.local_rotation;
        p.mul(&Affine::from_trs(n.local_position, (r[0], r[1], r[2], r[3]), n.local_scale))
    }

    fn active_in_hierarchy(&self, node: usize) -> bool {
        let mut current = Some(node);
        while let Some(c) = current {
            if !self.node_active[c] {
                return false;
            }
            current = self.spec.nodes[c].parent;
        }
        true
    }

    pub(crate) fn alive(&self) -> bool {
        !self.health.is_exhausted()
    }

    /// Projectiles can hit it.
    pub fn is_target(&self) -> bool {
        self.alive() && self.explosion.is_none()
    }

    pub fn position(&self) -> SimVec2 {
        self.bodies[0].body.position
    }

    pub fn driver(&self) -> Option<Entity> {
        self.slot.and_then(|s| s.controller())
    }

    pub fn slot_position(&self) -> SimVec3 {
        let p = self.position();
        SimVec3::new(p.x, p.y, self.root_z)
    }

    /// Where its `Targetable` ("Target") is, while projectiles can hit it.
    pub fn target_position(&self) -> Option<SimVec2> {
        let node = self.spec.target?;
        if !self.is_target() {
            return None;
        }
        let p = self.node_world(node, &self.root_affine()).t;
        Some(SimVec2::new(p[0], p[1]))
    }

    pub fn disembarkment_point(&self) -> Option<SimVec3> {
        let node = self.spec.disembarkment_point?;
        let p = self.node_world(node, &self.root_affine()).t;
        Some(SimVec3::new(p[0], p[1], p[2]))
    }

    /// The engine `AudioSource` while the car is in one piece: its settings, position, volume and
    /// pitch (`CarControl.FixedUpdate`: `0.48 + 0.17 |desiredSpeed|`; the rocket keeps its pitch).
    pub fn engine(&self) -> Option<(&unity_data::vehicles::LoopingSound, SimVec3, f32, f32)> {
        let sound = self.spec.engine_sound.as_ref()?;
        if !self.is_target() {
            return None;
        }
        let (volume, pitch) = match &self.rocket {
            Some((rocket, _)) => (rocket.engine_volume(), sound.pitch),
            None => (self.engine_volume, self.drive.engine_pitch()),
        };
        Some((sound, self.slot_position(), volume, pitch))
    }

    /// `CarControl.SetController`.
    pub(crate) fn take_control(&mut self, player: Entity, team: u8) {
        let Some(slot) = self.slot.as_mut() else { return };
        slot.set_controller(player);
        self.engine_volume = 1.0;
        self.exit_latch.on_set_controller();
        self.layer = self.layer_unoccupied;
        self.layer_to_set = Some(team_vehicles_layer(team));
    }

    /// `ControlSlot.RelinquishControl` → `CarControl.ClearController`.
    pub(crate) fn relinquish(&mut self) -> Option<Entity> {
        let player = self.slot.as_mut()?.clear_controller();
        self.drive.clear();
        if let Some((rocket, _)) = &mut self.rocket {
            rocket.clear();
        }
        self.input = (0.0, 0.0);
        self.exit = false;
        self.engine_volume = 0.0;
        self.layer = self.layer_unoccupied;
        self.layer_to_set = None;
        player
    }

    /// The "Camera Influencer" while driven (`LinkBehaviourEnablementToControlSlotControlledness`).
    pub fn camera_influence(&self) -> Option<SimVec2> {
        (self.driver().is_some() && self.alive()).then(|| self.position() + self.camera_offset.unwrap_or(SimVec2::ZERO))
    }

    /// `GroundRaycastCameraInfluencer` on a rocket's root (enabled while it has a pilot,
    /// `LinkBehaviourEnablementToControlSlotControlledness`): where a ray straight down from the
    /// root first meets layer 0.
    pub fn ground_influence(&self, world: &sticks_sim::physics::World, hit_triggers: bool) -> Option<SimVec2> {
        if self.rocket.is_none() || !self.spec.ground_raycast_camera || self.driver().is_none() || !self.alive() {
            return None;
        }
        world.raycast_down_point(self.position(), 1, hit_triggers)
    }

    /// `Rigidbody2DPathCameraInfluencer` while driven.
    pub fn path_influence(&self) -> Option<SimVec2> {
        if self.driver().is_some() && self.alive() { self.path_point } else { None }
    }

    /// The chassis' health for the driver's HUD bar.
    pub fn health_normalized(&self) -> f32 {
        self.health.normalized()
    }

    fn rebuild_shape(&mut self) {
        let b = &self.bodies[0].body;
        let (s, c) = b.angle.sin_cos();
        let scale = self.root_scale;
        self.shape.polygons = self.hull.iter().map(|piece| piece.iter().map(|p| SimVec2::new(c * p.x * scale[0] - s * p.y * scale[1], s * p.x * scale[0] + c * p.y * scale[1])).collect()).collect();
    }

    /// The chassis and the wheels as one body, for the triggers the car drives through
    /// (`TriggeredByAllPlayers` counts a collider anywhere under the car's root).
    pub(crate) fn trigger_body(&self) -> sticks_sim::physics::Body {
        let chassis = &self.bodies[0].body;
        let mut shape = self.shape.clone();
        for (k, w) in self.spec.wheels.iter().enumerate() {
            let offset = self.bodies[1 + k].body.position - chassis.position;
            shape.circles.push(sticks_sim::physics::CircleShape { offset, radius: w.radius });
        }
        sticks_sim::physics::Body { position: chassis.position, velocity: chassis.linear_velocity, gravity_scale: 0.0, layer: self.layer, shape }
    }

    /// The hull pieces and the wheels as world polygons (moving obstacles for the characters).
    pub fn obstacle_pieces(&self) -> Vec<Vec<SimVec2>> {
        let p = self.position();
        let mut pieces: Vec<Vec<SimVec2>> = self.shape.polygons.iter().map(|piece| piece.iter().map(|v| *v + p).collect()).collect();
        for (k, w) in self.spec.wheels.iter().enumerate() {
            let c = self.bodies[1 + k].body.position;
            pieces.push((0..12).map(|i| {
                let a = i as f32 * std::f32::consts::PI / 6.0;
                c + SimVec2::new(a.cos() * w.radius, a.sin() * w.radius)
            }).collect());
        }
        pieces
    }
}

pub struct CarPlugin;

impl Plugin for CarPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CarHit>()
            .add_systems(OnEnter(AppState::InGame), spawn_cars.after(crate::figure::load_man_art))
            .add_systems(
                Update,
                (
                    drive_cars.in_set(crate::FrameSet::FixedSteps).after(crate::player::run_fixed_steps).before(crate::weapon::step_projectiles),
                    hit_cars.in_set(crate::FrameSet::FixedSteps).after(crate::weapon::step_projectiles),
                    explode_cars.in_set(crate::FrameSet::Characters),
                    operate_car_gunners.in_set(crate::FrameSet::Characters).run_if(in_state(AppState::InGame)),
                ),
            )
            .add_systems(Update, run_exhausts.in_set(crate::FrameSet::Present).before(QueueSet::Collect).run_if(in_state(AppState::InGame)))
            .add_systems(Update, queue_cars.in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)));
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_cars(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let assets_dir = config.assets_dir();
    let cars = level.0.cars.clone();
    if cars.is_empty() {
        return;
    }
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut libraries: HashMap<String, Option<SpriteLibrary>> = HashMap::new();
    for (index, spec) in cars.into_iter().enumerate() {
        let spec = Arc::new(spec);
        let root_scale = spec.world.lossy_scale();
        let hull: Vec<Vec<SimVec2>> = spec.collider.iter().flat_map(|path| merge_convex(triangulate(&path.iter().map(|p| SimVec2::new(p[0], p[1])).collect::<Vec<_>>()))).collect();
        let pieces: Vec<Polygon> = hull.iter().map(|piece| Polygon::ccw(piece.iter().map(|p| SimVec2::new(p.x * root_scale[0], p.y * root_scale[1])).collect())).collect();
        let angle = spec.world.m[1][0].atan2(spec.world.m[0][0]).to_degrees();
        let mut chassis = RigidBody::new(SimVec2::new(spec.world.t[0], spec.world.t[1]), angle, spec.body.mass, spec.body.linear_drag, spec.body.angular_drag, spec.body.gravity_scale);
        if !pieces.is_empty() {
            let (lc, inertia) = mass_properties(&pieces, spec.body.mass);
            chassis.local_center = lc;
            chassis.inertia = inertia;
            chassis.inv_inertia = if inertia > 0.0 { 1.0 / inertia } else { 0.0 };
        }
        let key = (index as u64 + 1) << 20;
        let mut bodies = vec![DynamicBody { key, body: chassis, pieces, layer: spec.layer }];
        let mut joints = Vec::new();
        let root = spec.world;
        for (k, w) in spec.wheels.iter().enumerate() {
            let world = spec.world_of_node(w.node, &root);
            let mut body = RigidBody::new(SimVec2::new(world.t[0], world.t[1]), angle, w.body.mass, w.body.linear_drag, w.body.angular_drag, w.body.gravity_scale);
            body.local_center = SimVec2::new(w.offset[0], w.offset[1]);
            body.inertia = 0.5 * w.body.mass * w.radius * w.radius + w.body.mass * (w.offset[0] * w.offset[0] + w.offset[1] * w.offset[1]);
            body.inv_inertia = if body.inertia > 0.0 { 1.0 / body.inertia } else { 0.0 };
            bodies.push(DynamicBody { key: key + 1 + k as u64, body, pieces: vec![Polygon::disc(SimVec2::new(w.offset[0], w.offset[1]), w.radius)], layer: spec.layer });
            // `m_Angle` is a world angle; Box2D keeps the axis in A's frame.
            let world_axis = SimVec2::new(w.angle.to_radians().cos(), w.angle.to_radians().sin());
            let a = angle.to_radians();
            let local_axis = SimVec2::new(a.cos() * world_axis.x + a.sin() * world_axis.y, -a.sin() * world_axis.x + a.cos() * world_axis.y);
            joints.push(WheelJoint::new(0, 1 + k, SimVec2::new(w.anchor[0], w.anchor[1]), SimVec2::new(w.connected_anchor[0], w.connected_anchor[1]), local_axis, w.frequency, w.damping_ratio));
        }
        // Sprites riding on the car (its chassis or a wheel), with their shadow copies.
        let mut sprites = Vec::new();
        for (i, sprite) in level.0.sprites.iter().enumerate() {
            if sprite.rigid_root != Some(spec.root) || sprite.shadow_of.is_some() {
                continue;
            }
            let Some(node) = spec.nodes.iter().position(|n| n.id == sprite.object) else { continue };
            let lib = libraries.entry(sprite.collection.clone()).or_insert_with(|| SpriteLibrary::load(&assets_dir, &sprite.collection).ok());
            let Some(lib) = lib.as_ref() else { continue };
            let Some(def) = lib.collection.sprites.get(sprite.sprite_id) else { continue };
            let Some(path) = lib.texture_of(def) else { continue };
            let texture = textures.id(path, &mut load);
            let shadow = level.0.sprites.iter().find(|s| s.shadow_of.is_some_and(|(c, _)| c == i)).and_then(|s| {
                let plane = s.shadow_of.map_or(0.0, |(_, p)| p);
                let def = if s.collection == sprite.collection { lib.collection.sprites.get(s.sprite_id) } else { lib.collection.sprite(&format!("{} shadow", def.name)).or(Some(def)) }?;
                let path = lib.texture_of(def)?;
                Some((def.clone(), textures.id(path, &mut load), plane))
            });
            // `CopyLocalTransform`: a sprite under "Wheels" that starts where a wheel does.
            let local = spec.nodes[node].local_position;
            let wheel = spec.wheels.iter().position(|w| {
                let wl = spec.nodes[w.node].local_position;
                (wl[0] - local[0]).abs() < 0.01 && (wl[1] - local[1]).abs() < 0.01
            });
            sprites.push(CarSprite { node, wheel, def: def.clone(), tiled: sprite.tiled, scale: sprite.scale, color: sprite.color, texture, shadow });
        }
        let camera_offset = spec.camera_influencer.map(|i| {
            let p = spec.world_of_node(i, &root).t;
            SimVec2::new(p[0] - root.t[0], p[1] - root.t[1])
        });
        let mut health = Health::new(spec.starting_health);
        health.reset();
        let radii: Vec<f32> = spec.wheels.iter().map(|w| w.radius).collect();
        let mut car = Car {
            drive: CarDrive::new(spec.tuning, &radii),
            rocket: spec.rocket.map(|tuning| {
                // The engine in the body's frame (the root's own rotation undone).
                let engine = tuning.engine.map_or(SimVec2::ZERO, |e| {
                    let local = root.inverse().transform_point(spec.world_of_node(e, &root).t);
                    SimVec2::new(local[0] * root_scale[0], local[1] * root_scale[1])
                });
                (sticks_sim::car::RocketDrive::new(tuning), engine)
            }),
            passengers: spec.passengers.iter().map(|_| Passenger { slot: ControlSlot::new(spec.control_radius), exit_latch: ExitLatch::default(), exit: false }).collect(),
            gunners: spec
                .gunners
                .iter()
                .map(|g| CarGunnerSeat { slot: ControlSlot::new(g.control_radius), exit_latch: ExitLatch::default(), exit: false, fire: false, aim: None, team: 0, corrected: SimVec2::new(1.0, 0.0), weapon: None })
                .collect(),
            wheels_grounded: vec![false; spec.wheels.len()],
            bodies,
            joints,
            warm: WarmStart::default(),
            slot: Some(ControlSlot::new(spec.control_radius)),
            exit_latch: ExitLatch::default(),
            exit: false,
            input: (0.0, 0.0),
            health,
            sprites,
            node_active: spec.nodes.iter().map(|n| n.active_self).collect(),
            root_z: root.t[2],
            root_scale,
            camera_offset,
            path_point: None,
            body_touching: false,
            layer_unoccupied: spec.layer,
            layer_to_set: None,
            layer: spec.layer,
            shape: BodyShape::default(),
            hull,
            explode_pending: false,
            explosion: None,
            engine_volume: spec.engine_sound.as_ref().map_or(0.0, |e| e.volume),
            exhaust: spec.exhaust.as_ref().map(|(node, fx)| Exhaust { node: *node, spec: fx.clone(), system: sticks_sim::particles::ParticleSystem::play() }),
            spec,
        };
        car.rebuild_shape();
        info!("car {} with {} wheels{}", car.spec.name, car.spec.wheels.len(), if car.rocket.is_some() { " (rocket)" } else { "" });
        commands.spawn((Name::new(car.spec.name.clone()), car));
    }
}

/// `CarControl.FixedUpdate` and the physics step of every car, per fixed step; then the layer
/// switch (`layerToSet` once no other car is within 10 units) and the path influencer.
fn drive_cars(
    clock: Res<FrameClock>,
    collision: Res<Collision>,
    mut cars: Query<&mut Car>,
    mut memory: ResMut<crate::lobby::LobbyMemory>,
    players: Query<&crate::player::Player>,
) {
    let rules = &collision.rules;
    for &dt in &clock.steps {
        for mut car in &mut cars {
            if !car.alive() {
                continue;
            }
            let car = &mut *car;
            let input = car.driver().map(|_| car.input);
            match &mut car.rocket {
                Some((rocket, engine)) => rocket.fixed_update(&mut car.bodies[0].body, *engine, input, dt),
                None => {
                    // The flip goals: "rover 720" for two flips, also "rover 1440" for four.
                    let goal = car.drive.fixed_update(&mut car.bodies, &car.wheels_grounded, car.body_touching, input, dt);
                    if let (Some(goal), Some(player)) = (goal, car.driver().and_then(|d| players.get(d).ok())) {
                        let id = sticks_sim::level_end::PlayerId::anonymous_local(player.index);
                        let mut changed = memory.goals.goal_done(&id, "rover 720", None);
                        if goal == sticks_sim::car::FlipGoal::FourFlips {
                            changed |= memory.goals.goal_done(&id, "rover 1440", None);
                        }
                        info!("{}: {:?}", car.spec.name, goal);
                        if changed {
                            memory.save.goals = true;
                        }
                    }
                }
            }
            let touches = step_with_joints(&mut car.bodies, &mut car.joints, &collision.world, rules, dt, rules.velocity_iterations, rules.position_iterations, &mut car.warm);
            let n = car.wheels_grounded.len();
            for i in 0..n {
                car.wheels_grounded[i] = touches.iter().any(|t| matches!(t, Touch::Static { body, .. } if *body == 1 + i));
            }
            car.body_touching = touches.iter().any(|t| matches!(t, Touch::Static { body: 0, .. }));
            // `Rigidbody2DPathCameraInfluencer.FixedUpdate`: a quarter-second ballistic path
            // for up to five seconds; the first linecast hit on layer 0 (the rovers only).
            if car.driver().is_some() && car.rocket.is_none() {
                let b = car.bodies[0].body;
                let gravity = rules.gravity * b.gravity_scale;
                let (mut p, mut v) = (b.position, b.linear_velocity);
                car.path_point = None;
                let mut t = 0.0;
                while t < 5.0 {
                    let dv = gravity * 0.25;
                    let next = p + (v + dv * 0.5) * 0.25;
                    if collision.world.linecast(p, next, 1, rules.raycasts_hit_triggers) {
                        // The hit point: bisect along the segment.
                        let (mut lo, mut hi) = (0.0f32, 1.0f32);
                        for _ in 0..12 {
                            let mid = (lo + hi) * 0.5;
                            if collision.world.linecast(p, p + (next - p) * mid, 1, rules.raycasts_hit_triggers) {
                                hi = mid;
                            } else {
                                lo = mid;
                            }
                        }
                        car.path_point = Some(p + (next - p) * hi);
                        break;
                    }
                    t += 0.25;
                    p = next;
                    v = v + dv;
                }
            }
        }
    }
    let positions: Vec<SimVec2> = cars.iter().filter(|c| c.alive()).map(|c| c.position()).collect();
    for mut car in &mut cars {
        car.rebuild_shape();
        if let Some(layer) = car.layer_to_set {
            let p = car.position();
            let near = positions.iter().filter(|q| (**q - p).sqr_magnitude() < 100.0).count() > 1;
            if !near {
                car.layer = layer;
                car.layer_to_set = None;
            }
        }
        let layer = car.layer;
        for b in &mut car.bodies {
            b.layer = layer;
        }
    }
}

/// Car hulls as moving obstacles for the characters, appended after the helicopters'.
pub fn car_obstacles(cars: &Query<&Car>) -> Vec<(StaticCollider, Vec<Vec<SimVec2>>)> {
    cars.iter()
        .filter(|c| c.is_target())
        .enumerate()
        .map(|(i, c)| (StaticCollider { id: CAR_COLLIDER_BASE + i as u32, layer: c.layer, ..Default::default() }, c.obstacle_pieces()))
        .collect()
}

/// `Health.Damage` from projectiles (`TellOwnerToExplodeOnHealthExhausted`).
fn hit_cars(mut hits: MessageReader<CarHit>, mut cars: Query<&mut Car>) {
    for hit in hits.read() {
        let Ok(mut car) = cars.get_mut(hit.car) else { continue };
        if !car.alive() {
            continue;
        }
        // The lunar module has a `Health` nothing acts on.
        if !car.spec.explodes {
            continue;
        }
        car.health.damage(hit.amount);
        if !car.alive() {
            info!("{} destroyed", car.spec.name);
            car.explode_pending = true;
        }
    }
}

/// `Explodable.Explode` (`EjectControllersOnExplode` happens in `crate::vehicles`), the
/// explosion and `ExplosionFinished`: the car is gone.
#[allow(clippy::too_many_arguments)]
fn explode_cars(
    mut commands: Commands,
    art: Option<Res<ManArt>>,
    light: Res<SceneLight>,
    clock: Res<FrameClock>,
    mut rng: ResMut<crate::life::DeathRng>,
    mut sounds: MessageWriter<PlaySound>,
    mut cars: Query<(Entity, &mut Car)>,
) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut car) in &mut cars {
        if car.explode_pending {
            car.explode_pending = false;
            let root = car.root_affine();
            let at = SimVec3::new(root.t[0], root.t[1], root.t[2]);
            if let Some(sound) = &car.spec.explode_sound {
                sounds.write(PlaySound { group: sound.clone(), at: Some(at), owner: None });
            }
            let (Some(fade_wait), Some(art)) = (car.spec.explosion_fade_wait, art.as_ref()) else {
                commands.entity(entity).despawn();
                continue;
            };
            let mut parts_art = sticks_sim::visual::VisualArt::empty(art.art.line_uvs());
            let mut parts = Vec::new();
            for s in &car.sprites {
                if !car.active_in_hierarchy(s.node) || s.tiled.is_some() {
                    continue;
                }
                parts_art.add_def(&s.def, s.shadow.as_ref().map(|(d, _, _)| d), s.texture);
                let (world, q) = sprite_pose(&car, s, &root);
                let position = SimVec3::new(world.t[0], world.t[1], world.t[2]);
                let plane = s.shadow.as_ref().map_or(0.0, |(_, _, p)| *p);
                let (shadow_offset, shadow_scale) = match light.for_shadows {
                    Some(l) => shadow_offset_and_scale(l, position, plane),
                    None => ([0.0, 0.0], 1.0),
                };
                let lossy = world.lossy_scale();
                parts.push(sticks_sim::visual::FlatSpriteInstance {
                    bone: 0,
                    sprite_name: s.def.name.clone(),
                    position,
                    flipped: false,
                    roll_degrees: 0.0,
                    tilt: [q.x, q.y, q.z, q.w],
                    scale: [s.scale[0] * lossy[0], s.scale[1] * lossy[1], s.scale[2] * lossy[2]],
                    color: s.color,
                    shadow_plane_z: plane,
                    shadow_offset,
                    shadow_scale,
                });
            }
            let visual = sticks_sim::visual::CharacterVisual { shadows: Vec::new(), lines: Vec::new(), sprites: parts };
            let explosion = sticks_sim::explosion::Explosion::go(&visual, at, at, fade_wait, &mut rng.0, clock.last_step_delta);
            car.explosion = Some((explosion, parts_art));
            continue;
        }
        if car.explosion.as_mut().is_some_and(|(e, _)| e.update(delta)) {
            commands.entity(entity).despawn();
        }
    }
}

/// Where a sprite is drawn: under the chassis, or at its wheel (`CopyLocalTransform`), and its
/// world rotation.
fn sprite_pose(car: &Car, s: &CarSprite, root: &Affine) -> (Affine, SimQuat) {
    let world = car.node_world(s.node, root);
    match s.wheel {
        Some(w) => {
            let b = &car.bodies[1 + w].body;
            let q = SimQuat::about_z(b.angle);
            (Affine::from_trs([b.position.x, b.position.y, world.t[2]], (q.x, q.y, q.z, q.w), world.lossy_scale()), q)
        }
        None => (world, node_rotation(car, s.node)),
    }
}

/// World rotation of a node under the chassis.
fn node_rotation(car: &Car, node: usize) -> SimQuat {
    let mut q = SimQuat::about_z(car.bodies[0].body.angle);
    let mut chain = Vec::new();
    let mut current = Some(node);
    while let Some(c) = current {
        chain.push(c);
        current = car.spec.nodes[c].parent;
    }
    for &c in chain.iter().rev().skip(1) {
        q = q * SimQuat::from_array(car.spec.nodes[c].local_rotation);
    }
    q
}

#[allow(clippy::too_many_arguments)]
fn queue_cars(
    light: Res<SceneLight>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LevelCamera>>,
    art: Option<Res<ManArt>>,
    animation: Option<Res<CharacterAnimation>>,
    cars: Query<(Entity, &Car)>,
    pilots: Query<(&Piloting, &ControlledBy, Option<&Palette>), With<Disabled>>,
    mut queue: ResMut<FrameQueue>,
) {
    let Ok(rig) = rigs.single() else { return };
    let view = camera_view(rig, &windows);
    let light = light.for_shadows;
    for (entity, car) in &cars {
        if let (Some((explosion, parts_art)), Some(art)) = (&car.explosion, art.as_ref()) {
            for (texture, g) in explosion.renderers_textured(parts_art, light) {
                if !g.is_empty() {
                    queue.0.push_frame(texture.unwrap_or(art.texture), g);
                }
            }
            continue;
        }
        if !car.alive() {
            continue;
        }
        let root = car.root_affine();
        let mut renderers: Vec<(u32, Geometry)> = Vec::new();
        for s in &car.sprites {
            if !car.active_in_hierarchy(s.node) {
                continue;
            }
            let (world, rotation) = sprite_pose(car, s, &root);
            renderers.push((s.texture, scene_sprite_geometry(&s.def, s.tiled.as_ref(), s.scale, s.color, &world)));
            if let (Some((def, texture, plane)), Some(light)) = (&s.shadow, light) {
                renderers.push((*texture, sprite_shadow_geometry(def, s.tiled.as_ref(), &world, rotation, s.scale, s.color[3], *plane, light)));
            }
        }
        // `ParticleSystemLines` of the rocket's exhaust.
        if let (Some(ex), Some(art)) = (&car.exhaust, art.as_ref()) {
            if let Some(lines) = &ex.spec.lines {
                let w = car.node_world(ex.node, &root).t;
                let drawing = ex.system.drawing(&ex.spec.system, lines, ex.spec.rotation_follows_velocity, SimVec3::new(w[0], w[1], w[2]), light);
                for g in drawing.geometry(art.art.line_uvs()) {
                    renderers.push((art.texture, g));
                }
            }
        }
        // `CarControl.SetController` → `driver.enabled`: the driver sits in the seat in the
        // player's colours while somebody drives.
        let palette_of = |player: Entity| pilots.iter().find(|(p, c, _)| p.vehicle == entity && c.0 == player).and_then(|(_, _, p)| p.map(|p| p.0));
        if let (Some(seat), Some(driver), Some(art), Some(animation)) = (&car.spec.driver, car.driver(), art.as_ref(), animation.as_ref()) {
            if car.active_in_hierarchy(seat.node) {
                let world = car.node_world(seat.node, &root);
                let at = SimVec3::new(world.t[0], world.t[1], world.t[2]);
                renderers.extend(art.occupant_renderers(&animation.0, &seat.animation_state, at, node_rotation(car, seat.node), palette_of(driver).unwrap_or_else(|| art.prefab_palette()), light, None));
            }
        }
        // `CarGunnerControl.SetController` → `gunner.enabled`: the gunner with the weapon aimed.
        if let (Some(art), Some(animation)) = (art.as_ref(), animation.as_ref()) {
            for (g, seat) in car.gunners.iter().enumerate() {
                let (Some(gunner), Some((weapon, _))) = (seat.gunner(), seat.weapon.as_ref()) else { continue };
                let spec = &car.spec.gunners[g].seat;
                if !car.active_in_hierarchy(spec.node) {
                    continue;
                }
                let world = car.node_world(spec.node, &root);
                let at = SimVec3::new(world.t[0], world.t[1], world.t[2]);
                let aimed = sticks_sim::aim::AimedWeapon { weapon: *weapon, aim: seat.corrected, face_right: true };
                renderers.extend(art.occupant_renderers(&animation.0, &spec.animation_state, at, node_rotation(car, spec.node), palette_of(gunner).unwrap_or_else(|| art.prefab_palette()), light, Some(&aimed)));
            }
        }
        for (texture, g) in renderers {
            if !g.is_empty() && view.may_see(g.bounds_center(), crate::unity_render::extent(&g)) {
                queue.0.push_frame(texture, g);
            }
        }
    }
}

/// `CarGunnerControl.Update` and the occupant weapon's `ProjectileLauncher.Update`: the weapon
/// pivot turns to `CorrectAndSetAim(RelativeAim(pivot), faceRight: true, canAimBackwards: true)`
/// and fires along the relative aim from its launch point.
#[allow(clippy::too_many_arguments)]
fn operate_car_gunners(
    mut commands: Commands,
    clock: Res<FrameClock>,
    session: Res<Session>,
    collision: Res<Collision>,
    art: Option<Res<ManArt>>,
    animation: Option<Res<CharacterAnimation>>,
    library: Option<Res<ProjectileLibrary>>,
    mut cars: Query<&mut Car>,
    mut sounds: MessageWriter<PlaySound>,
) {
    let (Some(art), Some(animation), Some(library)) = (art, animation, library) else { return };
    let weapons = &art.model().rig().weapons;
    let delta = clock.stepper.frame().delta;
    for mut car in &mut cars {
        if !car.is_target() {
            continue;
        }
        let spec = car.spec.clone();
        let root = car.root_affine();
        for (g, gunner_spec) in spec.gunners.iter().enumerate() {
            // `SetUpNewWeapon(GetWeaponPivots(occupant), weaponName, ...)` on the first `Update`.
            if car.gunners[g].weapon.is_none() {
                let Some(index) = weapons.iter().position(|w| w.name == gunner_spec.weapon) else { continue };
                let w = &weapons[index];
                let chargeable = library.0.get(&w.projectile).is_some_and(|p| p.chargeable);
                car.gunners[g].weapon = Some((index, Launcher::new(w.firing_rate_time, w.fully_automatic, chargeable)));
            }
            let Some(responsible) = car.gunners[g].gunner() else { continue };
            let Some(aim) = car.gunners[g].aim else { continue };
            let Some(index) = car.gunners[g].weapon.as_ref().map(|(i, _)| *i) else { continue };
            let weapon = &weapons[index];
            let world = car.node_world(gunner_spec.seat.node, &root);
            let at = SimVec3::new(world.t[0], world.t[1], world.t[2]);
            let rotation = node_rotation(&car, gunner_spec.seat.node);
            // The occupant is posed at the usual depth; its points move to the seat's z.
            let dz = at.z - sticks_sim::character::DEFAULT_Z;
            let previous = car.gunners[g].corrected;
            let aimed = |aim: SimVec2| sticks_sim::aim::AimedWeapon { weapon: index, aim, face_right: true };
            let Some(posed) = art.occupant_posed(&animation.0, &gunner_spec.seat.animation_state, at, rotation, Some(&aimed(previous))) else { continue };
            let pivot = posed.world_position(weapon.bone);
            let relative = aim.relative_to([pivot.x, pivot.y, pivot.z + dz]);
            let corrected = corrected_aim_with(relative, previous, true, true);
            car.gunners[g].corrected = corrected;
            let fire = car.gunners[g].fire;
            let Some((_, launcher)) = car.gunners[g].weapon.as_mut() else { continue };
            let Some(launch) = launcher.update(fire, clock.time, delta) else { continue };
            let (Some(projectile), Some(launch_point)) = (library.0.get(&weapon.projectile), weapon.launch_point) else { continue };
            let Some(posed) = art.occupant_posed(&animation.0, &gunner_spec.seat.animation_state, at, rotation, Some(&aimed(corrected))) else { continue };
            let muzzle = posed.world_position(launch_point);
            let damage = match &projectile.damage_scaling {
                Some(s) if session.mode == GameMode::Campaign => difficulty_modified(projectile.damage, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
                _ => projectile.damage,
            };
            let team = car.gunners[g].team;
            let shot = Shot::launch(projectile, damage, SimVec2::new(muzzle.x, muzzle.y), relative, launch.sub_frame_offset, team.into(), collision.rules.gravity);
            commands.spawn((Name::new(projectile.name.clone()), Projectile::new(shot, projectile.clone(), Some(responsible))));
            if !weapon.firing_sound.is_empty() {
                sounds.write(PlaySound { group: weapon.firing_sound.clone(), at: Some(SimVec3::new(muzzle.x, muzzle.y, muzzle.z + dz)), owner: None });
            }
        }
    }
}

/// `RocketControl.Update` (`engineParticles.emissionRate`) and the exhaust's simulation, after
/// the frame's `LateUpdate` like the other particle systems.
fn run_exhausts(clock: Res<FrameClock>, collision: Res<Collision>, mut rng: ResMut<crate::effects::ParticleRng>, mut cars: Query<&mut Car>) {
    let delta = clock.stepper.frame().delta;
    let gravity = collision.rules.gravity;
    for mut car in &mut cars {
        let car = &mut *car;
        let (Some(node), Some((rocket, _))) = (car.exhaust.as_ref().map(|e| e.node), car.rocket.as_ref()) else { continue };
        if !car.is_target() {
            continue;
        }
        let rate = rocket.engine_volume() * rocket.tuning.engine_particle_rate;
        let w = car.node_world(node, &car.root_affine()).t;
        let rotation = node_rotation(car, node);
        let Some(ex) = car.exhaust.as_mut() else { continue };
        ex.spec.system.emission.rate = unity_data::particles::MinMaxCurve::constant(rate);
        ex.system.update_oriented(&ex.spec.system, delta, SimVec3::new(w[0], w[1], w[2]), rotation, gravity, &mut rng.0);
    }
}
