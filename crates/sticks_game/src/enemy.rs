//! Computer-controlled characters.
//!
//! An enemy is the same `Man` as the player's avatar with a different controller: its
//! `CharacterInput` reads an `EnemyCharacterInput` ([`AiInput`]) that the enemy's behaviours
//! ([`AiBrain`]) fill every `Update`. `SetEnemyLayer` puts it on layer 9, `RemovePlayerOnlyParts`
//! takes away its `Target` (so it is neither targetable nor a camera influence), its respawner
//! and its control slot, and `DestroyRootOnRootExplosionFinished` removes it once its explosion
//! is over.
//!
//! The rules are in `sticks_sim::ai`; the enemies, their settings and their placement come from
//! the level scene (`unity_data::level::EnemySpawn`).
//!
//! `DeactivateOffScreen` deactivates enemies far outside the view and `OffScreenReactivator`
//! brings them back. A deactivated root is Bevy's [`Disabled`]: it drops out of every query, so
//! it neither moves, thinks, fires nor gets hit, exactly like an inactive `GameObject`.
//!
//! TODO(UNCERTAIN): `OnLoadedLevelEverywhere` (which arms `DeactivateOffScreen` and sets
//! `CharacterControl2D.readyForRPCs`, without which `SelectedWeapon` is ignored) is broadcast by
//! `LevelLoader.Update` on the first frame the asynchronous load reports done. It is taken to
//! arrive before the level objects' first `Update`.

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::ai::{Ballistics, Brain, Candidate, EMERGE_DONE_Z, EMERGE_HIDDEN_Z, EMERGE_LAYER, EmergeEvent, Emergence, OffScreenSleep, Targeter, fire_at_first_target};
use sticks_sim::camera::ViewBounds;
use sticks_sim::character::{Character, CharacterInput, DEFAULT_Z, StandingColliders};
use sticks_sim::life::{LifeCycle, LifeEvent};
use sticks_sim::visual::{CharacterPalette, random_face_color};
use sticks_sim::weapons::difficulty_modified;
use unity_compat::{Vec2 as SimVec2, Vec3 as SimVec3};

use crate::camera::{CAMERA_TARGET_HEIGHT, CameraPlaceholder, LevelCamera, Rig};
use crate::figure::{ManArt, Palette};
use crate::level::{ActiveLevel, CharacterAnimation, Collision};
use crate::life::{DeathRng, ExplosionWarmup, LifeNotice, Mortal};
use crate::player::{Avatar, ControlledBy, Controls, TriggerEntered};
use crate::state::{AppState, GameMode, Session};
use crate::weapon::{Armament, LauncherOverride, LauncherOverrides, ProjectileLibrary, Team};
use crate::FrameSet;

/// The team index enemies fight for (their body layer is `8 + 1`, their shots travel on the
/// team-1 projectile layers).
pub const ENEMY_TEAM: i32 = 1;

/// Marks a character as driven by an `EnemyCharacterInput`.
#[derive(Component)]
pub struct Enemy;

/// `EnemyCharacterInput`: what the behaviours decided, copied into [`Controls`] by the next
/// `CharacterInput.Update`.
#[derive(Component, Deref, DerefMut)]
pub struct AiInput(pub sticks_sim::ai::EnemyInput);

/// The enemy's behaviours and `Targeter`, run from the controller object's position.
#[derive(Component)]
pub struct AiBrain {
    pub brain: Brain,
    pub targeter: Targeter,
    /// Position of the controller object relative to the root (`(0, 1.8, 0)` for every enemy
    /// recovered so far). The root only ever turns about y, which leaves such offsets unchanged.
    pub origin_offset: SimVec3,
}

/// `Emerge` running on an enemy: its coroutine state, the activation trigger's object and the
/// root's layer to go back to.
#[derive(Component)]
pub struct Emerging {
    pub emergence: Emergence,
    pub trigger: Option<unity_data::scene::FileId>,
    pub original_layer: u8,
}

/// `CharacterControl2D.z` when it is not the default -0.1 (an emerging enemy waits at 0, behind
/// the scenery).
#[derive(Component, Clone, Copy, Debug)]
pub struct DrawZ(pub f32);

/// `TriggeredByAnyPlayer.Triggered` of every trigger object a player has entered.
#[derive(Resource, Default)]
pub struct AnyPlayerTriggers(pub std::collections::HashSet<unity_data::scene::FileId>);

/// `DeactivateOffScreen` on an active enemy.
#[derive(Component, Deref, DerefMut)]
pub struct OffScreenSleeper(pub OffScreenSleep);

/// A deactivated enemy waiting in `OffScreenReactivator`'s list: it wakes when `position` (where
/// it was deactivated) comes within `radius` of the view.
///
/// TODO(UNCERTAIN): Unity 5.1 resets an `Animator` to its default state when its object is
/// reactivated; the animator state is kept here.
#[derive(Component)]
pub struct Asleep {
    pub position: SimVec2,
    pub radius: f32,
}

pub struct EnemyPlugin;

impl Plugin for EnemyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AnyPlayerTriggers>()
            .add_systems(OnEnter(AppState::InGame), (|mut t: ResMut<AnyPlayerTriggers>| t.0.clear(), spawn_enemies.after(crate::figure::load_man_art)))
            .add_systems(Update, any_player_triggers.in_set(FrameSet::FixedSteps).after(crate::player::run_fixed_steps))
            .add_systems(Update, copy_enemy_input.in_set(FrameSet::RoleInput))
            // In a network game the host's enemies think and sleep; the other machines follow
            // their streams (`crate::net`).
            .add_systems(Update, (sleep_off_screen, think).chain().in_set(FrameSet::Characters).before(crate::player::update_characters).run_if(crate::net::decides))
            .add_systems(Update, wake_on_screen.in_set(FrameSet::Characters).after(crate::weapon::expire_projectiles).run_if(crate::net::decides))
            .add_systems(Update, despawn_finished_enemies.run_if(in_state(AppState::InGame)));
    }
}

/// Instantiate the level's enemies (scene objects `Awake` in file order).
///
/// TODO(UNCERTAIN): the order of `Awake` across scene objects decides which enemy draws which
/// random face colour; file-id order is used.
#[allow(clippy::too_many_arguments)]
fn spawn_enemies(
    mut commands: Commands,
    level: Res<ActiveLevel>,
    collision: Res<Collision>,
    animation: Res<CharacterAnimation>,
    session: Res<Session>,
    art: Res<ManArt>,
    mut rng: ResMut<DeathRng>,
) {
    // Only the development level `Platforms` gets the test target: Moon Race and other levels
    // without enemies are meant to be empty.
    if level.0.enemies.is_empty() && level.0.name == "Platforms" && !crate::settings::original_only() {
        spawn_target_dummy(&mut commands, &level, &collision, &animation);
        return;
    }
    let weapon_index = |name: &str| art.model().rig().weapons.iter().position(|w| w.name == name);
    let campaign = session.mode == GameMode::Campaign;
    // `DifficultyModifier.Awake` only modifies anything in campaign games.
    let modified = |value: f32, scaling: &unity_data::projectile::DifficultyScaling| {
        difficulty_modified(value, &scaling.amount, scaling.affected_by_player_count, session.player_count as i32, session.difficulty)
    };
    for spawn in &level.0.enemies {
        let position = SimVec2::new(spawn.position[0], spawn.position[1]);
        let mut character = Character::new(animation.0.clone(), &collision.rules, StandingColliders::MAN, position);
        // `SetEnemyLayer`.
        character.body.layer = (8 + ENEMY_TEAM) as u8;
        // `CharacterControl2D.facingRight` starts true whatever the scene rotation is.
        character.set_facing_right(true);

        // `HealthDifficultyModifier` (order -82) scales `SetStartingHealth` before its `Awake`
        // sets and resets the health.
        let mut life = LifeCycle::man().without_respawner();
        if let Some(mut health) = spawn.starting_health {
            if let (true, Some(scaling)) = (campaign, &spawn.health_scaling) {
                health = modified(health, scaling);
            }
            let _ = life.health.set_starting_health(health);
        }

        // `ProjectileLauncherFiringRateTimeDifficultyModifier` (order -81) scales every
        // `ProjectileLauncherModifier.firingRateTime` (even the -1 "keep" value, which then
        // becomes 0.01) before the modifiers apply themselves.
        let overrides = spawn
            .launcher_modifiers
            .iter()
            .filter_map(|m| {
                let Some(weapon) = weapon_index(&m.weapon) else {
                    warn!("{}: no weapon pivot named {:?}", spawn.name, m.weapon);
                    return None;
                };
                let mut firing_rate_time = m.firing_rate_time;
                if let (true, Some(scaling)) = (campaign, &spawn.firing_rate_scaling) {
                    firing_rate_time = modified(firing_rate_time, scaling).max(0.01);
                }
                Some(LauncherOverride { weapon, firing_rate_time, projectile: m.projectile.clone() })
            })
            .collect();

        // `CharacterColorSetter`: body and helmet from the scene, a random face.
        let prefab = art.prefab_palette();
        let palette = CharacterPalette {
            body: spawn.body_color.unwrap_or(prefab.body),
            helmet: spawn.helmet_color.unwrap_or(prefab.helmet),
            face: random_face_color(&mut rng.0),
        };

        // `Emerge.Start`: the enemy hides at z 0 on layer 25 with its fighting held back (a held
        // `Duck` is disabled, which clears the processor's duck).
        let emergence = spawn.emerge.as_ref().map(|e| (Emergence::of(e), e.trigger_object));
        let original_layer = character.body.layer;
        if emergence.is_some() {
            character.body.layer = EMERGE_LAYER;
        }
        let input = sticks_sim::ai::EnemyInput {
            duck: spawn.duck && !emergence.as_ref().is_some_and(|(e, _)| e.disables_duck),
            selected_weapon: spawn.selected_weapon.as_deref().and_then(weapon_index),
            ..Default::default()
        };
        let brain = AiBrain {
            brain: Brain::of_enemy(spawn),
            targeter: Targeter::of_enemy(spawn, |name| warn!("{}: target qualifier {name} is not implemented", spawn.name)),
            origin_offset: SimVec3::new(spawn.controller_offset[0], spawn.controller_offset[1], spawn.controller_offset[2]),
        };

        let mut entity = commands.spawn((
            Name::new(spawn.name.clone()),
            Enemy,
            Team(ENEMY_TEAM),
            Avatar(character),
            Controls(input.character_input()),
            AiInput(input),
            brain,
            LauncherOverrides(overrides),
            Palette(palette),
            Mortal(life),
            ExplosionWarmup(1),
            CameraPlaceholder::default(),
            Transform::from_xyz(position.x, position.y, -DEFAULT_Z),
        ));
        entity.insert(crate::level_end::SceneObject(spawn.object));
        if let Some(interval) = spawn.constant_explosion {
            entity.insert(crate::figure::ConstantExplosion::new(interval));
        }
        if let Some(period) = spawn.color_cycle {
            entity.insert(crate::figure::ColorCycler::new(period));
        }
        if let Some((emergence, trigger)) = emergence {
            entity.insert((Emerging { emergence, trigger, original_layer }, DrawZ(EMERGE_HIDDEN_Z)));
        }
        if spawn.registered {
            // `EnemyRegistrar.Start` → `NewEnemy`.
            entity.insert(crate::level_end::Registered);
        }
        if let Some(radius) = spawn.deactivate_off_screen {
            let mut sleep = OffScreenSleep::new(radius);
            sleep.on_loaded_level_everywhere();
            entity.insert(OffScreenSleeper(sleep));
        }
    }
    info!("spawned {} enemies", level.0.enemies.len());
}

/// Not part of the original: on the enemy-less development level `Platforms`, one idle
/// enemy `Man` stands near the second spawn point as a shootable target for testing weapons.
fn spawn_target_dummy(commands: &mut Commands, level: &ActiveLevel, collision: &Collision, animation: &CharacterAnimation) {
    let points = &level.0.spawn_points;
    let position = match (points.get(1), points.first()) {
        (Some(p), _) => SimVec2::new(p.position[0], p.position[1]),
        (None, Some(p)) => SimVec2::new(p.position[0] + 6.0, p.position[1]),
        (None, None) => return,
    };
    let mut character = Character::new(animation.0.clone(), &collision.rules, StandingColliders::MAN, position);
    character.body.layer = (8 + ENEMY_TEAM) as u8;
    character.set_facing_right(false);
    info!("level has no enemies: test target spawned at {position:?}");
    commands.spawn((
        Name::new("Enemy target (test)"),
        Enemy,
        Team(ENEMY_TEAM),
        Avatar(character),
        Controls(CharacterInput { aim: SimVec2::new(-1.0, 0.0), ..CharacterInput::default() }),
        Mortal(LifeCycle::man().without_respawner()),
        ExplosionWarmup(1),
        CameraPlaceholder::default(),
        Transform::from_xyz(position.x, position.y, -DEFAULT_Z),
    ));
}

/// `CharacterInput.Update` with an `EnemyCharacterInput` processor.
pub(crate) fn copy_enemy_input(mut enemies: Query<(&AiInput, &mut Controls)>) {
    for (input, mut controls) in &mut enemies {
        controls.0 = input.character_input();
    }
}

/// `LocalOffScreenReactivator.CameraBounds`: the camera as the previous `LateUpdate` left it.
fn view_bounds(windows: &Query<&Window, With<PrimaryWindow>>, rigs: &Query<&Rig, With<LevelCamera>>) -> Option<ViewBounds> {
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    rigs.single().ok().map(|rig| rig.view_bounds(aspect))
}

/// `DeactivateOffScreen.Update`: deactivate enemies that are too far outside the view.
fn sleep_off_screen(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LevelCamera>>,
    mut enemies: Query<(Entity, &Avatar, &Mortal, &AiBrain, &mut OffScreenSleeper)>,
) {
    let Some(view) = view_bounds(&windows, &rigs) else { return };
    for (entity, avatar, mortal, brain, mut sleeper) in &mut enemies {
        // An exploded root is inactive and runs no `Update`.
        if !mortal.avatar_active() {
            continue;
        }
        let p = avatar.position();
        let position = SimVec2::new(p.x + brain.origin_offset.x, p.y + brain.origin_offset.y);
        if sleeper.update(position, &view) {
            debug!("{entity:?} deactivated off screen at {position:?}");
            commands.entity(entity).insert((Disabled, Asleep { position, radius: sleeper.radius }));
        }
    }
}

/// `OffScreenReactivator.Update`: wake sleeping enemies whose spot is back near the view.
///
/// TODO(UNCERTAIN): whether a reactivated object's behaviours already `Update` in the frame it
/// was reactivated; here they start on the next frame (it runs after this frame's `Update`s).
/// TODO: `LevelComplete.Complete` stops all reactivation (level goals are not implemented).
fn wake_on_screen(
    mut commands: Commands,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LevelCamera>>,
    sleepers: Query<(Entity, &Asleep), With<Disabled>>,
) {
    let Some(view) = view_bounds(&windows, &rigs) else { return };
    for (entity, asleep) in &sleepers {
        if view.is_on_screen(asleep.position, asleep.radius) {
            debug!("{entity:?} reactivated at {:?}", asleep.position);
            commands.entity(entity).remove::<(Disabled, Asleep)>();
        }
    }
}

/// The enemies' `Update` behaviours: `Targeter.Targets` once per frame, then `FireAtFirstTarget`,
/// `MoveToFirstTarget` or `FireAndMoveIfNecessary` write into the processor.
///
/// TODO(UNCERTAIN): candidates are listed in query order; the original lists `Targetable`s in
/// `OnEnable` order, which only matters for targets at exactly equal distances.
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn think(
    mut commands: Commands,
    collision: Res<Collision>,
    library: Option<Res<ProjectileLibrary>>,
    clock: Res<crate::player::FrameClock>,
    triggers: Res<AnyPlayerTriggers>,
    players: Query<(&Avatar, &Mortal), With<ControlledBy>>,
    helicopters: Query<&crate::helicopter::Helicopter>,
    mut enemies: Query<(Entity, &mut Avatar, &Mortal, &AiBrain, &mut AiInput, Option<&Armament>, Option<&mut Emerging>), Without<ControlledBy>>,
) {
    // The "Target" of every active player helicopter (scene objects, registered first), then
    // the `Target` child of each active player avatar (`Targetable.OnEnable`/`OnDisable` follow
    // the avatar's activity: a piloting player's avatar is no target).
    let mut candidates: Vec<Candidate> = helicopters.iter().filter_map(crate::helicopter::Helicopter::target_candidate).collect();
    candidates.extend(players
        .iter()
        .filter(|(_, mortal)| mortal.avatar_active())
        .map(|(avatar, _)| {
            let p = avatar.position();
            Candidate { position: SimVec3::new(p.x, p.y + CAMERA_TARGET_HEIGHT, DEFAULT_Z), controlled: true }
        }));
    let gravity_y = collision.rules.gravity.y;
    let hit_triggers = collision.rules.raycasts_hit_triggers;
    let obstructed = |from: SimVec2, to: SimVec2, layers: u32| collision.world.linecast(from, to, layers, hit_triggers);
    let delta = clock.stepper.frame().delta;
    for (entity, mut avatar, mortal, brain, mut input, armament, mut emerging) in &mut enemies {
        if !mortal.avatar_active() {
            continue;
        }
        let p = avatar.position();
        let origin = SimVec3::new(p.x + brain.origin_offset.x, p.y + brain.origin_offset.y, DEFAULT_Z + brain.origin_offset.z);
        // `characterControl.WeaponPivot` → its launcher's prefab → `Projectile`.
        let projectile = armament.zip(library.as_ref()).and_then(|(armament, library)| {
            let weapon = armament.equipped()?;
            let spec = library.0.get(armament.projectile(weapon))?;
            Some(Ballistics { propelled: spec.propelled, gravity_scale: spec.gravity_scale, speed: spec.speed })
        });
        let targets = brain.targeter.targets(origin, &candidates, obstructed);
        // `Emerge` holds some behaviours back until it is done.
        let behaviours = emerging.as_ref().map_or(brain.brain, |e| e.emergence.restricted(&brain.brain));
        behaviours.think(&mut input.0, &targets, projectile, origin, gravity_y);
        // The `Emerge` coroutine, after the frame's `Update`s.
        if let Some(emerging) = emerging.as_mut() {
            let triggered = emerging.trigger.is_some_and(|t| triggers.0.contains(&t));
            let first = targets.first();
            let event = emerging.emergence.update(&mut input.0, triggered, origin.x, delta, |i| {
                fire_at_first_target(i, projectile, first, origin, gravity_y);
            });
            match event {
                // Out: `characterControl.z = -0.1` and the root's own layer again.
                EmergeEvent::Emerged => {
                    avatar.body.layer = emerging.original_layer;
                    debug_assert_eq!(EMERGE_DONE_Z, DEFAULT_Z);
                    commands.entity(entity).remove::<DrawZ>();
                }
                EmergeEvent::Completed => {
                    commands.entity(entity).remove::<Emerging>();
                }
                EmergeEvent::None => {}
            }
        }
    }
}

/// `TriggeredByAnyPlayer.OnTriggerEnter2D`: a player's character entered the trigger collider of
/// an object; it stays triggered.
fn any_player_triggers(level: Res<ActiveLevel>, mut entered: MessageReader<TriggerEntered>, players: Query<(), With<ControlledBy>>, mut triggers: ResMut<AnyPlayerTriggers>) {
    for e in entered.read() {
        if !players.contains(e.avatar) {
            continue;
        }
        if let Some(c) = level.0.colliders.get(e.collider as usize) {
            triggers.0.insert(c.object);
        }
    }
}

/// `DestroyRootOnRootExplosionFinished`: nothing is left once an enemy's explosion is over.
fn despawn_finished_enemies(mut commands: Commands, mut notices: MessageReader<LifeNotice>, enemies: Query<(), With<Enemy>>) {
    for notice in notices.read() {
        if matches!(notice.event, LifeEvent::ExplosionFinished) && enemies.contains(notice.avatar) {
            commands.entity(notice.avatar).try_despawn();
        }
    }
}
