//! Weapons: the loadout, the aimed rig pose, the trigger and the projectiles in flight.
//!
//! The rules live in `sticks_sim` (`weapons`: aim correction, launcher timing, shots; `aim`: the
//! posed rig). This module connects them to entities: every avatar carries an [`Armament`]
//! (which weapons it may use and each launcher's fire timing) and a [`Posed`] rig that the
//! figure is drawn from and the muzzle position is read from; every projectile in flight is an
//! entity with a [`Projectile`].

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use sticks_sim::aim::{AimedWeapon, PosedCharacter};
use sticks_sim::explosion::{Anchor, CollisionData};
use sticks_sim::weapons::{Hit, Launcher, Loadout, Shot, ShotTarget, corrected_aim, difficulty_modified};
use unity_compat::{Quat as SimQuat, Vec2 as SimVec2, Vec3 as SimVec3};
use sticks_sim::melee::{Swing, SwingTarget};
use unity_data::projectile::{ContactExplosion, MeleeSpec, ProjectileSpec};

use crate::effects::{AnchorRef, Anchors, EffectLibrary, FadeOutSounds, PlaySound, ProjectileExploded, avatar_anchor, prepooled, trail_for};
use crate::enemy::Enemy;
use crate::figure::ManArt;
use crate::level::{ActiveLevel, Collision};
use crate::life::{Damage, LifeNotice, Mortal, OnKill};
use crate::player::{Avatar, ControlledBy, Controls, FrameClock};
use crate::state::{AppState, GameMode, Session};
use crate::FrameSet;
use sticks_sim::life::LifeEvent;

/// `Player.TeamIndex` (players) or the enemy side: decides the projectile layer a character's
/// shots travel on and the layer its own body is on (`8 + team`).
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Team(pub i32);

/// The weapons an avatar may switch between and the fire timing of each.
#[derive(Component)]
pub struct Armament {
    loadout: Loadout,
    /// One launcher per rig weapon: each `ProjectileLauncher` keeps its own timing.
    launchers: Vec<Launcher>,
    /// `ProjectileLauncher.prefab` of each rig weapon (GUID; empty when it has none).
    projectiles: Vec<String>,
    /// `CharacterControl2D.correctedAim`.
    corrected_aim: SimVec2,
    /// The weapon whose firing animation plays (`firingAnimator.SetTrigger`) and its time.
    action: Option<(usize, f32)>,
}

impl Armament {
    /// `ProjectileLauncher.ChargeTime` of the equipped weapon.
    pub(crate) fn charge_time(&self) -> f32 {
        self.equipped().map_or(0.0, |w| self.launchers[w].charge_time())
    }

    /// The equipped weapon as an index into the rig's weapon list.
    pub fn equipped(&self) -> Option<usize> {
        self.loadout.current()
    }

    /// The rig weapons this avatar may use, in switching order.
    pub fn available(&self) -> &[usize] {
        self.loadout.weapons()
    }

    /// Development hook (not in the original): every rig weapon, whatever the level allows.
    pub(crate) fn unlock_all_weapons(&mut self) {
        let count = self.launchers.len();
        self.loadout.unlock_all(count);
    }

    /// Development hook (not in the original): the launchers, one per rig weapon.
    pub(crate) fn launchers_mut(&mut self) -> &mut [Launcher] {
        &mut self.launchers
    }

    /// `SetWeapon` from the owner of a networked character: equip `weapon` if the loadout holds
    /// it and something else is up.
    pub(crate) fn select_weapon(&mut self, weapon: usize) {
        if self.equipped() != Some(weapon) {
            self.loadout.select(weapon);
        }
    }

    /// GUID of the prefab rig weapon `weapon` launches.
    pub fn projectile(&self, weapon: usize) -> &str {
        &self.projectiles[weapon]
    }
}

/// `ProjectileLauncherModifier.Awake` on a character: replaces launcher settings before the
/// launchers start. Applied in order when the armament is built.
#[derive(Component, Default)]
pub struct LauncherOverrides(pub Vec<LauncherOverride>);

pub struct LauncherOverride {
    /// Rig weapon index.
    pub weapon: usize,
    /// When non-negative: the launcher's `firingRateTime`, and it becomes fully automatic.
    pub firing_rate_time: f32,
    /// Replacement `prefab` (GUID).
    pub projectile: Option<String>,
}

/// The avatar's rig for this frame (animation, facing and the aimed weapon).
#[derive(Component, Deref)]
pub struct Posed(pub PosedCharacter);

/// World position of the equipped weapon's pivot, which aim vectors are measured from. Seeded
/// from the previous frame's pose when the mouse is read.
#[derive(Component, Deref)]
pub struct AimPivot(pub SimVec2);

/// Projectile prefabs by GUID, loaded once with the character rig.
#[derive(Resource)]
pub struct ProjectileLibrary(pub HashMap<String, Arc<ProjectileSpec>>);

/// The melee effects rig weapons launch instead of a projectile (the Sword's "Sword Swish").
#[derive(Resource, Default)]
pub struct MeleeLibrary(pub HashMap<String, Arc<MeleeSpec>>);

/// A melee effect following its launcher (`sticks_sim::melee`).
#[derive(Component)]
pub struct MeleeSwing {
    pub swing: Swing,
    /// The avatar whose launch point it follows.
    pub owner: Entity,
    pub launch_point: usize,
    /// `DamageOtherOnCollision.responsiblePlayer`.
    pub responsible: Option<Entity>,
}

/// A projectile in flight.
#[derive(Component)]
pub struct Projectile {
    pub shot: Shot,
    pub spec: Arc<ProjectileSpec>,
    /// The player slot credited with the damage (`DamageOtherOnCollision.responsiblePlayer`);
    /// none for enemy fire.
    pub responsible: Option<Entity>,
    /// Frames it has been active (its `ExplodeSpritesAndLines` builds the explosion object one
    /// frame after `Start`).
    frames: u32,
    /// Its pooled instance was used before (or prepooled): `Start` ran long ago.
    reused: bool,
    /// After `Explodable.Explode`: on layer 28 (no more contacts); `DespawnOnExplode` removes it
    /// the next frame.
    exploded: Option<Exploded>,
    /// `TellOwnerToExplodeOnCollisionUnlessOtherIsDestroyed`: the contact waits for `LateUpdate`.
    pending: Option<PendingContact>,
}

/// The anchor id of a sword swing a projectile ended on (it stays where it was struck).
const SWING_COLLIDER: u32 = 0x7D00_0000;

/// `Seeker`: what a seeking projectile steers towards (the nearest `Targetable` when it was
/// spawned, followed for the rest of its flight) and how fast it may turn.
#[derive(Component, Clone, Copy, Debug)]
pub struct Seeking {
    target: SeekTarget,
    max_degrees_per_second: f32,
}

#[derive(Clone, Copy, Debug)]
enum SeekTarget {
    /// A player's `Man`: its "Target" child.
    Avatar(Entity),
    /// A player helicopter's "Target".
    Helicopter(Entity),
}

#[derive(Clone, Copy, Debug)]
struct Exploded {
    despawn_next_frame: bool,
    frame_seen: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingContact {
    anchor: AnchorRef,
    collided_with: Anchor,
    /// `collided`: set by the contact, cleared by the next `Update`; the projectile is held at
    /// the contact until `LateUpdate` decides.
    hold: bool,
}

impl Projectile {
    pub fn new(shot: Shot, spec: Arc<ProjectileSpec>, responsible: Option<Entity>) -> Projectile {
        Projectile { shot, spec, responsible, frames: 0, reused: false, exploded: None, pending: None }
    }

    /// `ExplodeSpritesAndLines.explosion != null`.
    /// TODO(UNCERTAIN): `Start` is taken to run in the frame after the spawn and the `Run`
    /// coroutine to resume one frame later; a projectile hitting something before that shows no
    /// explosion (only the first instances of a pool that no `Prepooler` warmed up).
    fn explosion_prepared(&self) -> bool {
        self.reused || self.frames >= 3
    }

    /// The object a `LifetimeParticleSystem` follows is still active.
    pub fn follower_active(&self) -> bool {
        self.exploded.is_none_or(|e| !e.despawn_next_frame || !e.frame_seen)
    }

    /// Drawn this frame (`ExplodeSpritesAndLines` hides it in the `LateUpdate` of its explosion).
    pub fn visible(&self) -> bool {
        self.exploded.is_none() || self.spec.explosion.is_none()
    }
}

/// `NetworkPool` per prefab: how many instances exist and how many are in use. A spawn takes
/// an existing instance when one is free (its `Start` has run long ago).
#[derive(Resource, Default)]
pub struct ProjectilePools(HashMap<String, (usize, usize)>);

impl ProjectilePools {
    /// `NetworkPool.Spawn`: true when an existing instance is reused.
    fn take(&mut self, guid: &str) -> bool {
        let (created, active) = self.0.entry(guid.to_owned()).or_default();
        let reused = *active < *created;
        if !reused {
            *created += 1;
        }
        *active += 1;
        reused
    }

    /// `NetworkPool.Despawn`.
    pub fn release(&mut self, guid: &str) {
        if let Some((_, active)) = self.0.get_mut(guid) {
            *active = active.saturating_sub(1);
        }
    }
}

/// The pool a projectile's instance belongs to, carried by whatever entity stands for the
/// instance until it is despawned (the projectile, then its explosion).
#[derive(Component)]
pub struct PoolMember(pub String);

pub struct WeaponPlugin;

impl Plugin for WeaponPlugin {
    fn build(&self, app: &mut App) {
        // `CharacterControl2D.Start` runs before the character's first `Update` (the enemy AI
        // reads the equipped weapon from its first frame on).
        app.init_resource::<ProjectilePools>()
        .init_resource::<MeleeLibrary>()
        .add_systems(OnEnter(AppState::InGame), |mut pools: ResMut<ProjectilePools>| pools.0.clear())
        .add_systems(Update, attach_armaments.in_set(FrameSet::RoleInput))
        .add_systems(
            Update,
            (despawn_exploded_projectiles, reload_on_respawn, aim_and_pose, switch_weapons, fire_weapons, start_projectiles, expire_projectiles, expire_swings, remember_rotations)
                .chain()
                .in_set(FrameSet::Characters)
                .after(crate::player::update_characters),
        )
        .add_systems(Update, resolve_pending_contacts.in_set(FrameSet::Present))
        .add_systems(Update, queue_aim_arcs.in_set(crate::unity_render::QueueSet::Collect).run_if(in_state(AppState::InGame)))
        .add_systems(
            Update,
            (step_swings, step_projectiles).chain().in_set(FrameSet::FixedSteps).after(crate::player::run_fixed_steps).before(crate::life::apply_damage),
        );
    }
}

/// `CharacterControl2D.Start`: keep the weapons the level allows (in rig order) and start with
/// the first.
#[allow(clippy::type_complexity)]
fn attach_armaments(
    mut commands: Commands,
    art: Option<Res<ManArt>>,
    library: Option<Res<ProjectileLibrary>>,
    level: Res<ActiveLevel>,
    new_avatars: Query<(Entity, &Avatar, Has<Enemy>, Option<&LauncherOverrides>), Without<Armament>>,
) {
    let (Some(art), Some(library)) = (art, library) else { return };
    let model = art.model();
    let weapons = &model.rig().weapons;
    let names: Vec<&str> = weapons.iter().map(|w| w.name.as_str()).collect();
    for (entity, avatar, for_enemy, overrides) in &new_avatars {
        let allowed = |name: &str| level.0.allowed_weapons.as_ref().is_none_or(|allowed| allowed.allows(name, for_enemy));
        // `Awake`: the prefab's launcher settings, then any `ProjectileLauncherModifier`.
        let mut settings: Vec<(f32, bool, String)> = weapons.iter().map(|w| (w.firing_rate_time, w.fully_automatic, w.projectile.clone())).collect();
        for o in overrides.map(|o| o.0.as_slice()).unwrap_or_default() {
            let s = &mut settings[o.weapon];
            if o.firing_rate_time >= 0.0 {
                s.0 = o.firing_rate_time;
                s.1 = true;
            }
            if let Some(projectile) = &o.projectile {
                s.2 = projectile.clone();
            }
        }
        // `Start` reads `Chargeable` from whichever prefab the launcher ended up with.
        let launchers: Vec<Launcher> = settings
            .iter()
            .map(|(rate, automatic, projectile)| {
                let chargeable = library.0.get(projectile).is_some_and(|spec| spec.chargeable);
                Launcher::new(*rate, *automatic, chargeable)
            })
            .collect();
        let projectiles = settings.into_iter().map(|(_, _, projectile)| projectile).collect();
        let pose = model.pose_character(&avatar.animator.pose(), avatar.position(), avatar.facing_right(), None);
        commands.entity(entity).insert((
            Armament { loadout: Loadout::new(&names, allowed), launchers, projectiles, corrected_aim: SimVec2::new(1.0, 0.0), action: None },
            AimPivot(avatar.position()),
            Posed(pose),
        ));
    }
}

/// `ProjectileLauncher.OnRespawned`: a trigger held through death does not fire on its own.
fn reload_on_respawn(mut notices: MessageReader<LifeNotice>, mut avatars: Query<&mut Armament>) {
    for notice in notices.read() {
        if !matches!(notice.event, LifeEvent::Respawned { .. }) {
            continue;
        }
        let Ok(mut armament) = avatars.get_mut(notice.avatar) else { continue };
        // `SendMessage` only reaches the active launcher.
        if let Some(weapon) = armament.equipped() {
            armament.launchers[weapon].on_respawned();
        }
    }
}

/// `CorrectAndSetAim` and `OnAnimatorIK`: pose the rig for this frame's animation and aim.
fn aim_and_pose(
    art: Option<Res<ManArt>>,
    clock: Res<FrameClock>,
    mut avatars: Query<(&Avatar, &Controls, &Mortal, &mut Armament, &mut Posed, &mut AimPivot, Option<&crate::enemy::DrawZ>)>,
) {
    let Some(art) = art else { return };
    let model = art.model();
    for (avatar, controls, mortal, mut armament, mut posed, mut pivot, draw_z) in &mut avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let face_right = avatar.facing_right();
        let aim = corrected_aim(controls.aim, armament.corrected_aim, face_right);
        armament.corrected_aim = aim;
        let equipped = armament.equipped();
        let weapon = equipped.map(|weapon| AimedWeapon { weapon, aim, face_right });
        // The weapon's firing animation runs once from the shot, then the pivot's transforms are
        // back at rest; switching weapons ends it.
        if let Some((w, t)) = armament.action {
            let t = t + clock.stepper.frame().delta;
            armament.action = art.action(w).filter(|clip| Some(w) == equipped && t <= clip.length).map(|_| (w, t));
        }
        let action = armament.action.and_then(|(w, t)| art.action(w).map(|clip| (clip, t)));
        posed.0 = model.pose_character_acting(&avatar.animator.pose(), avatar.position(), face_right, weapon, action);
        // `CharacterControl2D.z` (an emerging enemy waits behind the scenery at 0).
        if let Some(z) = draw_z {
            posed.0.placement.position.z = z.0;
        }
        if let Some(bone) = equipped.and_then(|weapon| model.rig().weapons[weapon].aim_pivot) {
            let p = posed.world_position(bone);
            pivot.0 = SimVec2::new(p.x, p.y);
        }
    }
}

/// `WeaponIndex++` / `WeaponIndex--` from the input, then `SelectedWeapon` (after the aim, as
/// `CharacterControl2D.Update` orders them, so the new weapon is posed from the next frame on).
///
/// `SelectedWeapon` is honoured once `readyForRPCs` is set by `OnLoadedLevelEverywhere`, which
/// is taken to arrive before the first frame's `Update` (TODO(UNCERTAIN), see `enemy`).
fn switch_weapons(mut avatars: Query<(&Controls, &Mortal, &mut Armament)>) {
    for (controls, mortal, mut armament) in &mut avatars {
        if !mortal.avatar_active() {
            continue;
        }
        if controls.next_weapon {
            armament.loadout.step(1);
        }
        if controls.previous_weapon {
            armament.loadout.step(-1);
        }
        if let Some(selected) = controls.selected_weapon {
            // Only when another weapon is up, and only if the loadout holds it.
            if armament.equipped() != Some(selected) {
                armament.loadout.select(selected);
            }
        }
    }
}

/// `ProjectileLauncher.Update`: spawn the projectile of every launcher whose trigger is due.
#[allow(clippy::too_many_arguments)]
fn fire_weapons(
    mut commands: Commands,
    clock: Res<FrameClock>,
    session: Res<Session>,
    collision: Res<Collision>,
    art: Option<Res<ManArt>>,
    library: Option<Res<ProjectileLibrary>>,
    melee: Res<MeleeLibrary>,
    mut avatars: Query<(Entity, &Controls, &Mortal, &mut Armament, &Posed, Option<&Team>, Option<&ControlledBy>)>,
    mut sounds: MessageWriter<PlaySound>,
) {
    let (Some(art), Some(library)) = (art, library) else { return };
    let weapons = &art.model().rig().weapons;
    let delta = clock.stepper.frame().delta;
    for (entity, controls, mortal, mut armament, posed, team, controlled_by) in &mut avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let Some(equipped) = armament.equipped() else { continue };
        let weapon = &weapons[equipped];
        // A melee weapon: the launcher spawns its effect at the launch point.
        if let (Some(spec), Some(launch_point)) = (melee.0.get(armament.projectile(equipped)), weapon.launch_point) {
            let Some(_launch) = armament.launchers[equipped].update(controls.fire, clock.time, delta) else { continue };
            armament.action = Some((equipped, 0.0));
            let muzzle = posed.world_position(launch_point);
            let damage = match &spec.damage_scaling {
                Some(scaling) if session.mode == GameMode::Campaign => {
                    difficulty_modified(spec.damage, &scaling.amount, scaling.affected_by_player_count, session.player_count as i32, session.difficulty)
                }
                _ => spec.damage,
            };
            let swing = Swing::new(spec, damage, SimVec2::new(muzzle.x, muzzle.y), controls.aim, team.map_or(0, |t| t.0));
            commands.spawn((Name::new(spec.name.clone()), MeleeSwing { swing, owner: entity, launch_point, responsible: controlled_by.map(|c| c.0) }));
            if !weapon.firing_sound.is_empty() {
                sounds.write(PlaySound { group: weapon.firing_sound.clone(), at: Some(SimVec3::new(muzzle.x, muzzle.y, muzzle.z)), owner: None });
            }
            continue;
        }
        let (Some(spec), Some(launch_point)) = (library.0.get(armament.projectile(equipped)), weapon.launch_point) else { continue };
        let Some(launch) = armament.launchers[equipped].update(controls.fire, clock.time, delta) else { continue };
        armament.action = Some((equipped, 0.0));
        let muzzle = posed.world_position(launch_point);
        let damage = match &spec.damage_scaling {
            Some(scaling) if session.mode == GameMode::Campaign => difficulty_modified(
                spec.damage,
                &scaling.amount,
                scaling.affected_by_player_count,
                session.player_count as i32,
                session.difficulty,
            ),
            _ => spec.damage,
        };
        let team = team.map_or(0, |t| t.0);
        // The launcher's fire direction is the aim relative to its pivot, which is exactly what
        // the input produced for the character's own aiming this frame.
        // `SetChargeTime(chargeTime)` reaches `SetProjectileSpeedByCharge` before `SetTeam`.
        let charge = spec.chargeable.then_some(launch.charge_time);
        let shot = Shot::launch_charged(spec, damage, SimVec2::new(muzzle.x, muzzle.y), controls.aim, launch.sub_frame_offset, team, collision.rules.gravity, charge);
        commands.spawn((Name::new(spec.name.clone()), Projectile::new(shot, spec.clone(), controlled_by.map(|c| c.0))));
        // `MasterAudio.PlaySound3DAtTransformAndForget(firingSound, transform)`.
        if !weapon.firing_sound.is_empty() {
            sounds.write(PlaySound { group: weapon.firing_sound.clone(), at: Some(SimVec3::new(muzzle.x, muzzle.y, muzzle.z)), owner: None });
        }
    }
}

/// What every new projectile does on `OnSpawned` / `OnEnable`: take a pool instance
/// (`NetworkPool.Spawn`), start its `LifetimeParticleSystem` trail and its `LifetimeSound`.
#[allow(clippy::too_many_arguments)]
fn start_projectiles(
    mut commands: Commands,
    level: Res<ActiveLevel>,
    effects: Res<EffectLibrary>,
    mut pools: ResMut<ProjectilePools>,
    mut new: Query<(Entity, &mut Projectile), Added<Projectile>>,
    mut sounds: MessageWriter<PlaySound>,
    players: Query<(Entity, &Avatar, &Mortal), With<ControlledBy>>,
    helicopters: Query<(Entity, &crate::helicopter::Helicopter)>,
) {
    for (entity, mut projectile) in &mut new {
        // `Seeker.OnSpawned`: the first target of its `Targeter` (`DistanceTargetSorter`, no
        // qualifiers): the nearest enabled `Targetable`, i.e. a player's or a player
        // helicopter's "Target".
        if let Some(max_degrees_per_second) = projectile.spec.seeker {
            let from = projectile.shot.position;
            let avatars = players.iter().filter(|(_, _, m)| m.avatar_active()).map(|(e, a, _)| {
                let p = a.position();
                (SeekTarget::Avatar(e), SimVec2::new(p.x, p.y + crate::camera::CAMERA_TARGET_HEIGHT))
            });
            let helis = helicopters.iter().filter_map(|(e, h)| h.target_candidate().map(|c| (SeekTarget::Helicopter(e), SimVec2::new(c.position.x, c.position.y))));
            // Helicopter targets registered first (scene objects), then the avatars; the first of
            // equal distances wins.
            let nearest = helis.chain(avatars).fold(None::<(SeekTarget, f32)>, |best, (target, at)| {
                let d = (at - from).sqr_magnitude();
                match best {
                    Some((_, b)) if b <= d => best,
                    _ => Some((target, d)),
                }
            });
            if let Some((target, _)) = nearest {
                commands.entity(entity).insert(Seeking { target, max_degrees_per_second });
            }
        }
        let reused = pools.take(&projectile.spec.guid);
        projectile.reused = reused || prepooled(&level, &projectile.spec.guid);
        commands.entity(entity).insert(PoolMember(projectile.spec.guid.clone()));
        let at = SimVec3::new(projectile.shot.position.x, projectile.shot.position.y, projectile.spec.spawn_z);
        if let Some(trail) = trail_for(&effects, &projectile.spec, entity, at) {
            commands.spawn((Name::new("Rocket Trail"), trail));
        }
        if let Some(sound) = &projectile.spec.lifetime_sound {
            sounds.write(PlaySound { group: sound.clone(), at: Some(at), owner: Some(entity) });
        }
    }
}

/// `DespawnOnExplode`: the frame after the explosion its coroutine returns it to the pool
/// (`LifetimeSound.OnDisable` fades its sound, `LifetimeParticleSystem.OnDisable` stops its
/// trail).
fn despawn_exploded_projectiles(
    mut commands: Commands,
    mut projectiles: Query<(Entity, &mut Projectile)>,
    mut pools: ResMut<ProjectilePools>,
    mut fades: MessageWriter<FadeOutSounds>,
) {
    for (entity, mut projectile) in &mut projectiles {
        let Some(exploded) = &mut projectile.exploded else {
            projectile.frames += 1;
            continue;
        };
        if !exploded.despawn_next_frame {
            continue;
        }
        if !exploded.frame_seen {
            exploded.frame_seen = true;
            continue;
        }
        commands.entity(entity).try_despawn();
        pools.release(&projectile.spec.guid);
        fades.write(FadeOutSounds { owner: entity });
    }
}

/// `TellOwnerToExplodeOnCollision.Update` before a contact: remember the rotation the
/// explosion will use.
fn remember_rotations(mut projectiles: Query<&mut Projectile>) {
    for mut projectile in &mut projectiles {
        if projectile.exploded.is_none() && !projectile.pending.is_some_and(|p| p.hold) {
            projectile.shot.frame_update();
        }
    }
}

fn explode(projectile: &mut Projectile, entity: Entity, collision: Option<CollisionData>, anchor: Option<AnchorRef>, delta: f32, out: &mut MessageWriter<ProjectileExploded>) {
    let p = projectile.shot.position;
    out.write(ProjectileExploded {
        spec: projectile.spec.clone(),
        projectile: entity,
        root: SimVec3::new(p.x, p.y, projectile.spec.spawn_z),
        rotation_degrees: projectile.shot.rotation_degrees,
        collision,
        anchor,
        prepared: projectile.spec.explosion.is_some() && projectile.explosion_prepared(),
        delta,
    });
    // Without a sprite explosion nothing else would ever remove it.
    let despawn_next_frame = projectile.spec.despawns_on_explode || projectile.spec.explosion.is_none();
    projectile.exploded = Some(Exploded { despawn_next_frame, frame_seen: false });
    projectile.pending = None;
}

fn collision_data(projectile: &Projectile, collided_with: Anchor, anchor: AnchorRef) -> (CollisionData, AnchorRef) {
    let p = projectile.shot.position;
    let data = CollisionData {
        point: SimVec3::new(p.x, p.y, projectile.spec.spawn_z),
        last_rotation: SimQuat::about_z(projectile.shot.rotation_at_last_update.to_radians()),
        collided_with: Some((anchor.id(), collided_with)),
    };
    (data, anchor)
}

/// `TellOwnerToExplodeOnCollisionUnlessOtherIsDestroyed.LateUpdate`: explode at the current
/// position unless what was hit has exploded or gone since.
fn resolve_pending_contacts(
    clock: Res<FrameClock>,
    anchors: Anchors,
    mut projectiles: Query<(Entity, &mut Projectile)>,
    mut exploded: MessageWriter<ProjectileExploded>,
) {
    for (entity, mut projectile) in &mut projectiles {
        let Some(pending) = projectile.pending else { continue };
        if anchors.is_destroyed(pending.anchor) {
            // `collidedWith` stays set and is checked again next `LateUpdate`; the projectile
            // flies on.
            projectile.pending = Some(PendingContact { hold: false, ..pending });
            continue;
        }
        let (data, anchor) = collision_data(&projectile, pending.collided_with, pending.anchor);
        explode(&mut projectile, entity, Some(data), Some(anchor), clock.stepper.frame().delta, &mut exploded);
    }
}

/// `SpawnExpiration`: projectiles age with the frame time; when the time runs out
/// `DespawnOnSpawnExpiration` removes it or `ExplodeOnSpawnExpiration` explodes it (no
/// collision data). An exploded projectile is inactive and no longer ages.
pub(crate) fn expire_projectiles(
    mut commands: Commands,
    clock: Res<FrameClock>,
    mut projectiles: Query<(Entity, &mut Projectile)>,
    mut pools: ResMut<ProjectilePools>,
    mut exploded: MessageWriter<ProjectileExploded>,
    mut fades: MessageWriter<FadeOutSounds>,
) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut projectile) in &mut projectiles {
        if projectile.exploded.is_some() || !projectile.shot.age_by(delta) {
            continue;
        }
        if projectile.spec.explodes_on_expiration {
            explode(&mut projectile, entity, None, None, delta, &mut exploded);
        } else {
            commands.entity(entity).try_despawn();
            pools.release(&projectile.spec.guid);
            fades.write(FadeOutSounds { owner: entity });
        }
    }
}

/// `Projectile.FixedUpdate` plus the physics contact: every projectile advances once per fixed
/// step of the frame. On a new contact (`OnCollisionEnter2D`) a body with `Health` takes the
/// projectile's damage (`DamageOtherOnCollision`, the first component), then
/// `TellOwnerToExplodeOnCollision` explodes it at once (and puts it on layer 28: it coasts on,
/// touching nothing), or `...UnlessOtherIsDestroyed` keeps the contact for `LateUpdate`.
///
/// TODO(UNCERTAIN): projectiles move after all of the frame's character steps rather than
/// interleaved with them; identical while exactly one step runs per frame. A projectile whose
/// contact did not end it is held at the contact for the rest of the frame (Box2D's contact
/// solver would stop or deflect it; not modelled).
#[allow(clippy::too_many_arguments)]
pub(crate) fn step_projectiles(
    clock: Res<FrameClock>,
    collision: Res<Collision>,
    mut projectiles: Query<(Entity, &mut Projectile, Option<&Seeking>)>,
    bodies: Query<(Entity, &Avatar, &Mortal, Option<&Posed>)>,
    mut damage: MessageWriter<Damage>,
    mut level_hits: MessageWriter<crate::letters::LevelColliderHit>,
    helicopters: Query<(Entity, &crate::helicopter::Helicopter)>,
    mut helicopter_hits: MessageWriter<crate::helicopter::HelicopterHit>,
    cars: Query<(Entity, &crate::cars::Car)>,
    mut car_hits: MessageWriter<crate::cars::CarHit>,
    mut exploded: MessageWriter<ProjectileExploded>,
    swings: Query<&MeleeSwing>,
) {
    if projectiles.is_empty() || clock.steps.is_empty() {
        return;
    }
    let active: Vec<(Entity, &Avatar, Option<&Posed>)> = bodies.iter().filter(|(_, _, mortal, _)| mortal.avatar_active()).map(|(e, a, _, p)| (e, a, p)).collect();
    let mut targets: Vec<ShotTarget<'_>> = active.iter().map(|(_, avatar, _)| ShotTarget { position: avatar.position(), shape: &avatar.body.shape, layer: avatar.body.layer }).collect();
    // Helicopter hulls (rotated pieces relative to the body) come after the characters.
    let helis: Vec<(Entity, &crate::helicopter::Helicopter)> = helicopters.iter().filter(|(_, h)| h.is_target()).collect();
    targets.extend(helis.iter().map(|(_, h)| ShotTarget { position: h.body.position, shape: &h.shape, layer: h.layer }));
    // Then the cars' chassis.
    let car_list: Vec<(Entity, &crate::cars::Car)> = cars.iter().filter(|(_, c)| c.is_target()).collect();
    targets.extend(car_list.iter().map(|(_, c)| ShotTarget { position: c.position(), shape: &c.shape, layer: c.layer }));
    for (entity, mut projectile, seeking) in &mut projectiles {
        for &dt in &clock.steps {
            if projectile.exploded.is_some() {
                projectile.shot.coast(collision.rules.gravity, dt);
                continue;
            }
            // `Seeker.FixedUpdate`, while its target is active.
            if let Some(seeking) = seeking {
                let target = match seeking.target {
                    SeekTarget::Avatar(e) => bodies.get(e).ok().filter(|(_, _, m, _)| m.avatar_active()).map(|(_, a, _, _)| {
                        let p = a.position();
                        SimVec2::new(p.x, p.y + crate::camera::CAMERA_TARGET_HEIGHT)
                    }),
                    SeekTarget::Helicopter(e) => helicopters.get(e).ok().and_then(|(_, h)| h.target_candidate()).map(|c| SimVec2::new(c.position.x, c.position.y)),
                };
                if let Some(target) = target {
                    projectile.shot.seek(target, seeking.max_degrees_per_second, dt);
                }
            }
            if projectile.pending.is_some_and(|p| p.hold) {
                continue;
            }
            // A sword's deflector: the projectile bounces off and changes sides instead of
            // exploding.
            let mut deflected = false;
            for swing in &swings {
                if swing.swing.deflect(&mut projectile.shot, &collision.rules) {
                    deflected = true;
                    if swing.responsible.is_some() {
                        projectile.responsible = swing.responsible;
                    }
                }
            }
            // Meeting the swing's body instead (not its deflector) ends the projectile there: its
            // pieces drift and fade in mid-air (`TellOwnerToExplodeOnCollision`).
            if !deflected && projectile.spec.contact != ContactExplosion::Never && swings.iter().any(|s| s.swing.strikes(&projectile.shot, &collision.rules)) {
                projectile.shot.contact_point = Some(projectile.shot.position);
                let (data, anchor) = collision_data(&projectile, Anchor::IDENTITY, AnchorRef::Collider(SWING_COLLIDER));
                explode(&mut projectile, entity, Some(data), Some(anchor), dt, &mut exploded);
                continue;
            }
            let Some(hit) = projectile.shot.fixed_step(&collision.world, &collision.rules, &targets, dt) else { continue };
            let amount = projectile.shot.damage;
            let (anchor, collided_with) = match hit {
                Hit::Target(index) if index >= active.len() + helis.len() => {
                    let (car, c) = car_list[index - active.len() - helis.len()];
                    car_hits.write(crate::cars::CarHit { car, amount });
                    // TODO(UNCERTAIN): the parts of a projectile exploding on a car stay where
                    // it hit instead of riding on the car.
                    let _ = c;
                    (AnchorRef::Collider(crate::cars::CAR_COLLIDER_BASE), Anchor::IDENTITY)
                }
                Hit::Target(index) if index >= active.len() => {
                    let (heli, h) = helis[index - active.len()];
                    helicopter_hits.write(crate::helicopter::HelicopterHit { helicopter: heli, amount });
                    (AnchorRef::Helicopter(heli), h.anchor())
                }
                Hit::Target(index) => {
                    let (target, avatar, posed) = active[index];
                    damage.write(Damage { target, amount, on_kill: OnKill::Nothing, responsible: projectile.responsible });
                    (AnchorRef::Avatar(target), avatar_anchor(avatar, posed))
                }
                // Whatever it touched takes the damage if it has a `Health` (the letters of an
                // `ExplodableText`).
                Hit::Level(collider) => {
                    level_hits.write(crate::letters::LevelColliderHit { collider, amount });
                    (AnchorRef::Collider(collider), Anchor::IDENTITY)
                }
            };
            match projectile.spec.contact {
                ContactExplosion::FirstContact => {
                    let (data, anchor) = collision_data(&projectile, collided_with, anchor);
                    // Inside the physics callback `Time.deltaTime` is the fixed step.
                    explode(&mut projectile, entity, Some(data), Some(anchor), dt, &mut exploded);
                }
                ContactExplosion::UnlessOtherIsDestroyed => projectile.pending = Some(PendingContact { anchor, collided_with, hold: true }),
                ContactExplosion::Never => {}
            }
        }
    }
}

/// `FollowProjectileLauncher.FixedUpdate` and the effect's `OnCollisionEnter2D`s: every physics
/// step the swing moves to its launcher's launch point and damages each body it starts touching
/// (characters and helicopters on layers its layer collides with).
pub(crate) fn step_swings(
    clock: Res<FrameClock>,
    collision: Res<Collision>,
    mut swings: Query<&mut MeleeSwing>,
    bodies: Query<(Entity, &Avatar, &Mortal, Option<&Posed>)>,
    helicopters: Query<(Entity, &crate::helicopter::Helicopter)>,
    mut damage: MessageWriter<Damage>,
    mut helicopter_hits: MessageWriter<crate::helicopter::HelicopterHit>,
) {
    if swings.is_empty() || clock.steps.is_empty() {
        return;
    }
    let active: Vec<(Entity, &Avatar)> = bodies.iter().filter(|(_, _, mortal, _)| mortal.avatar_active()).map(|(e, a, _, _)| (e, a)).collect();
    let mut targets: Vec<SwingTarget<'_>> = active.iter().map(|(e, a)| SwingTarget { key: e.to_bits(), position: a.position(), shape: &a.body.shape, layer: a.body.layer }).collect();
    let helis: Vec<(Entity, &crate::helicopter::Helicopter)> = helicopters.iter().filter(|(_, h)| h.is_target()).collect();
    targets.extend(helis.iter().map(|(e, h)| SwingTarget { key: e.to_bits(), position: h.body.position, shape: &h.shape, layer: h.layer }));
    for mut swing in &mut swings {
        let to = bodies.get(swing.owner).ok().and_then(|(_, _, _, posed)| posed).map(|p| p.world_position(swing.launch_point));
        for &dt in &clock.steps {
            let at = to.map_or(swing.swing.position, |p| SimVec2::new(p.x, p.y));
            let amount = swing.swing.damage;
            for key in swing.swing.fixed_step(at, dt, &targets, &collision.rules) {
                if let Some(&(heli, _)) = helis.iter().find(|(e, _)| e.to_bits() == key) {
                    helicopter_hits.write(crate::helicopter::HelicopterHit { helicopter: heli, amount });
                } else if let Some(&(target, _)) = active.iter().find(|(e, _)| e.to_bits() == key) {
                    damage.write(Damage { target, amount, on_kill: OnKill::Nothing, responsible: swing.responsible });
                }
            }
        }
    }
}

/// `SpawnExpiration.Update` → `DespawnOnSpawnExpiration`.
fn expire_swings(mut commands: Commands, clock: Res<FrameClock>, mut swings: Query<(Entity, &mut MeleeSwing)>) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut swing) in &mut swings {
        if swing.swing.update(delta) {
            commands.entity(entity).try_despawn();
        }
    }
}

/// `AimVisualizer` (the Spear's launcher): while the throw charges, the arc the spear would fly is
/// drawn as white dashes (alpha 0.576, width 0.18) with shadows on the plane -0.22, from the
/// launch point along `FireDirection` at the charged speed under the projectile's gravity. Kept
/// only up to its `MaximumDifficultyLevelComponents` difficulty.
#[allow(clippy::too_many_arguments)]
fn queue_aim_arcs(
    art: Option<Res<ManArt>>,
    library: Option<Res<ProjectileLibrary>>,
    collision: Res<Collision>,
    session: Res<Session>,
    light: Res<crate::unity_render::SceneLight>,
    avatars: Query<(&Controls, &Mortal, &Armament, &Posed)>,
    mut queue: ResMut<crate::unity_render::FrameQueue>,
) {
    let (Some(art), Some(library)) = (art, library) else { return };
    let weapons = &art.model().rig().weapons;
    for (controls, mortal, armament, posed) in &avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let Some(equipped) = armament.equipped() else { continue };
        let weapon = &weapons[equipped];
        let (Some(cap), Some(launch_point)) = (weapon.aim_visualizer, weapon.launch_point) else { continue };
        if session.difficulty > cap {
            continue;
        }
        let charge = armament.charge_time();
        if charge == 0.0 {
            continue;
        }
        let Some(spec) = library.0.get(armament.projectile(equipped)) else { continue };
        let speed = spec.speed * spec.speed_by_charge.as_ref().map_or(1.0, |s| s.multiplier(charge));
        let aim = controls.0.aim;
        let length = aim.magnitude();
        if length == 0.0 {
            continue;
        }
        let start_velocity = aim * (speed / length);
        let gravity = collision.rules.gravity * spec.gravity_scale;
        let origin = posed.world_position(launch_point);
        let o = SimVec2::new(origin.x, origin.y);
        let dashes = sticks_sim::weapons::aim_arc(o, start_velocity, gravity, |a, b| collision.world.linecast(a, b, 1, collision.rules.raycasts_hit_triggers));
        let color = [255u8, 255, 255, (0.576f32 * 255.0) as u8];
        let lines: Vec<sticks_sim::visual::StickLine> = dashes
            .iter()
            .map(|(a, b)| sticks_sim::visual::StickLine {
                a: SimVec3::new(o.x + a.x, o.y + a.y, origin.z),
                b: SimVec3::new(o.x + b.x, o.y + b.y, origin.z),
                width: 0.18,
                color,
                shadow_plane_z: -0.22,
            })
            .collect();
        let g = crate::helicopter::lines_geometry(&lines, origin, light.for_shadows, art.art.line_uvs());
        if !g.is_empty() {
            queue.0.push_frame(art.texture, g);
        }
    }
}
