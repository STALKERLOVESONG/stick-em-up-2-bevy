//! Boarding, flying and leaving vehicles (`ControlSlot`, `CharacterControl2D`'s vehicle search,
//! `HelicopterControl.Update`, `Player.Update`), with the rules in `sticks_sim::vehicles`.
//!
//! * An active player avatar looks for the nearest free helicopter slot within its
//!   `controlRadius` (10) every frame; its `CharacterInput.EnterVehicle` (E, or a gamepad's A)
//!   takes it. The avatar's object is deactivated (here:
//!   Bevy's [`Disabled`], so every avatar system skips it, as `SetActive(false)` stops its
//!   scripts, physics, `Targetable` and camera influencer) and remembers the vehicle in
//!   [`Piloting`].
//! * While flown, the pilot's `KeyboardAndMouseHelicopterInput` drives the helicopter: A/D and
//!   S/W move it, the left button fires the single-player gun, which turns to the cursor; a
//!   gamepad pilot's `XboxishGamepadHelicopterInput` flies with the left stick, fires with the
//!   right trigger, aims with the right stick and leaves with A. The
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
//! The "Enter Vehicle" control prompt (`Input/Non-Touch Control Prompts/Offset/Enter Vehicle` in
//! the Permanent System) shows while a player can enter a vehicle ([`EnterPrompt`]; `hud` draws
//! it).

use bevy::ecs::entity_disabling::Disabled;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::character::DEFAULT_Z;
use sticks_sim::devices::{HelicopterControls, ProcessorStatus, keyboard_helicopter_status};
use sticks_sim::vehicles::{Enterable, disembark_position, nearest_enterable};
use unity_compat::{Vec2 as SimVec2, Vec3 as SimVec3};

use crate::FrameSet;
use crate::camera::LevelCamera;
use crate::enemy::Enemy;
use crate::helicopter::{HeliAim, Helicopter};
use crate::input::{Gamepads, InputDevice, MouseActivity};
use crate::life::Mortal;
use crate::player::{Avatar, ControlledBy, ENTER_OR_EXIT_KEY, EnterVehiclePressed, PlayerDevices, cursor_world};
use crate::state::AppState;
use crate::weapon::Team;


/// Which "Enter Vehicle" control prompts show this frame (`ControlPrompt.SetUsed`): some player
/// whose engaged input is the keyboard (or a gamepad) stands where a vehicle can be entered.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct EnterPrompt {
    pub keyboard: bool,
    pub gamepad: bool,
}

/// On an avatar whose player flies a vehicle (the avatar itself is [`Disabled`]).
#[derive(Component, Clone, Copy, Debug)]
pub struct Piloting {
    pub vehicle: Entity,
}

/// `CharacterInput.EnterableVehicle`: the vehicle the last avatar searched would board.
#[derive(Resource, Default)]
pub struct EnterableVehicle(pub Option<Entity>);

pub struct VehiclePlugin;

impl Plugin for VehiclePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<EnterableVehicle>()
            .init_resource::<EnterPrompt>()
            .add_systems(OnExit(AppState::InGame), |mut enterable: ResMut<EnterableVehicle>| enterable.0 = None)
            .add_systems(Update, (read_helicopter_input, read_car_input).in_set(FrameSet::RoleInput).run_if(in_state(AppState::InGame)))
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

/// `HelicopterInput.Update` while the input is in use (`RoleInput.SetUsed`: only while its
/// player flies the helicopter): the pilot's processors (`KeyboardAndMouseHelicopterInput`:
/// A/D and S/W fly, the left button fires at the cursor, E leaves; `XboxishGamepadHelicopterInput`:
/// see `sticks_sim::devices::PadHelicopter`) run until one is active; with none every value is
/// cleared.
#[allow(clippy::too_many_arguments)]
pub(crate) fn read_helicopter_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<LevelCamera>>,
    mut pads: ResMut<Gamepads>,
    mouse: Res<MouseActivity>,
    mut slots: Query<&mut PlayerDevices>,
    mut helicopters: Query<&mut Helicopter>,
    mut cars: Query<&mut crate::cars::Car>,
) {
    let axis = |negative: KeyCode, positive: KeyCode| (if keys.pressed(negative) { -1.0 } else { 0.0 }) + (if keys.pressed(positive) { 1.0 } else { 0.0 });
    let keyboard = HelicopterControls {
        movement: SimVec2::new(axis(KeyCode::KeyA, KeyCode::KeyD), axis(KeyCode::KeyS, KeyCode::KeyW)),
        fire: buttons.pressed(MouseButton::Left),
        aim: None,
        exit: keys.just_pressed(ENTER_OR_EXIT_KEY),
    };
    let keyboard_status = keyboard_helicopter_status(keyboard.movement, keyboard.fire, keyboard.exit, mouse.moved_last_frame);
    // `Mouse.AbsoluteAim` falls back to `CharacterInput.DefaultAim` (as a point) without a camera.
    let cursor = cursor_world(&windows, &cameras).map_or(SimVec2::new(1.0, 0.0), |p| SimVec2::new(p.x, p.y));
    for mut h in &mut helicopters {
        if h.spec.enemy.is_some() {
            continue;
        }
        let Some(pilot) = h.pilot() else { continue };
        match helicopter_controls(pilot, &mut slots, &mut *pads, keyboard, keyboard_status) {
            Some((InputDevice::KeyboardMouse, c)) => {
                h.input = (c.movement, c.fire, Some(HeliAim::Absolute(cursor)));
                h.exit = c.exit;
            }
            Some((InputDevice::Gamepad(_), c)) => {
                // No relative aim yet: `DefaultAim`.
                h.input = (c.movement, c.fire, Some(HeliAim::Relative(c.aim.unwrap_or(SimVec2::new(1.0, 0.0)))));
                h.exit = c.exit;
            }
            None => {
                h.input = (SimVec2::new(0.0, 0.0), false, Some(HeliAim::Relative(SimVec2::new(1.0, 0.0))));
                h.exit = false;
            }
        }
    }
    // `HelicopterGunnerInput` of every gunner (keyboard: the left button fires at the cursor, E
    // leaves; a gamepad's helicopter processor otherwise): the helicopters' gun seats and the
    // cars' `CarGunnerControl` seats.
    let mut gunner_input = |gunner: Entity| -> (bool, bool, HeliAim) {
        match helicopter_controls(gunner, &mut slots, &mut *pads, keyboard, keyboard_status) {
            Some((InputDevice::KeyboardMouse, c)) => (c.fire, c.exit, HeliAim::Absolute(cursor)),
            Some((InputDevice::Gamepad(_), c)) => (c.fire, c.exit, HeliAim::Relative(c.aim.unwrap_or(SimVec2::new(1.0, 0.0)))),
            None => (false, false, HeliAim::Relative(SimVec2::new(1.0, 0.0))),
        }
    };
    for mut h in &mut helicopters {
        for (_, seat) in h.seats_mut() {
            let Some(gunner) = seat.gunner() else { continue };
            let (fire, exit, aim) = gunner_input(gunner);
            seat.fire = fire;
            seat.exit = exit;
            seat.aim = Some(aim);
        }
    }
    for mut car in &mut cars {
        for seat in &mut car.gunners {
            let Some(gunner) = seat.gunner() else { continue };
            let (fire, exit, aim) = gunner_input(gunner);
            seat.fire = fire;
            seat.exit = exit;
            seat.aim = Some(aim);
        }
    }
}

/// The current helicopter-role processor of `player`'s devices and its controls this frame.
fn helicopter_controls(player: Entity, slots: &mut Query<&mut PlayerDevices>, pads: &mut Gamepads, keyboard: HelicopterControls, keyboard_status: ProcessorStatus) -> Option<(InputDevice, HelicopterControls)> {
    let mut devices = slots.get_mut(player).ok()?;
    let devices = &mut *devices;
    let list = devices.devices.clone();
    let mut outputs: Vec<Option<HelicopterControls>> = vec![None; list.len()];
    let current = devices.helicopter.update(
        list.len(),
        |i| match list[i] {
            InputDevice::KeyboardMouse => {
                outputs[i] = Some(keyboard);
                keyboard_status
            }
            InputDevice::Gamepad(k) => match pads.frame_mut(k) {
                None => ProcessorStatus::Unavailable,
                Some(f) => {
                    outputs[i] = Some(f.helicopter.process(&f.state, &f.edges));
                    if f.state.is_active() { ProcessorStatus::Active } else { ProcessorStatus::Idle }
                }
            },
        },
        |i| list[i].kind(),
    );
    current.and_then(|i| outputs[i].map(|c| (list[i], c)))
}

/// `CarInput.OwnerOnlyUpdate` for driven cars: `KeyboardAndMouseCarInput` (`DesiredSpeed` =
/// W/S, `ClockwiseRotation` = A/D, E leaves) or a gamepad's helicopter processor (its movement
/// stick), whichever the player's devices make current.
pub(crate) fn read_car_input(keys: Res<ButtonInput<KeyCode>>, mut pads: ResMut<Gamepads>, mouse: Res<MouseActivity>, mut slots: Query<&mut PlayerDevices>, mut cars: Query<&mut crate::cars::Car>) {
    let axis = |negative: KeyCode, positive: KeyCode| (if keys.pressed(negative) { -1.0 } else { 0.0 }) + (if keys.pressed(positive) { 1.0 } else { 0.0 });
    let movement = SimVec2::new(axis(KeyCode::KeyA, KeyCode::KeyD), axis(KeyCode::KeyS, KeyCode::KeyW));
    let exit = keys.just_pressed(ENTER_OR_EXIT_KEY);
    let keyboard = HelicopterControls { movement, fire: false, aim: None, exit };
    let keyboard_status = keyboard_helicopter_status(movement, false, exit, mouse.moved_last_frame);
    // The controls of one player, from whichever of their devices is current.
    let mut controls_of = |player: Entity| -> Option<HelicopterControls> {
        let mut devices = slots.get_mut(player).ok()?;
        let devices = &mut *devices;
        let list = devices.devices.clone();
        let mut outputs: Vec<Option<HelicopterControls>> = vec![None; list.len()];
        let current = devices.helicopter.update(
            list.len(),
            |i| match list[i] {
                InputDevice::KeyboardMouse => {
                    outputs[i] = Some(keyboard);
                    keyboard_status
                }
                InputDevice::Gamepad(k) => match pads.frame_mut(k) {
                    None => ProcessorStatus::Unavailable,
                    Some(f) => {
                        outputs[i] = Some(f.helicopter.process(&f.state, &f.edges));
                        if f.state.is_active() { ProcessorStatus::Active } else { ProcessorStatus::Idle }
                    }
                },
            },
            |i| list[i].kind(),
        );
        current.and_then(|i| outputs[i])
    };
    for mut car in &mut cars {
        match car.driver().and_then(&mut controls_of) {
            Some(c) => {
                car.input = (c.movement.y, c.movement.x);
                car.exit = c.exit;
            }
            None => {
                car.input = (0.0, 0.0);
                car.exit = false;
            }
        }
        // `PassengerInput.Exit` of each passenger.
        for seat in &mut car.passengers {
            seat.exit = seat.slot.controller().and_then(&mut controls_of).is_some_and(|c| c.exit);
        }
    }
}

/// `CharacterControl2D.Update`'s vehicle search and `ControlSlot.SendRequestForControlFrom`.
fn enter_vehicles(
    mut commands: Commands,
    devices: Query<&PlayerDevices>,
    mut prompt: ResMut<EnterPrompt>,
    avatars: Query<(Entity, &Avatar, &Mortal, &ControlledBy, Option<&Team>, Option<&EnterVehiclePressed>), Without<Enemy>>,
    mut helicopters: Query<(Entity, &mut Helicopter)>,
    mut cars: Query<(Entity, &mut crate::cars::Car)>,
    mut cannons: Query<(Entity, &mut crate::cannon::Cannon)>,
    palettes: Query<&crate::figure::Palette>,
    art: Option<Res<crate::figure::ManArt>>,
    mut enterable: ResMut<EnterableVehicle>,
    players: Query<(), With<crate::player::Player>>,
) {
    // `Player.ActivePlayers.Count`.
    let active_players = players.iter().count();
    enterable.0 = None;
    *prompt = EnterPrompt::default();
    for (avatar_entity, avatar, mortal, controlled_by, team, pressed) in &avatars {
        if !mortal.avatar_active() {
            continue;
        }
        let player = controlled_by.0;
        let p = avatar.position();
        // The avatar's own `ControlSlot` is the slot the player controls; it sits on the
        // character root, at the controller's fixed z.
        let at = SimVec3::new(p.x, p.y, DEFAULT_Z);
        // Each candidate: the vehicle and, for a passenger seat, its index.
        let mut entities: Vec<(Entity, Option<usize>)> = Vec::new();
        let mut candidates = Vec::new();
        for (entity, h) in &helicopters {
            let Some(slot) = h.slot else { continue };
            let position = h.slot_position();
            let selectable = h.is_active_object() && slot.can_be_controlled_by(player, position, Some(at), true) && !slot.is_controlled_by(player);
            entities.push((entity, None));
            candidates.push(Enterable { position, selectable });
            // `HelicopterGunnerControl.CanControl`: a gunner seat once the pilot's is taken.
            let pilot_taken = slot.controller().is_some();
            for (g, seat) in h.seats() {
                let position = h.seat_position(g);
                let selectable = h.is_active_object() && pilot_taken && seat.slot.can_be_controlled_by(player, position, Some(at), true) && !seat.slot.is_controlled_by(player);
                entities.push((entity, Some(g)));
                candidates.push(Enterable { position, selectable });
            }
        }
        for (entity, car) in &cars {
            let Some(slot) = car.slot else { continue };
            let position = car.slot_position();
            let selectable = car.is_target() && slot.can_be_controlled_by(player, position, Some(at), true) && !slot.is_controlled_by(player);
            entities.push((entity, None));
            candidates.push(Enterable { position, selectable });
            // `PassengerControl.CanControl`: a passenger seat once the pilot's is taken.
            let pilot_taken = slot.controller().is_some();
            for (i, seat) in car.passengers.iter().enumerate() {
                let selectable = car.is_target() && pilot_taken && seat.slot.can_be_controlled_by(player, position, Some(at), true) && !seat.slot.is_controlled_by(player);
                entities.push((entity, Some(i)));
                candidates.push(Enterable { position, selectable });
            }
            // `CarGunnerControl.CanControl`: the gunner seat once the driver's is taken (seat
            // indices after the passengers').
            for (g, seat) in car.gunners.iter().enumerate() {
                let position = car.gunner_position(g);
                let selectable = car.is_target() && pilot_taken && seat.slot.can_be_controlled_by(player, position, Some(at), true) && !seat.slot.is_controlled_by(player);
                entities.push((entity, Some(car.passengers.len() + g)));
                candidates.push(Enterable { position, selectable });
            }
        }
        // Cannons (`CannonControl`, a non-exclusive slot: the character stays).
        for (entity, cannon) in &cannons {
            let position = cannon.position();
            let selectable = cannon.slot.can_be_controlled_by(player, position, Some(at), true) && !cannon.slot.is_controlled_by(player);
            entities.push((entity, None));
            candidates.push(Enterable { position, selectable });
        }
        let chosen = nearest_enterable(at, &candidates).map(|i| entities[i]);
        enterable.0 = chosen.map(|(e, _)| e);
        // `enterVehicleControlPrompt.SetUsed(processor)` by the engaged processor while a vehicle
        // can be entered.
        if chosen.is_some() {
            match devices.get(player).ok().and_then(PlayerDevices::character_device) {
                Some(InputDevice::KeyboardMouse) => prompt.keyboard = true,
                Some(InputDevice::Gamepad(_)) => prompt.gamepad = true,
                None => {}
            }
        }
        let Some((vehicle, seat)) = chosen.filter(|_| pressed.is_some_and(|p| p.0)) else { continue };
        let team_index = team.map_or(0, |t| t.0.clamp(0, 3) as u8);
        if let Ok((_, mut h)) = helicopters.get_mut(vehicle) {
            // `RelinquishControlExceptFor`: the avatar's object is deactivated.
            match seat {
                Some(g) => {
                    h.take_seat(g, player, team_index);
                    info!("player boarded {} as a gunner", h.spec.name);
                }
                None => {
                    h.take_control(player, team_index, active_players);
                    info!("player boarded {}", h.spec.name);
                }
            }
        } else if let Ok((_, mut cannon)) = cannons.get_mut(vehicle) {
            let body = crate::cannon::body_color(palettes.get(avatar_entity).ok(), art.as_ref().map_or([255; 4], |a| a.body_color()));
            cannon.take_control(player, Some(body));
            commands.entity(avatar_entity).insert(crate::cannon::OperatingCannon { cannon: vehicle });
            info!("player took {}", cannon.spec.name);
            enterable.0 = None;
            continue;
        } else if let Ok((_, mut car)) = cars.get_mut(vehicle) {
            match seat {
                Some(i) if i >= car.passengers.len() => {
                    let g = i - car.passengers.len();
                    car.take_gun(g, player, team_index);
                    info!("player boarded {} as the gunner", car.spec.name);
                }
                Some(i) => {
                    let seat = &mut car.passengers[i];
                    seat.slot.set_controller(player);
                    seat.exit_latch.on_set_controller();
                    info!("player boarded {} as a passenger", car.spec.name);
                }
                None => {
                    car.take_control(player, team_index);
                    info!("player boarded {}", car.spec.name);
                }
            }
        } else {
            continue;
        }
        commands.entity(avatar_entity).insert((Disabled, Piloting { vehicle }));
        enterable.0 = None;
    }
}

/// `HelicopterControl.Update` (exit unless `controllerJustSet`), `EjectControllersOnExplode`, then
/// `Player.Update`: the avatar reappears at the disembarkment point.
fn leave_vehicles(
    mut commands: Commands,
    mut helicopters: Query<(Entity, &mut Helicopter)>,
    mut cars: Query<(Entity, &mut crate::cars::Car)>,
    mut avatars: Query<(Entity, &mut Avatar, &Piloting, &ControlledBy, Option<&Team>), With<Disabled>>,
    players: Query<(), With<crate::player::Player>>,
) {
    let active_players = players.iter().count();
    // The vehicle, the player who left it (`None`: everybody in it) and where they get out.
    let mut released: Vec<(Entity, Option<Entity>, Option<SimVec3>)> = Vec::new();
    for (entity, mut h) in &mut helicopters {
        let eject = !h.is_alive();
        let point = h.disembarkment_point();
        // `HelicopterGunnerControl.Update` (exit unless `controllerJustSet`); the explosion
        // ejects the gunners too.
        // TODO(UNCERTAIN): gunners get out at the helicopter's disembarkment point (their
        // `VehicleOccupant` has none of its own).
        let seats: Vec<usize> = h.seats().filter(|(_, s)| s.gunner().is_some()).map(|(g, _)| g).collect();
        for g in seats {
            let leave = {
                let Some((_, seat)) = h.seats_mut().find(|(i, _)| *i == g) else { continue };
                let exit = seat.exit;
                eject || seat.exit_latch.update(exit)
            };
            if leave {
                let player = h.leave_seat(g);
                released.push((entity, player, point));
                info!("gunner left {}", h.spec.name);
            }
        }
        if h.pilot().is_some() {
            let exit = h.exit;
            if eject || h.exit_latch.update(exit) {
                let player = h.relinquish();
                released.push((entity, player, point));
                info!("player left {}", h.spec.name);
            }
        }
    }
    // `MoveToOtherControlSlotIfAvailable` on a gunner seat: its gunner takes over the free pilot
    // seat (`SendRequestForControlFrom` relinquishes the gun).
    for (_, _, piloting, controlled_by, team) in &avatars {
        let Ok((_, mut h)) = helicopters.get_mut(piloting.vehicle) else { continue };
        if h.pilot().is_some() || h.slot.is_none() || !h.is_alive() {
            continue;
        }
        let player = controlled_by.0;
        let seat = h.seats().find(|(g, s)| s.gunner() == Some(player) && h.spec.guns[*g].moves_to_pilot).map(|(g, _)| g);
        let Some(g) = seat else { continue };
        h.leave_seat(g);
        h.take_control(player, team.map_or(0, |t| t.0.clamp(0, 3) as u8), active_players);
        info!("gunner took the controls of {}", h.spec.name);
    }
    // `CarControl.Update` (exit unless `controllerJustSet`) and `EjectControllersOnExplode`.
    for (entity, mut car) in &mut cars {
        let eject = !car.alive();
        let point = car.disembarkment_point();
        // `PassengerControl.Update`: each passenger leaves on their own exit input.
        for seat in &mut car.passengers {
            if seat.slot.controller().is_none() {
                continue;
            }
            let exit = seat.exit;
            if eject || seat.exit_latch.update(exit) {
                let player = seat.slot.clear_controller();
                seat.exit = false;
                released.push((entity, player, point));
            }
        }
        // `CarGunnerControl.Update`: the gunner leaves on their exit input (not on the frame they
        // got in); the explosion ejects them too.
        for g in 0..car.gunners.len() {
            let seat = &mut car.gunners[g];
            if seat.gunner().is_none() {
                continue;
            }
            let exit = seat.exit;
            if eject || seat.exit_latch.update(exit) {
                let player = car.leave_gun(g);
                released.push((entity, player, point));
            }
        }
        if car.driver().is_none() {
            continue;
        }
        let exit = car.exit;
        if eject || car.exit_latch.update(exit) {
            let player = car.relinquish();
            released.push((entity, player, point));
            info!("player left {}", car.spec.name);
        }
    }
    for (avatar_entity, mut avatar, piloting, controlled_by, _) in &mut avatars {
        // A vehicle that no longer exists cannot hold the avatar either.
        let point = match released.iter().find(|(v, p, _)| *v == piloting.vehicle && p.is_none_or(|p| p == controlled_by.0)) {
            Some((_, _, point)) => *point,
            None if helicopters.get(piloting.vehicle).is_err() && cars.get(piloting.vehicle).is_err() => None,
            None => continue,
        };
        let p = avatar.position();
        let destination = disembark_position(point, SimVec3::new(p.x, p.y, DEFAULT_Z));
        avatar.reactivate_at(SimVec2::new(destination.x, destination.y));
        commands.entity(avatar_entity).remove::<(Disabled, Piloting)>();
    }
}
