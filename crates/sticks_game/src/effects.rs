//! Projectile impacts and particle effects.
//!
//! * A projectile that explodes (`Explodable.Explode`, from a contact or from running out of
//!   time) raises [`ProjectileExploded`]. Handled in the same physics callback: its
//!   `PlaySoundOnExplode` sound, its `SpawnLocallyOnExplode` effect at its position, and — when
//!   it has `ExplodeSpritesAndLines` and that explosion object exists already — an [`Explosion`]
//!   of its sprites ([`ProjectileExplosion`]), whose parts stay pinned to whatever the projectile
//!   hit (`sticks`) and follow that object's transform every frame (`sticks_sim::explosion`).
//! * Particle effects ([`ParticleFx`]): `Rocket Trail` follows its rocket
//!   (`LifetimeParticleSystem`) and stops emitting when the rocket goes; explosions burst and
//!   die. They are drawn by `ParticleSystemLines` in `LateUpdate` (before the frame's particle
//!   simulation, which runs after it) and despawn when the system is dead
//!   (`DespawnOnParticleSystemDeath.Update`).

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use sticks_sim::explosion::{Anchor, AnchorId, CollisionData, Explosion, ProjectilePartSource};
use sticks_sim::life::Stage;
use sticks_sim::particles::ParticleEffect;
use sticks_sim::rng::GameRng;
use unity_compat::{Quat as SimQuat, Vec3 as SimVec3};
use unity_data::particles::{ParticleEffectSpec, load_particle_effect};
use unity_data::projectile::ProjectileSpec;

use crate::config::GameConfig;
use crate::figure::ManArt;
use crate::level::{ActiveLevel, Collision};
use crate::life::{DeathRng, Mortal};
use crate::player::{Avatar, FrameClock};
use crate::state::AppState;
use crate::unity_render::{FrameQueue, QueueSet, SceneLight};
use crate::weapon::{Posed, ProjectileLibrary};
use crate::FrameSet;

/// What a projectile hit, as the explosion needs it (`collision.transform`: the rigidbody's
/// object when there is one, else the collider's).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnchorRef {
    /// A character's root (`CharacterControl2D`'s rigidbody object).
    Avatar(Entity),
    /// A helicopter's root.
    Helicopter(Entity),
    /// A static level collider (letters included).
    Collider(u32),
}

const COLLIDER_TAG: u64 = 1 << 63;

impl AnchorRef {
    pub fn id(self) -> AnchorId {
        match self {
            AnchorRef::Avatar(e) | AnchorRef::Helicopter(e) => e.to_bits() & !COLLIDER_TAG,
            AnchorRef::Collider(id) => COLLIDER_TAG | u64::from(id),
        }
    }
}

/// `Explodable.Explode(causedByCollision, collisionPoint, lastRotationBeforeCollision,
/// collidedWith)` for a projectile.
#[derive(Message, Clone, Debug)]
pub struct ProjectileExploded {
    pub spec: Arc<ProjectileSpec>,
    /// The projectile entity (still alive; this handler decides when it goes).
    pub projectile: Entity,
    /// The projectile's root position and rotation (degrees about z) now.
    pub root: SimVec3,
    pub rotation_degrees: f32,
    pub collision: Option<CollisionData>,
    pub anchor: Option<AnchorRef>,
    /// `ExplodeSpritesAndLines.explosion != null` (its `Start` coroutine has run).
    pub prepared: bool,
    /// `Time.deltaTime` where `Explode` was called (the fixed step inside physics callbacks).
    pub delta: f32,
}

/// A projectile's sprites flying or pinned after it exploded.
#[derive(Component)]
pub struct ProjectileExplosion {
    explosion: Explosion,
    anchor: Option<AnchorRef>,
}

/// A spawned particle-effect prefab.
#[derive(Component)]
pub struct ParticleFx {
    spec: Arc<ParticleEffectSpec>,
    effect: ParticleEffect,
    /// `LifetimeParticleSystem`: the projectile it follows and the follower's offset from it.
    follow: Option<(Entity, SimVec3)>,
}

/// Effect prefabs by GUID (the ones projectiles spawn or carry).
#[derive(Resource, Default)]
pub struct EffectLibrary(pub HashMap<String, Arc<ParticleEffectSpec>>);

/// Shuriken's own random stream (separate from `UnityEngine.Random`).
#[derive(Resource)]
pub struct ParticleRng(pub GameRng);

impl Default for ParticleRng {
    fn default() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(7, |d| d.subsec_nanos().rotate_left(7));
        ParticleRng(GameRng::from_seed(seed))
    }
}

/// A sound for the audio module (`MasterAudio.PlaySound3DAtTransform[AndForget]`).
#[derive(Message, Clone, Debug)]
pub struct PlaySound {
    pub group: String,
    /// Where the voice is placed (game space); `None` for 2D/UI sounds.
    pub at: Option<SimVec3>,
    /// The object that owns it (`LifetimeSound`): [`FadeOutSounds`] for that owner fades it.
    pub owner: Option<Entity>,
}

/// `LifetimeSound.OnDisable` → `FadeOutNow()` for every voice `owner` started.
#[derive(Message, Clone, Copy, Debug)]
pub struct FadeOutSounds {
    pub owner: Entity,
}

pub struct EffectsPlugin;

impl Plugin for EffectsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ProjectileExploded>()
            .add_message::<PlaySound>()
            .add_message::<FadeOutSounds>()
            .init_resource::<EffectLibrary>()
            .init_resource::<ParticleRng>()
            .add_systems(OnEnter(AppState::InGame), load_effects.after(crate::figure::load_man_art))
            .add_systems(OnExit(AppState::InGame), clear_effects)
            .add_systems(Update, explode_projectiles.in_set(FrameSet::FixedSteps).after(crate::life::apply_damage))
            .add_systems(
                Update,
                (tick_projectile_explosions.after(crate::life::tick_lives), despawn_dead_particle_systems).in_set(FrameSet::Characters),
            )
            .add_systems(Update, follow_projectiles.in_set(FrameSet::Present))
            .add_systems(Update, (queue_projectile_explosions, queue_particles).in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)))
            .add_systems(Update, simulate_particles.in_set(QueueSet::Draw).run_if(in_state(AppState::InGame)));
    }
}

/// Every particle prefab a loaded projectile can spawn (`SpawnLocallyOnExplode`) or carry
/// (`LifetimeParticleSystem`).
fn load_effects(config: Res<GameConfig>, library: Option<Res<ProjectileLibrary>>, mut effects: ResMut<EffectLibrary>) {
    let Some(library) = library else { return };
    let assets = config.assets_dir();
    let guids: Vec<String> = library
        .0
        .values()
        .flat_map(|spec| spec.spawns_on_explode.iter().cloned().chain(spec.lifetime_particles.iter().map(|(g, _)| g.clone())))
        .collect();
    for guid in guids {
        if effects.0.contains_key(&guid) {
            continue;
        }
        match load_particle_effect(&assets, &guid) {
            Ok(Some(spec)) => {
                effects.0.insert(guid, Arc::new(spec));
            }
            Ok(None) => info!("effect prefab {guid} has no particle system"),
            Err(e) => warn!("effect prefab {guid}: {e}"),
        }
    }
}

fn clear_effects(mut commands: Commands, explosions: Query<Entity, With<ProjectileExplosion>>, particles: Query<Entity, With<ParticleFx>>) {
    for e in explosions.iter().chain(particles.iter()) {
        commands.entity(e).try_despawn();
    }
}

/// The current transform of a projectile's anchor while it is active.
#[derive(bevy::ecs::system::SystemParam)]
pub struct Anchors<'w, 's> {
    avatars: Query<'w, 's, (&'static Avatar, &'static Mortal, Option<&'static Posed>)>,
    helicopters: Query<'w, 's, &'static crate::helicopter::Helicopter>,
    collision: Option<Res<'w, Collision>>,
}

impl Anchors<'_, '_> {
    /// `collidedWith` with `activeInHierarchy`: `None` when inactive or gone.
    pub fn resolve(&self, anchor: AnchorRef) -> Option<Anchor> {
        match anchor {
            AnchorRef::Avatar(e) => {
                let (avatar, mortal, posed) = self.avatars.get(e).ok()?;
                if !mortal.avatar_active() {
                    return None;
                }
                Some(avatar_anchor(avatar, posed))
            }
            AnchorRef::Helicopter(e) => self.helicopters.get(e).ok().filter(|h| h.is_active_object()).map(|h| h.anchor()),
            // Static colliders do not move; only whether their object is still there matters.
            AnchorRef::Collider(id) => self.collision.as_ref().is_some_and(|c| c.world.is_enabled(id)).then_some(Anchor::IDENTITY),
        }
    }

    /// `TellOwnerToExplodeOnCollisionUnlessOtherIsDestroyed.IsDestroyed`: gone, or exploded
    /// (`ExplodeSpritesAndLines` hides the object in the hierarchy when it explodes).
    pub fn is_destroyed(&self, anchor: AnchorRef) -> bool {
        match anchor {
            AnchorRef::Avatar(e) => self.avatars.get(e).map_or(true, |(_, m, _)| m.stage() != Stage::Alive),
            AnchorRef::Helicopter(e) => self.helicopters.get(e).map_or(true, |h| !h.is_alive()),
            // A letter explodes from any hit (its health is 6.1e-5); other colliders never go.
            AnchorRef::Collider(id) => id >= crate::letters::LETTER_COLLIDER_BASE || !self.collision.as_ref().is_some_and(|c| c.world.is_enabled(id)),
        }
    }
}

/// A character root's transform: rigidbody position, the root's depth and its facing rotation
/// (`CharacterControl2D`: ±90° about Y).
pub fn avatar_anchor(avatar: &Avatar, posed: Option<&Posed>) -> Anchor {
    let p = avatar.position();
    match posed {
        Some(posed) => Anchor { position: SimVec3::new(p.x, p.y, posed.placement.position.z), rotation: posed.placement.rotation, scale: SimVec3::new(1.0, 1.0, 1.0) },
        None => Anchor {
            position: SimVec3::new(p.x, p.y, 0.0),
            rotation: SimQuat::euler_y_degrees(if avatar.facing_right() { 90.0 } else { -90.0 }),
            scale: SimVec3::new(1.0, 1.0, 1.0),
        },
    }
}

/// The handlers of `Explodable.Exploded` / `ExplodedWithCollisionData` on a projectile.
#[allow(clippy::too_many_arguments)]
fn explode_projectiles(
    mut commands: Commands,
    mut exploded: MessageReader<ProjectileExploded>,
    effects: Res<EffectLibrary>,
    mut rng: ResMut<DeathRng>,
    mut sounds: MessageWriter<PlaySound>,
    mut pools: ResMut<crate::weapon::ProjectilePools>,
) {
    for e in exploded.read() {
        let spec = &e.spec;
        // `SpawnLocallyOnExplode`: `Pool.Spawn(prefab, t.position, t.rotation)`.
        if let Some(fx) = spec.spawns_on_explode.as_ref().and_then(|g| effects.0.get(g)) {
            commands.spawn((Name::new(fx.name.clone()), ParticleFx { spec: fx.clone(), effect: ParticleEffect::spawn(e.root), follow: None }));
        }
        // `PlaySoundOnExplode`.
        if let Some(sound) = &spec.explosion_sound {
            sounds.write(PlaySound { group: sound.clone(), at: Some(e.root), owner: None });
        }
        let Some(explosion_spec) = spec.explosion else { continue };
        if !e.prepared {
            // `explosion == null`: `explodable.Finished()` at once → `DespawnOnExplosionFinished`.
            if spec.despawns_on_explosion_finished {
                commands.entity(e.projectile).try_despawn();
                pools.release(&spec.guid);
            }
            continue;
        }
        let turn = SimQuat::about_z(e.rotation_degrees.to_radians());
        let parts: Vec<ProjectilePartSource> = spec
            .sprites
            .iter()
            .map(|s| {
                let local = SimVec3::new(s.position[0], s.position[1], s.position[2]);
                ProjectilePartSource {
                    sprite_name: s.sprite_name.clone(),
                    position: e.root + turn * local,
                    rotation: turn * SimQuat::from_array(s.rotation),
                    scale: s.scale,
                    color: s.color,
                    shadow_plane_z: s.shadow_plane_z,
                    active: true,
                }
            })
            .collect();
        let collision = e.collision;
        let at_go = collision.and_then(|c| c.collided_with);
        let explosion = Explosion::go_projectile(&parts, e.root, collision.as_ref(), explosion_spec.sticks, explosion_spec.fade_wait_time, &mut rng.0, e.delta, &|id| {
            at_go.filter(|(a, _)| *a == id).map(|(_, anchor)| anchor)
        });
        // `ExplodeSpritesAndLines`: the projectile is hidden this frame (`LateUpdate`); its
        // explosion takes its place until `DespawnOnExplosionFinished`.
        commands.entity(e.projectile).try_despawn();
        commands.spawn((Name::new(format!("{} explosion", spec.name)), ProjectileExplosion { explosion, anchor: e.anchor }, crate::weapon::PoolMember(spec.guid.clone())));
    }
}

/// `Explosion.Update` for projectile explosions; `DespawnOnExplosionFinished` returns the
/// projectile to its pool.
fn tick_projectile_explosions(
    mut commands: Commands,
    clock: Res<FrameClock>,
    anchors: Anchors,
    mut explosions: Query<(Entity, &mut ProjectileExplosion, Option<&crate::weapon::PoolMember>)>,
    mut pools: ResMut<crate::weapon::ProjectilePools>,
) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut fx, member) in &mut explosions {
        let anchor = fx.anchor;
        let current = anchor.and_then(|a| anchors.resolve(a));
        let finished = fx.explosion.update_anchored(delta, &|id| current.filter(|_| anchor.is_some_and(|a| a.id() == id)));
        if finished {
            commands.entity(entity).try_despawn();
            if let Some(m) = member {
                pools.release(&m.0);
            }
        }
    }
}

fn queue_projectile_explosions(art: Option<Res<ManArt>>, light: Res<SceneLight>, explosions: Query<&ProjectileExplosion>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    for fx in &explosions {
        // No lines: skip the explosion's empty `LinesRenderer`.
        for geometry in fx.explosion.renderers(&art.art, light.for_shadows).into_iter().skip(1) {
            queue.0.push_frame(art.texture, geometry);
        }
    }
}

/// `LifetimeParticleSystem.LateUpdate` (copy the follower's position) and `OnDisable` (stop).
fn follow_projectiles(mut effects: Query<&mut ParticleFx>, projectiles: Query<&crate::weapon::Projectile>) {
    for mut fx in &mut effects {
        let Some((entity, offset)) = fx.follow else { continue };
        match projectiles.get(entity) {
            Ok(p) if p.follower_active() => {
                let turn = SimQuat::about_z(p.shot.rotation_degrees.to_radians());
                let o = turn * offset;
                fx.effect.position = SimVec3::new(p.shot.position.x + o.x, p.shot.position.y + o.y, p.spec.spawn_z + o.z);
            }
            _ => {
                fx.effect.system.stop();
                fx.follow = None;
            }
        }
    }
}

/// `ParticleSystemLines.LateUpdate` of every effect.
fn queue_particles(art: Option<Res<ManArt>>, light: Res<SceneLight>, effects: Query<&ParticleFx>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    for fx in &effects {
        let Some(drawing) = fx.effect.drawing(&fx.spec, light.for_shadows) else { continue };
        for geometry in drawing.geometry(art.art.line_uvs()) {
            if !geometry.indices.is_empty() {
                queue.0.push_frame(art.texture, geometry);
            }
        }
    }
}

/// The particle simulation, after the frame's `LateUpdate`.
fn simulate_particles(clock: Res<FrameClock>, collision: Option<Res<Collision>>, mut rng: ResMut<ParticleRng>, mut effects: Query<&mut ParticleFx>) {
    let delta = clock.stepper.frame().delta;
    let gravity = collision.map_or(unity_compat::Vec2::new(0.0, -9.81), |c| c.rules.gravity);
    for mut fx in &mut effects {
        let fx = &mut *fx;
        let at = fx.effect.position;
        fx.effect.system.update(&fx.spec.system, delta, at, gravity, &mut rng.0);
    }
}

/// `DespawnOnParticleSystemDeath.Update`.
fn despawn_dead_particle_systems(mut commands: Commands, effects: Query<(Entity, &ParticleFx)>) {
    for (entity, fx) in &effects {
        if !fx.effect.system.is_alive() {
            commands.entity(entity).try_despawn();
        }
    }
}

/// `LifetimeParticleSystem.OnEnable` for a projectile just spawned.
pub fn trail_for(effects: &EffectLibrary, spec: &ProjectileSpec, projectile: Entity, at: SimVec3) -> Option<ParticleFx> {
    let (guid, offset) = spec.lifetime_particles.as_ref()?;
    let fx = effects.0.get(guid)?;
    let offset = SimVec3::new(offset[0], offset[1], offset[2]);
    Some(ParticleFx { spec: fx.clone(), effect: ParticleEffect::spawn(at + offset), follow: Some((projectile, offset)) })
}

/// Whether a level prepools `guid` (its instances are prepared before play starts).
pub fn prepooled(level: &ActiveLevel, guid: &str) -> bool {
    level.0.prepooled.iter().any(|g| g == guid)
}
