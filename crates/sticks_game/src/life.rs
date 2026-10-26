//! Death and respawn: damage messages, the Abyss, lives, score and the avatar's respawn.
//!
//! The rules (`Health`, `Lives`, `Respawner`, explosion timing) live in `sticks_sim::life`; this
//! module connects them to entities. The original's "Player" object (lives, score) is the
//! [`Player`] slot entity; the character it controls is an avatar entity pointing back at it
//! through [`ControlledBy`], like `ControlSlot.Controller`.

use bevy::prelude::*;
use sticks_sim::life::{LifeCycle, LifeEvent, Lives, RespawnEnv, Score};
use sticks_sim::rng::GameRng;
use unity_compat::Vec2 as SimVec2;

use crate::camera::{CAMERA_TARGET_HEIGHT, CameraPlaceholder};
use crate::level::ActiveLevel;
use crate::player::{Avatar, ControlledBy, FrameClock, Player, TriggerEntered};
use crate::state::{GameMode, Session};
use crate::FrameSet;

/// Health and the respawner state machine of an avatar.
#[derive(Component, Deref, DerefMut)]
pub struct Mortal(pub LifeCycle);

/// `Lives` on the player slot (campaign only).
#[derive(Component, Deref, DerefMut)]
pub struct PlayerLives(pub Lives);

/// `PlayerScore` on the player slot.
#[derive(Component, Deref, DerefMut)]
pub struct PlayerScore(pub Score);

/// Frames to wait before the avatar's explosion object exists (`ExplodeSpritesAndLines.Run`
/// yields one frame after `Start`).
#[derive(Component)]
pub struct ExplosionWarmup(pub u8);

/// What happens to the target when a damage message is applied afterwards.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnKill {
    Nothing,
    /// `DamageOtherOnTriggerEnter`: the victim's controller loses a frag.
    SelfFrag,
}

/// A request to call `Health.Damage` on an avatar.
#[derive(Message, Debug, Clone, Copy)]
pub struct Damage {
    pub target: Entity,
    pub amount: f32,
    pub on_kill: OnKill,
    /// The player slot credited with the damage (`DamageOtherOnCollision.responsiblePlayer`).
    pub responsible: Option<Entity>,
}

/// Everything the life cycle raised, for effects, audio and UI to follow.
#[derive(Message, Debug, Clone, Copy)]
pub struct LifeNotice {
    pub avatar: Entity,
    pub event: LifeEvent,
}

/// `UnityEngine.Random`: the one global stream the original draws from (respawn point choice,
/// `CharacterColors.RandomFaceColor` for enemies, ...).
#[derive(Resource)]
pub struct DeathRng(pub GameRng);

impl Default for DeathRng {
    fn default() -> Self {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos() ^ d.as_secs() as u32);
        DeathRng(GameRng::from_seed(seed))
    }
}

pub struct LifePlugin;

impl Plugin for LifePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Session>()
            .init_resource::<DeathRng>()
            .add_message::<Damage>()
            .add_message::<LifeNotice>()
            // `OnTriggerEnter2D` callbacks run inside the physics step, before `Update`.
            .add_systems(Update, (abyss_triggers, apply_damage).chain().in_set(FrameSet::FixedSteps).after(crate::player::run_fixed_steps))
            .add_systems(Update, (prepare_explosions, tick_lives).chain().in_set(FrameSet::Characters).before(crate::player::update_characters))
            .add_systems(Update, late_update_lives.in_set(FrameSet::Present));
    }
}

/// `DamageOtherOnTriggerEnter.OnTriggerEnter2D`: anything with `Health` that enters the trigger
/// takes `float.MaxValue` damage (the Abyss).
fn abyss_triggers(mut entered: MessageReader<TriggerEntered>, level: Res<ActiveLevel>, mut damage: MessageWriter<Damage>) {
    for enter in entered.read() {
        let Some(collider) = level.0.colliders.get(enter.collider as usize) else { continue };
        if collider.has_behaviour("DamageOtherOnTriggerEnter") {
            damage.write(Damage { target: enter.avatar, amount: f32::MAX, on_kill: OnKill::SelfFrag, responsible: None });
        }
    }
}

/// Turn the life events of one avatar into world changes and notices.
fn apply_events(
    events: Vec<LifeEvent>,
    entity: Entity,
    avatar: &mut Avatar,
    placeholder: &mut CameraPlaceholder,
    level: &ActiveLevel,
    session: &Session,
    notices: &mut MessageWriter<LifeNotice>,
) {
    for event in events {
        match event {
            LifeEvent::CameraPlaceholderEnabled => {
                // `Respawner.EnablePlaceholder`: remember where the camera target was.
                let p = avatar.position();
                placeholder.position = Some(SimVec2::new(p.x, p.y + CAMERA_TARGET_HEIGHT));
            }
            LifeEvent::GameOver => info!("game over for {entity:?}"),
            LifeEvent::Exploded => info!("{entity:?} died at {:?}", avatar.position()),
            LifeEvent::Respawned { spawn_point } => {
                // `Respawner.Respawn`: the camera placeholder retires with one last update, then
                // `OnRespawned` reaches the controller.
                placeholder.respawn_frame_influence = placeholder.position.take();
                match level.0.spawn_points.get(spawn_point) {
                    Some(point) if session.mode != GameMode::Campaign => {
                        avatar.respawn_at(SimVec2::new(point.position[0], point.position[1]));
                    }
                    _ => avatar.respawn_at_last_safe_position(),
                }
                info!("{entity:?} respawned at {:?}", avatar.position());
            }
            _ => {}
        }
        notices.write(LifeNotice { avatar: entity, event });
    }
}

/// `Health.Damage` for every request this frame, in arrival order.
#[allow(clippy::too_many_arguments)]
pub(crate) fn apply_damage(
    mut requests: MessageReader<Damage>,
    mut notices: MessageWriter<LifeNotice>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    clock: Res<FrameClock>,
    mut rng: ResMut<DeathRng>,
    mut avatars: Query<(&mut Mortal, &mut Avatar, Option<&ControlledBy>, &mut CameraPlaceholder)>,
    mut slots: Query<(&Player, Option<&mut PlayerLives>, &mut PlayerScore)>,
) {
    for request in requests.read() {
        let Ok((mut mortal, mut avatar, controlled_by, mut placeholder)) = avatars.get_mut(request.target) else { continue };
        // Deactivated objects receive no physics callbacks.
        if !mortal.avatar_active() {
            continue;
        }
        // Characters with no controller (enemies) have neither lives nor a score.
        let controller = controlled_by.map(|c| c.0);
        let was_exhausted = mortal.health.is_exhausted();
        let remaining_before = mortal.health.remaining();
        let mut env = RespawnEnv {
            campaign: session.mode == GameMode::Campaign,
            versus_ended: false, // TODO: VersusGameEnder (versus modes are not implemented yet)
            spawn_point_count: level.0.spawn_points.len(),
            time_delta: clock.last_step_delta,
            rng: &mut rng.0,
        };
        let events = {
            let lives = controller.and_then(|c| slots.get_mut(c).ok()).and_then(|(_, lives, _)| lives).map(|l| l.into_inner());
            mortal.damage(request.amount, lives.map(|l| &mut l.0), &mut env)
        };
        let newly_exhausted = !was_exhausted && mortal.health.is_exhausted();
        let dealt = remaining_before - mortal.health.remaining();
        if request.on_kill == OnKill::SelfFrag && newly_exhausted {
            if let Some((player, _, mut score)) = controller.and_then(|c| slots.get_mut(c).ok()) {
                score.self_fragged(player.index);
            }
        }
        // `DamageOtherOnCollision`: the shooter is credited with the health the hit took away and
        // with a frag when it finished its victim off.
        // TODO: `DamagingDoesNotAwardScore` (no prefab recovered so far carries it).
        if let Some(shooter) = request.responsible {
            let victim_index = controller.and_then(|c| slots.get(c).ok()).map(|(player, _, _)| player.index);
            if let Ok((_, _, mut score)) = slots.get_mut(shooter) {
                score.damage_dealt += dealt;
                if newly_exhausted {
                    if let Some(index) = victim_index {
                        score.fragged(index);
                    }
                }
            }
        }
        apply_events(events, request.target, &mut avatar, &mut placeholder, &level, &session, &mut notices);
    }
}

/// `ExplodeSpritesAndLines.Run`: the explosion object is built a frame after the avatar starts.
fn prepare_explosions(mut commands: Commands, mut query: Query<(Entity, &mut Mortal, &mut ExplosionWarmup)>) {
    for (entity, mut mortal, mut warmup) in &mut query {
        if warmup.0 == 0 {
            mortal.explosion_prepared();
            commands.entity(entity).remove::<ExplosionWarmup>();
        } else {
            warmup.0 -= 1;
        }
    }
}

/// `Explosion.Update`: run the explosion clocks; a finished explosion respawns its avatar.
pub(crate) fn tick_lives(
    clock: Res<FrameClock>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    mut rng: ResMut<DeathRng>,
    mut notices: MessageWriter<LifeNotice>,
    mut avatars: Query<(Entity, &mut Mortal, &mut Avatar, &mut CameraPlaceholder)>,
) {
    let delta = clock.stepper.frame().delta;
    for (entity, mut mortal, mut avatar, mut placeholder) in &mut avatars {
        let mut env = RespawnEnv {
            campaign: session.mode == GameMode::Campaign,
            versus_ended: false,
            spawn_point_count: level.0.spawn_points.len(),
            time_delta: delta,
            rng: &mut rng.0,
        };
        let events = mortal.tick(delta, &mut env);
        if events.is_empty() {
            continue;
        }
        apply_events(events, entity, &mut avatar, &mut placeholder, &level, &session, &mut notices);
    }
}

/// `ExplodeSpritesAndLines.LateUpdate`: avatars that exploded this frame are hidden now.
fn late_update_lives(mut avatars: Query<&mut Mortal>) {
    for mut mortal in &mut avatars {
        mortal.late_update();
    }
}
