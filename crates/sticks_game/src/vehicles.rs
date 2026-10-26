//! Boarding, flying and leaving vehicles (`ControlSlot`, `CharacterControl2D`'s vehicle search,
//! `HelicopterControl.Update`, `Player.Update`), with the rules in `sticks_sim::vehicles`.
//!
//! * An active player avatar looks for the nearest free helicopter slot within its
//!   `controlRadius` (10) every frame; `E` takes it. The avatar's object is deactivated (here:
//!   Bevy's [`Disabled`], so every avatar system skips it, as `SetActive(false)` stops its
//!   scripts, physics, `Targetable` and camera influencer) and remembers the vehicle in
//!   [`Piloting`].
//! * While flown, the pilot's `KeyboardAndMouseHelicopterInput` drives the helicopter: A/D and
//!   S/W move it, the left button fires the single-player gun, which turns to the cursor. The
//!   "Camera Influencer" ten units ahead of the helicopter frames the camera instead of the
//!   avatar, and the HUD health bar shows the helicopter's health (`Player.SetUp` hands
//!   `HealthUI` the vehicle's `Health`).
//! * `E` again (not on the frame of entry) or the helicopter's destruction
//!   (`EjectControllersOnExplode`) relinquishes the slot; the avatar reappears at the
//!   "Disembarkment Point" (x/y; its own z) and takes control of itself again.
//!
//! Only helicopters exist as vehicles in the levels recovered so far that the game loads; cars,
//! rockets, cannons and passenger seats (`CarControl`, `RocketControl`, `CannonControl`,
//! `PassengerControl`) need the Box2D contact and joint solver (handoff item H7) and are not
//! implemented. The networked control transfer is not reproduced (the game is offline).
//!
//! TODO(UNCERTAIN): the vehicle and the avatar run their `Update`s in an unknown order; here
//! the avatar's search runs first, then the vehicle's exit check, then the disembarkment, all in
//! the same frame. A reactivated avatar is assumed not to run its `Update` (and so not to see
//! the stale `EnterVehicle` of its `CharacterInput`) until the next frame.
//! TODO: the "Enter Vehicle" control prompt (`Input/Non-Touch Control Prompts/Offset/Enter
//! Vehicle` in the Permanent System, shown while [`EnterableVehicle`] is set) is not drawn.

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::character::DEFAULT_Z;
use sticks_sim::vehicles::{Enterable, disembark_position, nearest_enterable};
use unity_compat::{Vec2 as SimVec2, Vec3 as SimVec3};

use crate::FrameSet;
use crate::camera::LevelCamera;
use crate::enemy::Enemy;
use crate::helicopter::Helicopter;
use crate::life::Mortal;
use crate::player::{Avatar, ControlledBy, cursor_world};
use crate::state::AppState;
use crate::weapon::Team;

/// `Keyboard.EnterVehiclePressedThisFrame` (= `ExitVehiclePressedThisFrame`).
const ENTER_OR_EXIT_KEY: KeyCode = KeyCode::KeyE;

/// On an avatar whose player flies a vehicle (the avatar itself is [`Disabled`]).
#[derive(Component, Clone, Copy, Debug)]
pub struct Piloting {
    pub vehicle: Entity,
}

/// `CharacterInput.EnterableVehicle`: the vehicle the local avatar would board on `E`.
#[derive(Resource, Default)]
pub struct EnterableVehicle(pub Option<Entity>);

pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnterableVehicle>()
            .add_systems(OnExit(AppState::InGame), |mut enterable: ResMut<EnterableVehicle>| enterable.0 = None)
            .add_systems(Update, read_helicopter_input.in_set(FrameSet::RoleInput).run_if(in_state(AppState::InGame)))
            .add_systems(
                Update,
                (enter_vehicles, leave_vehicles)
                    .chain()
                    .in_set(FrameSet::Characters)
                    .before(crate::helicopter::think_helicopters)
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

/// `HelicopterInput.Update` with `KeyboardAndMouseHelicopterInput` while the input is in use
/// (`RoleInput.SetUsed`: only while its player flies the helicopter).
fn read_helicopter_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<LevelCamera>>,
    mut helicopters: Query<&mut Helicopter>,
) {
    let axis = |negative: KeyCode, positive: KeyCode| (if keys.pressed(negative) { -1.0 } else { 0.0 }) + (if keys.pressed(positive) { 1.0 } else { 0.0 });
    let movement = SimVec2::new(axis(KeyCode::KeyA, KeyCode::KeyD), axis(KeyCode::KeyS, KeyCode::KeyW));
    // `Mouse.AbsoluteAim` falls back to `CharacterInput.DefaultAim` (as a point) without a camera.
    let aim = cursor_world(&windows, &cameras).map_or(SimVec2::new(1.0, 0.0), |p| SimVec2::new(p.x, p.y));
    for mut h in &mut helicopters {
        if h.spec.enemy.is_some() || h.pilot().is_none() {
            continue;
        }
        h.input = (movement, buttons.pressed(MouseButton::Left), Some(aim));
        h.exit = keys.just_pressed(ENTER_OR_EXIT_KEY);
    }
}

/// `CharacterControl2D.Update`'s vehicle search and `ControlSlot.SendRequestForControlFrom`.
fn enter_vehicles(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    avatars: Query<(Entity, &Avatar, &Mortal, &ControlledBy, Option<&Team>), Without<Enemy>>,
    mut helicopters: Query<(Entity, &mut Helicopter)>,
    mut enterable: ResMut<EnterableVehicle>,
) {
    enterable.0 = None;
    for (avatar_entity, avatar, mortal, controlled_by, team) in &avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let player = controlled_by.0;
        let p = avatar.position();
        // The avatar's own `ControlSlot` is the slot the player controls; it sits on the
        // character root, at the controller's fixed z.
        let at = SimVec3::new(p.x, p.y, DEFAULT_Z);
        let mut entities = Vec::new();
        let mut candidates = Vec::new();
        for (entity, h) in &helicopters {
            let Some(slot) = h.slot else { continue };
            let position = h.slot_position();
            let selectable = h.is_active_object() && slot.can_be_controlled_by(player, position, Some(at), true) && !slot.is_controlled_by(player);
            entities.push(entity);
            candidates.push(Enterable { position, selectable });
        }
        let chosen = nearest_enterable(at, &candidates).map(|i| entities[i]);
        enterable.0 = chosen;
        let Some(vehicle) = chosen.filter(|_| keys.just_pressed(ENTER_OR_EXIT_KEY)) else { continue };
        let Ok((_, mut h)) = helicopters.get_mut(vehicle) else { continue };
        // `RelinquishControlExceptFor`: the avatar's object is deactivated.
        h.take_control(player, team.map_or(0, |t| t.0.clamp(0, 3) as u8));
        commands.entity(avatar_entity).insert((Disabled, Piloting { vehicle }));
        enterable.0 = None;
        info!("player boarded {}", h.spec.name);
    }
}

/// `HelicopterControl.Update` (exit unless `controllerJustSet`), `EjectControllersOnExplode`, then
/// `Player.Update`: the avatar reappears at the disembarkment point.
fn leave_vehicles(mut commands: Commands, mut helicopters: Query<(Entity, &mut Helicopter)>, mut avatars: Query<(Entity, &mut Avatar, &Piloting), With<Disabled>>) {
    let mut released: Vec<(Entity, Option<SimVec3>)> = Vec::new();
    for (entity, mut h) in &mut helicopters {
        if h.pilot().is_none() {
            continue;
        }
        let eject = !h.is_alive();
        let exit = h.exit;
        if eject || h.exit_latch.update(exit) {
            h.relinquish();
            released.push((entity, h.disembarkment_point()));
            info!("player left {}", h.spec.name);
        }
    }
    for (avatar_entity, mut avatar, piloting) in &mut avatars {
        // A vehicle that no longer exists cannot hold the avatar either.
        let point = match released.iter().find(|(v, _)| *v == piloting.vehicle) {
            Some((_, point)) => *point,
            None if helicopters.get(piloting.vehicle).is_err() => None,
            None => continue,
        };
        let p = avatar.position();
        let destination = disembark_position(point, SimVec3::new(p.x, p.y, DEFAULT_Z));
        avatar.reactivate_at(SimVec2::new(destination.x, destination.y));
        commands.entity(avatar_entity).remove::<(Disabled, Piloting)>();
    }
}
