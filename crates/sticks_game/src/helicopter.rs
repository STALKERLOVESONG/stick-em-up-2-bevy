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
//! Not yet (see the handoff's helicopter plan): contacts with the level and other bodies (the
//! Box2D contact solver port), the seated occupants (H5: the pilot and passenger figures are not
//! drawn), the explosion (H4) and with it the player helicopter's respawn
//! (`RespawnerInstantiator`, `RespawnAtStartPosition`), which waits for `ExplosionFinished`.
//! TODO(UNCERTAIN): `HelicopterInput` and the AI behaviours share execution order 0; the input is
//! taken to copy the processor before the behaviours run (component order), so the flight acts on
//! the previous frame's decision, as for enemy soldiers.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::ai::{Candidate, OffScreenSleep, Targeter};
use sticks_sim::draw::{BACK, Geometry, color32, scene_sprite_geometry, shadow_offset_and_scale, sprite_shadow_geometry};
use sticks_sim::helicopter::{HelicopterControl, back_rotor_blades, gun_angle_degrees, move_to_first_target_height, top_rotor_blades};
use sticks_sim::life::Health;
use sticks_sim::physics::{BodyShape, triangulate};
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
}

/// One gun: launcher, its current world angle, whether it is switched on.
struct Gun {
    launcher: Launcher,
    angle: f32,
    active: bool,
}

#[derive(Component)]
pub struct Helicopter {
    pub(crate) spec: Arc<HelicopterSpawn>,
    pub body: RigidBody,
    control: HelicopterControl,
    /// `HelicopterInput` (copied from the processor) and the processor itself
    /// (`EnemyHelicopterInput`): movement, fire, absolute aim.
    pub(crate) input: (SimVec2, bool, Option<SimVec2>),
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
    /// The hull as projectiles see it (rotated to the body's angle, relative to its position).
    pub shape: BodyShape,
    pub layer: u8,
    sprites: Vec<HeliSprite>,
    node_active: Vec<bool>,
    root_z: f32,
    root_scale: [f32; 3],
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

    fn alive(&self) -> bool {
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
    pub(crate) fn take_control(&mut self, player: Entity, team: u8) {
        let Some(slot) = self.slot.as_mut() else { return };
        slot.set_controller(player);
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
    /// (`IsPilotAlsoAGunner`: one active player, which is always the case here). The other
    /// guns' own `ControlSlot`s are gunner seats for further local players.
    fn gun_has_input(&self, g: usize) -> bool {
        if self.spec.enemy.is_some() { self.controlled } else { self.pilot().is_some() && self.spec.single_player_gun == Some(g) }
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
                    fly_helicopters.in_set(crate::FrameSet::FixedSteps).before(crate::weapon::step_projectiles),
                    hit_helicopters.in_set(crate::FrameSet::FixedSteps).after(crate::weapon::step_projectiles),
                    (sleep_helicopters, think_helicopters, fire_helicopter_guns).chain().in_set(crate::FrameSet::Characters),
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
            let in_gun = spec.guns.iter().any(|g| {
                let mut current = Some(node);
                while let Some(c) = current {
                    if c == g.node {
                        return true;
                    }
                    current = spec.nodes[c].parent;
                }
                false
            });
            if in_gun || Some(node) == spec.fuselage {
                color = body_color.unwrap_or([1.0; 4]);
            }
            sprites.push(HeliSprite { node, def: def.clone(), tiled: sprite.tiled, scale: sprite.scale, color, texture, shadow });
        }
        let guns = spec
            .guns
            .iter()
            .map(|g| Gun {
                launcher: Launcher::new(modified(g.firing_rate_time, &g.firing_rate_scaling), g.fully_automatic, false),
                angle: 0.0,
                active: in_hierarchy(g.node),
            })
            .collect();
        let hull = spec
            .collider
            .iter()
            .flat_map(|path| triangulate(&path.iter().map(|p| SimVec2::new(p[0], p[1])).collect::<Vec<_>>()))
            .collect();
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
        // TODO(UNCERTAIN): the hull's mass properties (inertia, centre of mass) are not computed
        // yet; no contacts apply torque until the contact solver exists.
        body.inertia = 0.0;
        let enemy = spec.enemy.is_some();
        // `FixWorldOffsetFromTransform.Awake`: the offset is taken in world space once.
        let camera_offset = spec.camera_influencer.map(|i| {
            let p = spec.world_of(i, &spec.world, &|_| None).t;
            SimVec2::new(p[0] - spec.world.t[0], p[1] - spec.world.t[1])
        });
        if enemy && spec.control_slot {
            warn!("{}: an enemy helicopter with a usable ControlSlot; players could board it", spec.name);
        }
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
            shape: BodyShape::default(),
            // `SetEnemyLayer` puts an enemy helicopter's whole tree on layer 9.
            layer: if enemy { (8 + ENEMY_TEAM) as u8 } else { spec.layer },
            sprites,
            node_active,
            root_z: spec.world.t[2],
            root_scale: spec.world.lossy_scale(),
            spec,
        };
        helicopter.rebuild_shape();
        commands.spawn((Name::new(helicopter.spec.name.clone()), helicopter));
    }
    info!("{} helicopters", level.0.helicopters.len());
}

/// `HelicopterInput.OwnerOnlyUpdate`: copy the processor (enemies' `EnemyHelicopterInput`).
fn copy_helicopter_input(mut helicopters: Query<&mut Helicopter>) {
    for mut h in &mut helicopters {
        if h.asleep.is_none() && h.spec.enemy.is_some() {
            h.input = h.processor;
        }
    }
}

/// `HelicopterControl.FixedUpdate` and the physics step, for every fixed step of the frame.
/// TODO: the Box2D contact solver (hull vs level and other bodies).
fn fly_helicopters(clock: Res<FrameClock>, collision: Res<Collision>, mut helicopters: Query<&mut Helicopter>) {
    for mut h in &mut helicopters {
        if h.asleep.is_some() || !h.alive() {
            continue;
        }
        for &dt in &clock.steps {
            let input = if h.controlled { Some(h.input.0) } else { None };
            let h = &mut *h;
            // `FixWorldOffsetFromTransform.FixedUpdate` runs before the physics step.
            if let Some(offset) = h.camera_offset {
                h.camera_point = h.body.position + offset;
            }
            h.control.fixed_update(&mut h.body, input, dt);
            h.body.step(dt, collision.rules.gravity, &collision.rules.box2d);
        }
        h.rebuild_shape();
    }
}

/// `Health.Damage` from projectiles; `HelicopterColor` flashes the fuselage.
/// TODO: the explosion (`ExplodeSpritesAndLines`, occupants) when the health runs out.
fn hit_helicopters(mut hits: MessageReader<HelicopterHit>, mut helicopters: Query<&mut Helicopter>) {
    for hit in hits.read() {
        let Ok(mut h) = helicopters.get_mut(hit.helicopter) else { continue };
        if !h.alive() {
            continue;
        }
        h.health.damage(hit.amount);
        if h.alive() {
            h.flash = 0.1;
        } else {
            info!("{} destroyed", h.spec.name);
        }
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
        // `HelicopterGunnerControl.Update`: a gun with an input faces `RelativeAim(gun position)`.
        if h.controlled {
            if let Some(aim) = h.input.2 {
                for g in 0..h.guns.len() {
                    if !h.guns[g].active || !h.gun_has_input(g) {
                        continue;
                    }
                    let node = h.spec.guns[g].node;
                    let p = h.node_world(node, &root).t;
                    h.guns[g].angle = gun_angle_degrees(SimVec2::new(aim.x - p[0], aim.y - p[1]));
                }
            }
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
        if h.asleep.is_some() || !h.alive() || !h.controlled {
            continue;
        }
        let (fire, aim) = (h.input.1, h.input.2);
        let root = h.root_affine();
        for g in 0..h.guns.len() {
            // `ProjectileLauncher.Update` returns at once without an input.
            if !h.guns[g].active || !h.gun_has_input(g) {
                continue;
            }
            let Some(launch) = h.guns[g].launcher.update(fire, clock.time, delta) else { continue };
            let spec_gun = &h.spec.guns[g];
            let Some(spec) = library.0.get(&spec_gun.projectile) else { continue };
            let pivot = h.node_world(spec_gun.node, &root).t;
            let muzzle = h.node_world(spec_gun.launch_point, &root).t;
            let Some(aim) = aim else { continue };
            let direction = SimVec2::new(aim.x - pivot[0], aim.y - pivot[1]);
            let damage = match &spec.damage_scaling {
                Some(s) if session.mode == GameMode::Campaign => difficulty_modified(spec.damage, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
                _ => spec.damage,
            };
            // A player's input object is the `Player`: `SetResponsiblePlayer` credits it (and
            // gives its team); an enemy's input object is not, so the shot is team 1.
            let (team, responsible) = if h.spec.enemy.is_some() { (ENEMY_TEAM, None) } else { (h.pilot_team.into(), h.pilot()) };
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
fn lines_geometry(lines: &[StickLine], origin: SimVec3, light: Option<SimVec3>, line_uvs: [[f32; 2]; 8]) -> Geometry {
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
    mut helicopters: Query<&mut Helicopter>,
    mut queue: ResMut<FrameQueue>,
) {
    let Some(art) = art else { return };
    let Ok(rig) = rigs.single() else { return };
    let view = camera_view(rig, &windows);
    let camera = view.camera;
    let light = light.for_shadows;
    let delta = clock.stepper.frame().delta;
    for mut h in &mut helicopters {
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
        for (texture, g) in renderers {
            if !g.is_empty() && view.may_see(g.bounds_center(), crate::unity_render::extent(&g)) {
                queue.0.push_frame(texture, g);
            }
        }
    }
}
