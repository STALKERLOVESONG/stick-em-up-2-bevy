//! How a campaign level ends (see `sticks_sim::level_end` for the rules):
//!
//! * The "Level Ender" objects of the scene (`unity_data::level::LevelEnder`): an
//!   `ActivateSomethingOnTriggered` + `TriggeredByAllPlayers` on the "Marker" (every live player
//!   inside its trigger circle), a `DestroyAllEnemiesLevelEnder` (every registered enemy
//!   exhausted) or a `DestroyParticularEnemyLevelEnder` (one enemy exhausted) switches the
//!   "Ender" object on.
//! * The Ender's `Start`s, in component order: `EndOfLevelUIActivator` (1 s of scaled time, then
//!   the level time, then `CloseAllMenusImmediately` and the "End of Level" screen),
//!   `EndOfLevelCameraMover` (no edge margin, no minimum width, minimum distance 14),
//!   `EndOfLevelTimeSlower` (`timeScale` → 0.4; it keeps running in `Update` with unscaled
//!   time, whatever else sets the scale), `DisableGameOvering`, `LevelComplete.Complete`
//!   (`LevelTimer.LevelTime`, `EndOfLevelEnemyDestroyer` kills every registered, active enemy),
//!   `ExplodeSomethingOnStart` (the marker's lines fly apart) and the `EventSounds` "Applause".
//! * `GameOverUIActivator` (on the Level Ender): once every active player has run out of lives,
//!   `GameOver()`; disabled again by `DisableGameOvering`.
//! * `MaximumDamageCalculator`: one frame after `Start`, the starting health of every awake
//!   `Health` without a `ControlSlot` (the enemies and the enemy helicopters).
//! * The marker's `PolygonColliderOutlineLines` (white lines with shadows) until it explodes.
//!
//! TODO: `TimeLimit` and the "Timing UI" (Moon Race), the "Enemies Remaining UI" countdown text
//! (a level text on the UI layer, Office), `TriggeredByAllPlayers` for players inside a
//! helicopter (the hull's colliders entering the trigger), `DamagingDoesNotAwardScore`.
//! TODO(UNCERTAIN): an avatar that is deactivated inside the trigger is taken to send no
//! `OnTriggerExit2D` (it stays counted as inside).

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use sticks_sim::explosion::Explosion;
use sticks_sim::level_end::{AllPlayersTrigger, END_OF_LEVEL_UI_DELAY, EnemyCount, PlayerResult, TimeSlower};
use sticks_sim::life::{LifeEvent, PLAYER_STARTING_LIVES};
use sticks_sim::visual::{CharacterVisual, outline_visual};
use unity_compat::Vec3 as SimVec3;
use unity_data::level::{EnderActivator, EnderMarker, LevelEnder};
use unity_data::scene::FileId;

use crate::camera::{LevelCamera, Rig};
use crate::effects::PlaySound;
use crate::enemy::Enemy;
use crate::figure::{ManArt, Palette};
use crate::hud::{HudLayout, HudScene};
use crate::ingame_ui::{EndOfLevelReport, InGameUi};
use crate::level::ActiveLevel;
use crate::life::{Damage, DeathRng, LifeNotice, Mortal, OnKill, PlayerLives, PlayerScore};
use crate::lobby::LobbyMemory;
use crate::menu::TimeScale;
use crate::player::{ControlledBy, FrameClock, Player, TriggerEntered, TriggerExited};
use crate::state::{AppState, Session};
use crate::unity_render::{FrameQueue, QueueSet, SceneLight};
use crate::FrameSet;

/// The root object of a scene enemy (what `DestroyParticularEnemyLevelEnder.enemy` names).
#[derive(Component, Clone, Copy, Debug)]
pub struct SceneObject(pub FileId);

/// `EnemyRegistrar` announced this enemy (`NewEnemy`).
#[derive(Component, Clone, Copy, Debug)]
pub struct Registered;

/// Entities this module spawns (removed with the game).
#[derive(Component)]
pub struct LevelEndEntity;

struct MarkerState {
    spec: EnderMarker,
    active: bool,
    explosion: Option<Explosion>,
}

struct EnderState {
    spec: LevelEnder,
    /// `TriggeredByAllPlayers` per `ActivateSomethingOnTriggered` (its object), if any.
    triggers: Vec<(FileId, AllPlayersTrigger)>,
    /// `DestroyAllEnemiesLevelEnder`.
    count: Option<EnemyCount>,
    /// `DestroyParticularEnemyLevelEnder.enemy`.
    particular: Option<FileId>,
    /// The Ender is active.
    activated: bool,
    /// `GameOverUIActivator.enabled`.
    game_over_enabled: bool,
    markers: Vec<MarkerState>,
}

#[derive(Resource)]
struct LevelEnd {
    enders: Vec<EnderState>,
    /// `LevelTimer.startTime` (`Time.time` at `OnLoadedLevelEverywhere`).
    start_time: f32,
    /// `LevelTimer.LevelTime`.
    level_time: Option<f32>,
    /// `EndOfLevelTimeSlower` of an active Ender.
    time_slower: Option<TimeSlower>,
    /// `EndOfLevelUIActivator.Start`: scaled seconds left of `WaitForSeconds(1)`.
    ui_wait: Option<f32>,
    /// `MaximumDamageCalculator.MaximumDamage` (computed on the second frame).
    maximum_damage: Option<f32>,
    frames: u32,
}

pub struct LevelEndPlugin;

impl Plugin for LevelEndPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), setup_level_end)
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<LevelEnd>())
            .add_systems(
                Update,
                (level_end_events, tick_marker_explosions).chain().in_set(FrameSet::Characters).after(crate::life::tick_lives).run_if(resource_exists::<LevelEnd>),
            )
            .add_systems(
                Update,
                unscaled_level_end
                    .after(HudLayout)
                    .after(crate::ingame_ui::UiSet)
                    .before(QueueSet::Draw)
                    .run_if(in_state(AppState::InGame))
                    .run_if(resource_exists::<LevelEnd>),
            )
            .add_systems(Update, queue_markers.in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)).run_if(resource_exists::<LevelEnd>));
    }
}

fn setup_level_end(mut commands: Commands, level: Res<ActiveLevel>, clock: Res<FrameClock>) {
    let registered = level.0.enemies.iter().filter(|e| e.registered).count();
    let enders: Vec<EnderState> = level
        .0
        .enders
        .iter()
        .map(|spec| {
            let mut triggers = Vec::new();
            let mut count = None;
            let mut particular = None;
            for activator in &spec.activators {
                match activator {
                    EnderActivator::AllPlayersTrigger { trigger: Some(t) } => triggers.push((*t, AllPlayersTrigger::default())),
                    EnderActivator::AllPlayersTrigger { trigger: None } => {}
                    EnderActivator::DestroyAllEnemies { .. } => {
                        // `Awake` subscribes before any `EnemyRegistrar.Start` announces itself.
                        let mut c = EnemyCount::default();
                        for _ in 0..registered {
                            c.registered();
                        }
                        count = Some(c);
                    }
                    EnderActivator::DestroyParticularEnemy { enemy } => particular = *enemy,
                }
            }
            EnderState {
                triggers,
                count,
                particular,
                activated: false,
                game_over_enabled: spec.game_over_activator,
                markers: spec.markers.iter().map(|m| MarkerState { active: m.active, spec: m.clone(), explosion: None }).collect(),
                spec: spec.clone(),
            }
        })
        .collect();
    if enders.is_empty() {
        info!("level {} has no level ender", level.0.name);
    } else {
        info!("level {}: {} level ender(s), {} registered enemies", level.0.name, enders.len(), registered);
    }
    commands.insert_resource(LevelEnd { enders, start_time: clock.time, level_time: None, time_slower: None, ui_wait: None, maximum_damage: None, frames: 0 });
}

/// The player index controlling an avatar.
fn player_of(avatar: Entity, controlled: &Query<&ControlledBy>, players: &Query<(&Player, Option<&PlayerLives>)>) -> Option<usize> {
    let slot = controlled.get(avatar).ok()?.0;
    players.get(slot).ok().map(|(p, _)| p.index)
}

/// Trigger enters/exits, game overs and exhausted enemies → the activators; an activation runs
/// the Ender's `Start`s.
#[allow(clippy::too_many_arguments)]
fn level_end_events(
    mut end: ResMut<LevelEnd>,
    level: Res<ActiveLevel>,
    clock: Res<FrameClock>,
    mut entered: MessageReader<TriggerEntered>,
    mut exited: MessageReader<TriggerExited>,
    mut notices: MessageReader<LifeNotice>,
    controlled: Query<&ControlledBy>,
    players: Query<(&Player, Option<&PlayerLives>)>,
    enemies: Query<(Option<&SceneObject>, Has<Registered>), With<Enemy>>,
    active_enemies: Query<(Entity, &Mortal), (With<Enemy>, With<Registered>)>,
    mut rigs: Query<&mut Rig, With<LevelCamera>>,
    mut damage: MessageWriter<Damage>,
    mut sounds: MessageWriter<PlaySound>,
    mut rng: ResMut<DeathRng>,
    light: Res<SceneLight>,
    mut net: ResMut<crate::net::Net>,
) {
    let end = &mut *end;
    // In a network game the host decides when a level is complete
    // (`DestroyAllEnemiesLevelEnder`: `Offline || isServer`, then `RPC("Activate")`); clients
    // activate the Ender it names.
    let decides = net.decides();
    for (object, _) in net.take_level_events("LevelEnder") {
        let i = object as usize;
        if i < end.enders.len() && !end.enders[i].activated {
            let started = Started { level_time: &mut end.level_time, start_time: end.start_time, time_slower: &mut end.time_slower, ui_wait: &mut end.ui_wait };
            activate(&mut end.enders[i], started, &active_enemies, &mut rigs, &mut damage, &mut sounds, &mut rng, &light, &clock);
        }
    }
    // `Player.ActivePlayers` with `Lives.Remaining >= 0` (outside the campaign nobody has lives).
    let live: Vec<usize> = players.iter().filter(|(_, lives)| lives.is_none_or(|l| l.remaining() >= 0)).map(|(p, _)| p.index).collect();
    let trigger_object = |collider: u32| level.0.colliders.get(collider as usize).filter(|c| c.trigger).map(|c| c.object);
    for e in entered.read() {
        let (Some(object), Some(player)) = (trigger_object(e.collider), player_of(e.avatar, &controlled, &players)) else { continue };
        for ender in &mut end.enders {
            for (t, trigger) in &mut ender.triggers {
                if *t == object {
                    trigger.enter(&[player], &live);
                }
            }
        }
    }
    for e in exited.read() {
        let (Some(object), Some(player)) = (trigger_object(e.collider), player_of(e.avatar, &controlled, &players)) else { continue };
        for ender in &mut end.enders {
            for (t, trigger) in &mut ender.triggers {
                if *t == object {
                    trigger.exit(&[player]);
                }
            }
        }
    }
    for notice in notices.read() {
        match notice.event {
            // `Respawner.PlayerGotGameOver` → `CheckIfAllLivePlayersHaveTriggered`.
            LifeEvent::GameOver => {
                for ender in &mut end.enders {
                    for (_, trigger) in &mut ender.triggers {
                        trigger.check(&live);
                    }
                }
            }
            LifeEvent::Exhausted => {
                let Ok((object, registered)) = enemies.get(notice.avatar) else { continue };
                for (index, ender) in end.enders.iter_mut().enumerate() {
                    if registered {
                        if let Some(count) = ender.count.as_mut() {
                            count.exhausted();
                        }
                    }
                    if decides && ender.particular.is_some() && object.map(|o| o.0) == ender.particular && !ender.activated {
                        net.send_level_event("LevelEnder", index as u64, Vec::new());
                        let started = Started { level_time: &mut end.level_time, start_time: end.start_time, time_slower: &mut end.time_slower, ui_wait: &mut end.ui_wait };
                        activate(ender, started, &active_enemies, &mut rigs, &mut damage, &mut sounds, &mut rng, &light, &clock);
                    }
                }
            }
            _ => {}
        }
    }
    // `ActivateSomethingOnTriggered.Update` / `DestroyAllEnemiesLevelEnder.Update`.
    for i in 0..end.enders.len() {
        let ender = &end.enders[i];
        let fire = decides && !ender.activated && (ender.triggers.iter().any(|(_, t)| t.triggered()) || ender.count.is_some_and(|c| c.all_gone()));
        if fire {
            net.send_level_event("LevelEnder", i as u64, Vec::new());
            let started = Started { level_time: &mut end.level_time, start_time: end.start_time, time_slower: &mut end.time_slower, ui_wait: &mut end.ui_wait };
            activate(&mut end.enders[i], started, &active_enemies, &mut rigs, &mut damage, &mut sounds, &mut rng, &light, &clock);
        }
    }
}

/// The level-wide state an Ender's `Start`s change.
struct Started<'a> {
    /// `LevelTimer.LevelTime` and its `startTime`.
    level_time: &'a mut Option<f32>,
    start_time: f32,
    time_slower: &'a mut Option<TimeSlower>,
    ui_wait: &'a mut Option<f32>,
}

/// `toActivate.SetActive(true)`: the Ender's `Start`s.
#[allow(clippy::too_many_arguments)]
fn activate(
    ender: &mut EnderState,
    started: Started<'_>,
    active_enemies: &Query<(Entity, &Mortal), (With<Enemy>, With<Registered>)>,
    rigs: &mut Query<&mut Rig, With<LevelCamera>>,
    damage: &mut MessageWriter<Damage>,
    sounds: &mut MessageWriter<PlaySound>,
    rng: &mut DeathRng,
    light: &SceneLight,
    clock: &FrameClock,
) {
    ender.activated = true;
    info!("{}: the level is complete", ender.spec.name);
    let Some(spec) = ender.spec.ender.clone() else { return };
    if spec.ui {
        *started.ui_wait = Some(END_OF_LEVEL_UI_DELAY);
    }
    if let Some(distance) = spec.camera_minimum_distance {
        for mut rig in rigs.iter_mut() {
            rig.settings.screen_edge_margin = 0.0;
            rig.settings.minimum_on_screen_width = 0.0;
            rig.settings.minimum_distance = distance;
        }
    }
    if spec.time_slower {
        *started.time_slower = Some(TimeSlower::default());
    }
    if spec.disable_game_overing {
        // `activator.enabled = false` (the UI itself is hidden by the unscaled pass).
        ender.game_over_enabled = false;
    }
    if spec.level_complete && started.level_time.is_none() {
        // `LevelComplete.Complete` → `LevelTimer`: `LevelTime = Time.time - startTime`.
        *started.level_time = Some(clock.time - started.start_time);
    }
    if spec.level_complete && ender.spec.enemy_destroyer {
        // `EndOfLevelEnemyDestroyer.LevelCompleted`: every registered enemy whose root is active.
        for (entity, mortal) in active_enemies.iter() {
            if mortal.avatar_active() {
                damage.write(Damage { target: entity, amount: f32::MAX, on_kill: OnKill::Nothing, responsible: None });
            }
        }
    }
    if let Some(target) = spec.explode {
        if let Some(marker) = ender.markers.iter_mut().find(|m| m.spec.object == target) {
            // `Explodable.Explode` → `ExplodeSpritesAndLines`: what the outline drew last frame.
            let visual = marker_visual(&marker.spec, light.for_shadows);
            let root = position_of(&marker.spec);
            if marker.active {
                marker.explosion = Some(Explosion::go(&visual, root, root, marker.spec.fade_wait_time, &mut rng.0, clock.stepper.frame().delta));
            }
            marker.active = false;
        }
    }
    let at = ender.markers.first().map(|m| position_of(&m.spec));
    for group in &spec.start_sounds {
        sounds.write(PlaySound { group: group.clone(), at, owner: None });
    }
}

fn position_of(marker: &EnderMarker) -> SimVec3 {
    SimVec3::new(marker.position[0], marker.position[1], marker.position[2])
}

fn marker_visual(marker: &EnderMarker, light: Option<SimVec3>) -> CharacterVisual {
    outline_visual(&marker.outline, position_of(marker), marker.line_thickness, marker.color, marker.shadow_plane_z, light)
}

/// `Explosion.Update` of exploded markers (scaled time).
fn tick_marker_explosions(mut end: ResMut<LevelEnd>, clock: Res<FrameClock>) {
    let delta = clock.stepper.frame().delta;
    for ender in &mut end.enders {
        for marker in &mut ender.markers {
            if let Some(explosion) = marker.explosion.as_mut() {
                if explosion.update(delta) {
                    marker.explosion = None;
                }
            }
        }
    }
}

/// What runs on unscaled time or while paused: `EndOfLevelTimeSlower.Update`,
/// `EndOfLevelUIActivator`'s coroutine, `GameOverUIActivator.Update`,
/// `MaximumDamageCalculator.Start` (second frame).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn unscaled_level_end(
    mut end: ResMut<LevelEnd>,
    mut ui: Option<ResMut<InGameUi>>,
    mut hud: Option<ResMut<HudScene>>,
    mut time_scale: ResMut<TimeScale>,
    real: Res<Time<Real>>,
    scaled: Res<Time>,
    session: Res<Session>,
    level: Res<ActiveLevel>,
    mut memory: ResMut<LobbyMemory>,
    art: Option<Res<ManArt>>,
    enemies: Query<&Mortal, (With<Enemy>, Allow<Disabled>)>,
    helicopters: Query<&crate::helicopter::Helicopter, Allow<Disabled>>,
    players: Query<(Entity, &Player, &PlayerScore, Option<&PlayerLives>)>,
    avatars: Query<(&ControlledBy, &Mortal, Option<&Palette>), (Without<Enemy>, Allow<Disabled>)>,
) {
    let end = &mut *end;
    end.frames += 1;
    // `MaximumDamageCalculator.Start`: `yield return null`, then the sum.
    if end.maximum_damage.is_none() && end.frames >= 2 {
        let soldiers: f32 = enemies.iter().map(|m| m.health.starting()).sum();
        let helis: f32 = helicopters.iter().filter(|h| h.is_enemy()).map(|h| h.starting_health()).sum();
        end.maximum_damage = Some(soldiers + helis);
    }
    if let Some(slower) = end.time_slower.as_mut() {
        time_scale.scale = slower.update(time_scale.scale, real.delta_secs());
    }
    let (Some(ui), Some(hud)) = (ui.as_deref_mut(), hud.as_deref_mut()) else { return };
    // `DisableGameOvering.Start` hides the game-over screen.
    for ender in &end.enders {
        if ender.activated && ender.spec.ender.as_ref().is_some_and(|e| e.disable_game_overing) {
            ui.hide_game_over(hud, &mut time_scale);
        }
    }
    // `GameOverUIActivator.Update`.
    let any_players = !players.is_empty();
    let all_out = any_players && players.iter().all(|(_, _, _, lives)| lives.is_some_and(|l| l.remaining() < 0));
    if all_out {
        for i in 0..end.enders.len() {
            if end.enders[i].game_over_enabled {
                end.enders[i].game_over_enabled = false;
                ui.game_over(hud, &mut time_scale);
            }
        }
    }
    // `EndOfLevelUIActivator.Start`: `WaitForSeconds(1)` (scaled), then until `LevelTime`.
    if let Some(wait) = end.ui_wait.as_mut() {
        *wait -= scaled.delta_secs();
        if *wait <= 0.0 && end.level_time.is_some() {
            end.ui_wait = None;
            let mut results = Vec::new();
            for (slot, player, score, lives) in &players {
                let Some((_, mortal, palette)) = avatars.iter().find(|(c, _, _)| c.0 == slot) else { continue };
                let body = palette.map_or_else(|| art.as_ref().map_or([255; 4], |a| a.body_color()), |p| p.0.body);
                results.push((
                    PlayerResult {
                        player_index: player.index,
                        damage_dealt: score.0.damage_dealt,
                        starting_lives: PLAYER_STARTING_LIVES - 2 * session.difficulty,
                        lives_remaining: lives.map_or(0, |l| l.remaining()),
                        starting_health: mortal.health.starting(),
                        health_remaining: mortal.health.remaining(),
                    },
                    format!("Player {}", player.index + 1),
                    body.map(|v| f32::from(v) / 255.0),
                ));
            }
            let report = EndOfLevelReport { players: results, level_time: end.level_time, maximum_damage: end.maximum_damage.unwrap_or(0.0) };
            if let Some(score) = ui.show_end_of_level(hud, &mut time_scale, &report, session.player_count) {
                // `HighScores.GetScoreTable(level, PlayerCount, Difficulty).AddScore`.
                info!("{}: score {} (golden: {})", level.0.name, score.value, score.golden);
                let golden = score.golden;
                memory.high_scores.table(&level.0.name, session.player_count, session.difficulty).add(score);
                memory.save.scores = true;
                // `Goals.GoalDone`: "complete" (and "golden") for every difficulty up to this one.
                for (result, _, _) in &report.players {
                    let id = sticks_sim::level_end::PlayerId::anonymous_local(result.player_index);
                    for d in -1..=session.difficulty {
                        let mut changed = memory.goals.goal_done(&id, &format!("complete {d}"), Some(&level.0.name));
                        if golden {
                            changed |= memory.goals.goal_done(&id, &format!("golden {d}"), Some(&level.0.name));
                        }
                        if changed {
                            memory.save.goals = true;
                        }
                    }
                }
            }
        }
    }
}

/// The markers' outlines and the explosions of exploded ones.
fn queue_markers(end: Res<LevelEnd>, art: Option<Res<ManArt>>, light: Res<SceneLight>, mut queue: ResMut<FrameQueue>) {
    let Some(art) = art else { return };
    for ender in &end.enders {
        for marker in &ender.markers {
            if marker.active {
                for geometry in marker_visual(&marker.spec, light.for_shadows).renderers(&art.art) {
                    queue.0.push_frame(art.texture, geometry);
                }
            }
            if let Some(explosion) = &marker.explosion {
                for geometry in explosion.renderers(&art.art, light.for_shadows) {
                    queue.0.push_frame(art.texture, geometry);
                }
            }
        }
    }
}
