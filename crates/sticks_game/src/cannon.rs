//! Gun emplacements a player operates while their character stays where it is (`CannonControl`
//! on a non-exclusive `ControlSlot` with `IdleControllingCharacter`: Pirates' deck cannon).
//!
//! * Entering: like a vehicle (E near it, `crate::vehicles`), but the character is not put away:
//!   [`OperatingCannon`] idles its input every frame (`CharacterInput.SetAsIdle`: no movement,
//!   ducking, the idle aim 15° above horizontal).
//! * `HelicopterGunnerInput` of the operator: the keyboard aims at the mouse cursor and fires
//!   with the left button, E leaves; a gamepad's helicopter processor otherwise.
//! * `CannonControl.Update`: the "Pivot" turns to `RelativeAim(pivot)`; its launchers fire
//!   (`ProjectileLauncher`, the operator's team and credit) along the same direction.
//! * `CannonColor`: the "Barrel" takes the operator's body colour, white when nobody operates it.
//!
//! The sprites under the pivot are drawn here (they turn); the level art leaves them out.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::ecs::entity_disabling::Disabled;
use bevy::ecs::query::Allow;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::character::CharacterInput;
use sticks_sim::devices::{HelicopterControls, ProcessorStatus, keyboard_helicopter_status};
use sticks_sim::draw::{scene_sprite_geometry, sprite_shadow_geometry};
use sticks_sim::vehicles::{ControlSlot, ExitLatch};
use sticks_sim::weapons::{Launcher, Shot, difficulty_modified};
use unity_compat::{Quat as SimQuat, Vec2 as SimVec2, Vec3 as SimVec3};
use unity_data::math::Affine;
use unity_data::scene::FileId;
use unity_data::sprites::{SpriteDef, SpriteLibrary, TiledLayout};
use unity_data::vehicles::CannonSpawn;

use crate::FrameSet;
use crate::camera::LevelCamera;
use crate::config::GameConfig;
use crate::effects::PlaySound;
use crate::figure::Palette;
use crate::input::{Gamepads, InputDevice, MouseActivity};
use crate::level::{ActiveLevel, Collision};
use crate::player::{ControlledBy, Controls, ENTER_OR_EXIT_KEY, FrameClock, PlayerDevices, cursor_world};
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, TextureLoad, UnityLitMaterial, UnityTextures};
use crate::weapon::{Projectile, ProjectileLibrary, Team};

/// `CharacterInput.idleAim`: 15° above the horizontal.
fn idle_aim() -> SimVec2 {
    let a = std::f32::consts::PI / 12.0;
    SimVec2::new(a.cos(), a.sin())
}

/// A sprite turning with the pivot, at its place relative to the pivot.
struct CannonSprite {
    relative: Affine,
    def: SpriteDef,
    tiled: Option<TiledLayout>,
    scale: [f32; 3],
    color: [f32; 4],
    texture: u32,
    shadow: Option<(SpriteDef, u32, f32)>,
    barrel: bool,
}

/// One cannon.
#[derive(Component)]
pub struct Cannon {
    pub(crate) spec: Arc<CannonSpawn>,
    pub(crate) slot: ControlSlot<Entity>,
    pub(crate) exit_latch: ExitLatch,
    /// `HelicopterGunnerInput` of the operator this frame: exit, fire and aim (a world point for
    /// the mouse, a direction for a gamepad).
    pub(crate) exit: bool,
    fire: bool,
    aim: Option<Result<SimVec2, SimVec2>>,
    /// The pivot's world angle (degrees).
    angle: f32,
    launchers: Vec<Launcher>,
    sprites: Vec<CannonSprite>,
    /// `CannonColor`: the operator's body colour on the barrel.
    barrel_color: Option<[f32; 4]>,
}

impl Cannon {
    pub fn position(&self) -> SimVec3 {
        let p = self.spec.position;
        SimVec3::new(p[0], p[1], p[2])
    }

    pub fn operator(&self) -> Option<Entity> {
        self.slot.controller()
    }

    fn pivot_world(&self) -> Affine {
        let t = self.spec.pivot.t;
        let s = self.spec.pivot.lossy_scale();
        let q = SimQuat::about_z(self.angle.to_radians());
        Affine::from_trs(t, (q.x, q.y, q.z, q.w), s)
    }

    /// `ControlSlot.SetController` → `CannonControl.SetController` / `CannonColor`.
    pub(crate) fn take_control(&mut self, player: Entity, body_color: Option<[f32; 4]>) {
        self.slot.set_controller(player);
        self.exit_latch.on_set_controller();
        self.barrel_color = body_color;
    }

    /// `ClearController`: the launchers lose their input, the barrel turns white again.
    pub(crate) fn relinquish(&mut self) -> Option<Entity> {
        self.fire = false;
        self.aim = None;
        self.exit = false;
        self.barrel_color = None;
        self.slot.clear_controller()
    }
}

/// On the avatar of a player operating a cannon (`IdleControllingCharacter`).
#[derive(Component, Clone, Copy, Debug)]
pub struct OperatingCannon {
    pub cannon: Entity,
}

pub struct CannonPlugin;

impl Plugin for CannonPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), spawn_cannons)
            .add_systems(Update, (read_cannon_input, idle_operators).chain().in_set(FrameSet::RoleInput).after(crate::player::read_player_input).run_if(in_state(AppState::InGame)))
            .add_systems(Update, operate_cannons.in_set(FrameSet::Characters).run_if(in_state(AppState::InGame)))
            .add_systems(Update, queue_cannons.in_set(QueueSet::Collect).run_if(in_state(AppState::InGame)));
    }
}

fn spawn_cannons(
    mut commands: Commands,
    config: Res<GameConfig>,
    level: Res<ActiveLevel>,
    session: Res<Session>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    if level.0.cannons.is_empty() {
        return;
    }
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut libraries: HashMap<String, Option<SpriteLibrary>> = HashMap::new();
    for spec in &level.0.cannons {
        let spec = Arc::new(spec.clone());
        let part = |id: FileId| spec.parts.iter().find(|(o, _)| *o == id).map(|(_, a)| *a);
        let mut sprites = Vec::new();
        for (i, sprite) in level.0.sprites.iter().enumerate() {
            if sprite.shadow_of.is_some() {
                continue;
            }
            let Some(relative) = part(sprite.object) else { continue };
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
            sprites.push(CannonSprite { relative, def: def.clone(), tiled: sprite.tiled, scale: sprite.scale, color: sprite.color, texture, shadow, barrel: Some(sprite.object) == spec.barrel });
        }
        let launchers = spec
            .launchers
            .iter()
            .map(|l| {
                let rate = match &l.firing_rate_scaling {
                    Some(s) if session.mode == GameMode::Campaign => difficulty_modified(l.firing_rate_time, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
                    _ => l.firing_rate_time,
                };
                Launcher::new(rate, l.fully_automatic, false)
            })
            .collect();
        info!("cannon {} with {} launcher(s), {} sprite(s)", spec.name, spec.launchers.len(), sprites.len());
        commands.spawn((
            Name::new(spec.name.clone()),
            Cannon {
                slot: ControlSlot::new(spec.control_radius),
                exit_latch: ExitLatch::default(),
                exit: false,
                fire: false,
                aim: None,
                angle: spec.pivot_angle,
                launchers,
                sprites,
                barrel_color: None,
                spec,
            },
        ));
    }
}

/// `HelicopterGunnerInput` of each operator (its keyboard-and-mouse or gamepad processor).
#[allow(clippy::too_many_arguments)]
fn read_cannon_input(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<LevelCamera>>,
    mut pads: ResMut<Gamepads>,
    mouse: Res<MouseActivity>,
    mut slots: Query<&mut PlayerDevices>,
    mut cannons: Query<&mut Cannon>,
) {
    let fire = buttons.pressed(MouseButton::Left);
    let exit = keys.just_pressed(ENTER_OR_EXIT_KEY);
    let keyboard = HelicopterControls { movement: SimVec2::new(0.0, 0.0), fire, aim: None, exit };
    let keyboard_status = keyboard_helicopter_status(SimVec2::new(0.0, 0.0), fire, exit, mouse.moved_last_frame);
    let cursor = cursor_world(&windows, &cameras).map(|p| SimVec2::new(p.x, p.y));
    for mut cannon in &mut cannons {
        let Some(player) = cannon.operator() else { continue };
        let Ok(mut devices) = slots.get_mut(player) else { continue };
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
        match current.and_then(|i| outputs[i].map(|c| (list[i], c))) {
            Some((InputDevice::KeyboardMouse, c)) => {
                cannon.fire = c.fire;
                cannon.exit = c.exit;
                cannon.aim = cursor.map(Ok);
            }
            Some((InputDevice::Gamepad(_), c)) => {
                cannon.fire = c.fire;
                cannon.exit = c.exit;
                cannon.aim = c.aim.map(Err);
            }
            None => {
                cannon.fire = false;
                cannon.exit = false;
                cannon.aim = None;
            }
        }
    }
}

/// `CharacterControl2D.IdleInputIfAppropriate`: an operator's own input idles.
fn idle_operators(mut avatars: Query<(&mut Controls, &mut crate::player::EnterVehiclePressed), With<OperatingCannon>>) {
    for (mut controls, mut enter) in &mut avatars {
        controls.0 = CharacterInput { duck: true, aim: idle_aim(), ..CharacterInput::default() };
        enter.0 = false;
    }
}

/// `CannonControl.Update` (leave, or turn to the aim) and its launchers' `Update`.
#[allow(clippy::too_many_arguments)]
fn operate_cannons(
    mut commands: Commands,
    clock: Res<FrameClock>,
    session: Res<Session>,
    collision: Res<Collision>,
    library: Option<Res<ProjectileLibrary>>,
    mut cannons: Query<&mut Cannon>,
    operators: Query<(Entity, &ControlledBy, Option<&Team>, &crate::life::Mortal), (With<OperatingCannon>, Allow<Disabled>)>,
    mut sounds: MessageWriter<PlaySound>,
) {
    let delta = clock.stepper.frame().delta;
    for mut cannon in &mut cannons {
        let Some(player) = cannon.operator() else { continue };
        let operator = operators.iter().find(|(_, c, _, _)| c.0 == player);
        // An operator whose character is gone (dead, left the level) lets go too.
        let Some((avatar, _, team, mortal)) = operator else {
            cannon.relinquish();
            continue;
        };
        if !mortal.avatar_active() {
            cannon.relinquish();
            commands.entity(avatar).remove::<OperatingCannon>();
            continue;
        }
        let exit = cannon.exit;
        if cannon.exit_latch.update(exit) {
            cannon.relinquish();
            commands.entity(avatar).remove::<OperatingCannon>();
            info!("player left {}", cannon.spec.name);
            continue;
        }
        let pivot = cannon.spec.pivot.t;
        let Some(direction) = cannon.aim.map(|a| match a {
            Ok(point) => SimVec2::new(point.x - pivot[0], point.y - pivot[1]),
            Err(d) => d,
        }) else {
            continue;
        };
        if direction.sqr_magnitude() > 0.0 {
            cannon.angle = direction.y.atan2(direction.x).to_degrees();
        }
        let Some(library) = library.as_ref() else { continue };
        let world = cannon.pivot_world();
        let fire = cannon.fire;
        let team = team.map_or(0, |t| t.0);
        for i in 0..cannon.launchers.len() {
            let Some(launch) = cannon.launchers[i].update(fire, clock.time, delta) else { continue };
            let spec_launcher = &cannon.spec.launchers[i];
            let Some(spec) = library.0.get(&spec_launcher.projectile) else { continue };
            let Some((_, relative)) = cannon.spec.parts.iter().find(|(o, _)| *o == spec_launcher.object) else { continue };
            let muzzle = world.mul(relative).t;
            let damage = match &spec.damage_scaling {
                Some(s) if session.mode == GameMode::Campaign => difficulty_modified(spec.damage, &s.amount, s.affected_by_player_count, session.player_count as i32, session.difficulty),
                _ => spec.damage,
            };
            let shot = Shot::launch(spec, damage, SimVec2::new(muzzle[0], muzzle[1]), direction, launch.sub_frame_offset, team, collision.rules.gravity);
            commands.spawn((Name::new(spec.name.clone()), Projectile::new(shot, spec.clone(), Some(player))));
            if !spec_launcher.firing_sound.is_empty() {
                sounds.write(PlaySound { group: spec_launcher.firing_sound.clone(), at: Some(SimVec3::new(muzzle[0], muzzle[1], muzzle[2])), owner: None });
            }
        }
    }
}

/// The turning sprites (and their shadows) at the pivot's angle.
fn queue_cannons(light: Res<SceneLight>, cannons: Query<&Cannon>, mut queue: ResMut<FrameQueue>) {
    let light = light.for_shadows;
    for cannon in &cannons {
        let world = cannon.pivot_world();
        let rotation = SimQuat::about_z(cannon.angle.to_radians());
        for s in &cannon.sprites {
            let at = world.mul(&s.relative);
            let color = if s.barrel { cannon.barrel_color.unwrap_or([1.0, 1.0, 1.0, s.color[3]]) } else { s.color };
            let g = scene_sprite_geometry(&s.def, s.tiled.as_ref(), s.scale, color, &at);
            if !g.is_empty() {
                queue.0.push_frame(s.texture, g);
            }
            if let (Some((def, texture, plane)), Some(light)) = (&s.shadow, light) {
                let g = sprite_shadow_geometry(def, s.tiled.as_ref(), &at, rotation, s.scale, color[3], *plane, light);
                if !g.is_empty() {
                    queue.0.push_frame(*texture, g);
                }
            }
        }
    }
}

/// The operator's body colour for `CannonColor` (`Colors.Body`).
pub(crate) fn body_color(palette: Option<&Palette>, default: [u8; 4]) -> [f32; 4] {
    let c = palette.map_or(default, |p| p.0.body);
    [f32::from(c[0]) / 255.0, f32::from(c[1]) / 255.0, f32::from(c[2]) / 255.0, f32::from(c[3]) / 255.0]
}
