//! The player characters: input devices → `CharacterInput`, then the simulation steps.

use bevy::input::mouse::AccumulatedMouseScroll;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::character::{Character, CharacterInput, DEFAULT_Z, StandingColliders};
use unity_compat::time::FixedStepper;
use unity_compat::Vec2 as SimVec2;

use crate::camera::{CameraPlaceholder, LevelCamera};
use crate::level::{ActiveLevel, CharacterAnimation, Collision};
use crate::life::{ExplosionWarmup, Mortal, PlayerLives, PlayerScore};
use crate::state::{AppState, GameMode, Session};
use crate::weapon::{AimPivot, Team};
use sticks_sim::life::{LifeCycle, Lives, Score};
use sticks_sim::lobby::{MAX_PLAYERS, assign_spawn_points, spawn_palette, spawns, team_index};
use sticks_sim::rng::GameRng;
use crate::figure::Palette;
use crate::input::{Gamepads, InputDevice, MouseActivity};
use crate::FrameSet;
use sticks_sim::devices::{CharacterControls, ProcessorStatus, RoleInputChoice, keyboard_character_status};

/// A player slot: the original's `Player` object. It owns lives and score and controls one
/// avatar (see [`ControlledBy`]).
#[derive(Component)]
pub struct Player {
    pub index: usize,
    /// `Player.TeamIndex` (the player index in Versus, its `LobbyTeams` team in Team Versus,
    /// 0 in the campaign).
    pub team: usize,
}

/// The processors of a player's role inputs (`PlayerSpawner.Spawn` → `RoleInput.AddProcessor`)
/// and its `CharacterInput`'s current processor.
#[derive(Component, Debug, Clone)]
pub struct PlayerDevices {
    pub devices: Vec<InputDevice>,
    character: RoleInputChoice,
    /// The `HelicopterInput`'s current processor while the player flies.
    pub(crate) helicopter: RoleInputChoice,
}

impl PlayerDevices {
    pub fn new(devices: Vec<InputDevice>) -> Self {
        Self { devices, character: RoleInputChoice::default(), helicopter: RoleInputChoice::default() }
    }

    /// The device the `CharacterInput`'s current processor reads (the engaged one).
    pub(crate) fn character_device(&self) -> Option<InputDevice> {
        self.character.current.and_then(|i| self.devices.get(i).copied())
    }
}

/// `CharacterInput.EnterVehicle`: the enter/exit button went down this frame.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct EnterVehiclePressed(pub bool);

/// `ControlSlot.Controller`: the player slot driving this avatar.
#[derive(Component, Clone, Copy)]
pub struct ControlledBy(pub Entity);

/// An avatar's physics trigger entered this frame (`OnTriggerEnter2D`).
#[derive(Message, Debug, Clone, Copy)]
pub struct TriggerEntered {
    pub avatar: Entity,
    /// Index into the level's collider list.
    pub collider: u32,
}

/// An avatar's physics trigger was left this frame (`OnTriggerExit2D`).
#[derive(Message, Debug, Clone, Copy)]
pub struct TriggerExited {
    pub avatar: Entity,
    /// Index into the level's collider list.
    pub collider: u32,
}

/// Simulation state of a character (physics body, animator, controller bookkeeping).
#[derive(Component, Deref, DerefMut)]
pub struct Avatar(pub Character);

/// This frame's input for a character. The keyboard processor fills it for the player; enemy AI
/// will fill the same component, as in the original's `CharacterInput` role.
#[derive(Component, Deref, DerefMut)]
pub struct Controls(pub CharacterInput);

/// Frame timing under Unity's rules (dynamic `fixedDeltaTime`, clamped frame delta).
#[derive(Resource, Default)]
pub struct FrameClock {
    pub stepper: FixedStepper,
    /// Length of the most recent fixed step: `Time.deltaTime` inside physics callbacks.
    pub last_step_delta: f32,
    /// `Time.time`: the frame deltas added up.
    pub time: f32,
    /// Lengths of the fixed steps that ran this frame, for everything that moves with them.
    pub steps: Vec<f32>,
}

/// Height of the weapon pivot above the feet until the first rig pose exists; afterwards the
/// aim is measured from the equipped weapon's pivot bone ([`AimPivot`]).
const WEAPON_PIVOT_HEIGHT: f32 = 1.6;
/// `KeyboardAndMouseCharacterInput.MINIMUM_SCROLL_CHANGE_INTERVAL` and the smallest wheel
/// movement that counts.
const SCROLL_CHANGE_INTERVAL: f32 = 0.1;
const SCROLL_THRESHOLD: f32 = 0.04;
/// `Keyboard.WalkModifier` scales movement input.
const WALK_MODIFIER_FACTOR: f32 = 0.2;
/// `Keyboard.EnterVehiclePressedThisFrame` (= `ExitVehiclePressedThisFrame`).
pub const ENTER_OR_EXIT_KEY: KeyCode = KeyCode::KeyE;

pub struct PlayerPlugin;

impl Plugin for PlayerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FrameClock>()
            .add_systems(OnEnter(AppState::Loading), reset_clock)
            .add_message::<TriggerEntered>()
            .add_message::<TriggerExited>()
            .add_systems(OnEnter(AppState::InGame), spawn_player)
            .add_systems(Update, begin_frame.in_set(FrameSet::Input))
            .add_systems(Update, read_player_input.in_set(FrameSet::RoleInput))
            .add_systems(Update, run_fixed_steps.in_set(FrameSet::FixedSteps))
            .add_systems(Update, update_characters.in_set(FrameSet::Characters))
            .add_systems(Update, sync_transforms.in_set(FrameSet::Present));
    }
}

/// A new game starts its clock from zero.
fn reset_clock(mut clock: ResMut<FrameClock>) {
    *clock = FrameClock::default();
}

fn spawn_player(
    mut commands: Commands,
    level: Res<ActiveLevel>,
    collision: Res<Collision>,
    animation: Res<CharacterAnimation>,
    session: Res<Session>,
    memory: Res<crate::lobby::LobbyMemory>,
    net: Res<crate::net::Net>,
) {
    // `PlayerSlotAssignments.OnLoadedLevelEverywhere` → `PlayerSpawner.SpawnAll` → `Spawn`: every
    // slot with controls takes the next spawn point (shuffled outside the campaign or with
    // `RandomizeSpawnPoints`; `System.Random` is time-seeded, so any seed is faithful). In a
    // network game the host's seed makes every machine draw the same points and faces (the
    // original's server sent the indices).
    let seed = net.level_seed.unwrap_or_else(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(11, |d| d.subsec_nanos()));
    let mut rng = GameRng::from_seed(seed);
    let points = &level.0.spawn_points;
    let mut active = [false; MAX_PLAYERS];
    for p in &session.players {
        if p.index < MAX_PLAYERS && (p.remote || !p.devices.is_empty()) {
            active[p.index] = true;
        }
    }
    let shuffle = session.mode != GameMode::Campaign || level.0.randomize_spawn_points;
    let indices = assign_spawn_points(points.len(), active, shuffle, |i, n| rng.range_int(i as i32, n as i32) as usize);
    let spawned = spawns(indices, points.len());
    if spawned.is_empty() {
        error!("level {} has no player spawn points", level.0.name);
    }
    for (index, child) in spawned {
        let Some(spawn) = points.iter().find(|p| p.index == child) else { continue };
        let team_versus = (session.mode == GameMode::TeamVersus).then_some(&session.teams);
        let team = team_index(session.mode == GameMode::Versus, session.mode == GameMode::TeamVersus, index, &session.teams);
        // `ControlAssignment.GetPlayerColors` with the defaults filled in.
        // A remote player's colours are what its machine sent (`RemoteColorAssignments`).
        let remote = session.is_remote(index);
        let colors = if remote { net.colors[index] } else { memory.player_colors(index) };
        let palette = spawn_palette(index, &colors, team_versus, &mut rng);
        // The `Player` object: lives exist in campaign mode only (`Player.SetUp` destroys them
        // otherwise); its role inputs get the processors of the slot's control types.
        let devices = session.devices(index).to_vec();
        let mut slot = commands.spawn((Player { index, team }, PlayerScore(Score::default()), PlayerDevices::new(devices)));
        if !remote {
            slot.insert(crate::net::LocalPlayer);
        }
        let campaign = session.mode == GameMode::Campaign;
        if campaign {
            slot.insert(PlayerLives(Lives::new(sticks_sim::life::PLAYER_STARTING_LIVES, session.difficulty)));
        }
        let slot = slot.id();

        let mut character = Character::new(
            animation.0.clone(),
            &collision.rules,
            StandingColliders::MAN,
            SimVec2::new(spawn.position[0], spawn.position[1]),
        );
        character.set_facing_right(spawn.facing_right);
        // `Player.SetUp`: `SetLayer(avatar, Utility.Layers.Team(TeamIndex))`.
        character.body.layer = (8 + team) as u8;
        let aim = SimVec2::new(if spawn.facing_right { 1.0 } else { -1.0 }, 0.0);
        let mut life = LifeCycle::man();
        if campaign {
            // `Player.SetUp` → `Health.LinkTo(lives)` while the player has no avatar yet.
            life.health.link_to_lives();
        }
        let avatar = commands
            .spawn((
                Avatar(character),
                Controls(CharacterInput { aim, ..CharacterInput::default() }),
                EnterVehiclePressed::default(),
                Team(team as i32),
                Mortal(life),
                ExplosionWarmup(1),
                ControlledBy(slot),
                crate::net::NetObject { id: sticks_net::NetId::Player(index as u8), local: !remote },
                CameraPlaceholder::default(),
                Transform::from_xyz(spawn.position[0], spawn.position[1], -DEFAULT_Z),
            ))
            .id();
        // `Player.SetUp` → `CharacterColor.SetColors` with the slot's colours.
        if let Some(palette) = palette {
            commands.entity(avatar).insert(Palette(palette));
        }
        info!("player {} spawned (team {team}) with {:?}", index + 1, session.devices(index));
    }
}

fn begin_frame(time: Res<Time>, mut clock: ResMut<FrameClock>) {
    clock.stepper.begin_frame(time.delta_secs());
    clock.time += clock.stepper.frame().delta;
}

/// `CharacterInput.Update` (`RoleInput.Update` + `OwnerOnlyUpdate`) for every player avatar:
/// the player's processors run in order until one is active (`KeyboardAndMouseCharacterInput`:
/// A/D move (LeftShift walks), Space jumps, S ducks, the left mouse button fires, Z or the wheel
/// changes weapon, E enters a vehicle, the mouse aims; `XboxishGamepadCharacterInput`: see
/// `sticks_sim::devices`), the chosen one's values are copied; with none every value is
/// cleared and the aim is `DefaultAim`.
///
/// Runs in [`FrameSet::RoleInput`], after this frame's fixed steps: `CharacterControl2D.FixedUpdate`
/// reads the input `CharacterInput` copied during the previous frame's `Update`.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(crate) fn read_player_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    scroll: Res<AccumulatedMouseScroll>,
    clock: Res<FrameClock>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<LevelCamera>>,
    mut pads: ResMut<Gamepads>,
    mouse: Res<MouseActivity>,
    mut slots: Query<&mut PlayerDevices>,
    mut players: Query<(&Avatar, &mut Controls, &mut EnterVehiclePressed, Option<&AimPivot>, &ControlledBy)>,
    mut since_scroll_change: Local<Option<f32>>,
    scripted_aim: Res<crate::devscript::ScriptedAim>,
) {
    let horizontal = (if keys.pressed(KeyCode::KeyA) { -1.0 } else { 0.0 }) + (if keys.pressed(KeyCode::KeyD) { 1.0 } else { 0.0 });
    let walking = keys.pressed(KeyCode::ShiftLeft);
    let vertical = (if keys.pressed(KeyCode::KeyS) { -1.0 } else { 0.0 }) + (if keys.pressed(KeyCode::KeyW) { 1.0 } else { 0.0 });

    // The wheel changes weapon at most every 0.1 s; up is next, down is previous.
    let waited = since_scroll_change.get_or_insert(SCROLL_CHANGE_INTERVAL);
    *waited += clock.stepper.frame().delta;
    let (mut wheel_next, mut wheel_previous) = (false, false);
    if *waited >= SCROLL_CHANGE_INTERVAL && scroll.delta.y.abs() > SCROLL_THRESHOLD {
        *waited = 0.0;
        wheel_next = scroll.delta.y > 0.0;
        wheel_previous = !wheel_next;
    }
    let keyboard = CharacterControls {
        movement: horizontal * if walking { WALK_MODIFIER_FACTOR } else { 1.0 },
        jump: keys.pressed(KeyCode::Space),
        duck: vertical < 0.0,
        fire: buttons.pressed(MouseButton::Left),
        aim: SimVec2::new(1.0, 0.0),
        next_weapon: keys.just_pressed(KeyCode::KeyZ) || wheel_next,
        enter_vehicle: keys.just_pressed(ENTER_OR_EXIT_KEY),
    };
    let keyboard_status = keyboard_character_status(&keyboard, wheel_previous, mouse.moved_last_frame);

    let cursor_world = cursor_world(&windows, &cameras);
    for (avatar, mut controls, mut enter, pivot, controlled_by) in &mut players {
        let Ok(mut devices) = slots.get_mut(controlled_by.0) else { continue };
        let devices = &mut *devices;
        let list = devices.devices.clone();
        let mut outputs: Vec<Option<CharacterControls>> = vec![None; list.len()];
        let current = devices.character.update(
            list.len(),
            |i| match list[i] {
                InputDevice::KeyboardMouse => {
                    outputs[i] = Some(keyboard);
                    keyboard_status
                }
                InputDevice::Gamepad(k) => match pads.frame_mut(k) {
                    None => ProcessorStatus::Unavailable,
                    Some(f) => {
                        outputs[i] = Some(f.character.process(&f.state, &f.edges));
                        if f.state.is_active() { ProcessorStatus::Active } else { ProcessorStatus::Idle }
                    }
                },
            },
            |i| list[i].kind(),
        );
        let chosen = current.and_then(|i| outputs[i].map(|c| (list[i], c)));
        let Some((device, c)) = chosen else {
            // `currentProcessor == null`: everything cleared, `RelativeAim` → `DefaultAim`.
            controls.movement = 0.0;
            controls.jump = false;
            controls.duck = false;
            controls.fire = false;
            controls.next_weapon = false;
            controls.previous_weapon = false;
            controls.aim = SimVec2::new(1.0, 0.0);
            enter.0 = false;
            continue;
        };
        controls.movement = c.movement;
        controls.jump = c.jump;
        controls.duck = c.duck;
        controls.fire = c.fire;
        controls.next_weapon = c.next_weapon;
        enter.0 = c.enter_vehicle;
        match device {
            InputDevice::KeyboardMouse => {
                controls.previous_weapon = wheel_previous;
                if let Some(offset) = scripted_aim.0 {
                    controls.aim = SimVec2::new(offset.x, offset.y);
                } else if let Some(point) = cursor_world {
                    // `AbsoluteAim - origin`.
                    let origin = pivot.map_or_else(|| avatar.position() + SimVec2::new(0.0, WEAPON_PIVOT_HEIGHT), |p| p.0);
                    controls.aim = SimVec2::new(point.x - origin.x, point.y - origin.y);
                }
            }
            InputDevice::Gamepad(_) => {
                controls.previous_weapon = false;
                controls.aim = c.aim;
            }
        }
    }
}

/// `Mouse.AbsoluteAim`: the cursor at the camera's distance, i.e. on the z = 0 plane.
pub(crate) fn cursor_world(windows: &Query<&Window, With<PrimaryWindow>>, cameras: &Query<(&Camera, &GlobalTransform), With<LevelCamera>>) -> Option<Vec3> {
    let cursor = windows.single().ok()?.cursor_position()?;
    let (camera, transform) = cameras.single().ok()?;
    let ray = camera.viewport_to_world(transform, cursor).ok()?;
    let t = -ray.origin.z / ray.direction.z;
    Some(ray.origin + *ray.direction * t)
}

/// Zero or more fixed steps per frame. `FixedUpdateSynchronizer` retunes the step length to the
/// frame time after every step, so normally exactly one step runs per frame. Deactivated
/// (dead) avatars neither move nor collide.
pub fn run_fixed_steps(
    mut clock: ResMut<FrameClock>,
    collision: Res<Collision>,
    mut avatars: Query<(Entity, &mut Avatar, &Controls, &Mortal)>,
    mut entered: MessageWriter<TriggerEntered>,
    mut exited: MessageWriter<TriggerExited>,
) {
    let frame_count = clock.stepper.frame().frame_count;
    clock.steps.clear();
    while let Some(delta) = clock.stepper.next_step() {
        clock.last_step_delta = delta;
        clock.steps.push(delta);
        for (entity, mut avatar, controls, mortal) in &mut avatars {
            if !mortal.avatar_active() {
                continue;
            }
            let report = avatar.fixed_update(&controls.0, &collision.world, &collision.rules, delta, frame_count);
            for &collider in &report.entered_triggers {
                entered.write(TriggerEntered { avatar: entity, collider });
            }
            for &collider in &report.exited_triggers {
                exited.write(TriggerExited { avatar: entity, collider });
            }
        }
        clock.stepper.synchronize_fixed_delta();
    }
}

/// `CharacterControl2D.Update`, the animator evaluation and `OnAnimatorMove`.
pub fn update_characters(clock: Res<FrameClock>, mut players: Query<(&mut Avatar, &Controls, &Mortal)>) {
    let delta = clock.stepper.frame().delta;
    if delta <= 0.0 {
        return;
    }
    for (mut avatar, controls, mortal) in &mut players {
        if mortal.avatar_active() {
            avatar.frame_update(&controls.0, delta);
        }
    }
}

fn sync_transforms(mut query: Query<(&Avatar, &mut Transform)>) {
    for (avatar, mut transform) in &mut query {
        let p = avatar.position();
        // Game z (-0.1, towards the camera) is negated for Bevy's handedness.
        transform.translation = Vec3::new(p.x, p.y, -DEFAULT_Z);
    }
}
