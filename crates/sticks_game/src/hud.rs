//! The in-game HUD as the original draws it: the `Player Info` objects of the Permanent System
//! scene, seen by its `UI Camera` on top of the level.
//!
//! * `UI Camera`: orthographic, at (0, 1, -10), depth 10, culling mask = layer 5 ("UI"), clear
//!   flags "Don't Clear"; `BigScreenOrthographicCameraScaler` sets its size every frame to
//!   `0.5 * GetWindowFractionOfRange() + 1` (window diagonal in inches, 10..21.5 → 0..1).
//! * The HUD sprites are tk2d sprites of the "UI" collection drawn with `tk2d/BlendVertexColor`
//!   (`assets_raw/Assets/Resources/BlendVertexColor.bytes`): unlit `texture * primary`, alpha
//!   `texture * primary`, `Blend SrcAlpha OneMinusSrcAlpha`, `Cull Off`, `ZWrite Off`. The shared
//!   lit material doubles its colour, so these use a 0.5 tint and skip vertex lighting.
//! * `Player Info` (`ScaleWithAspectRatioToMaintainWidthFraction`) holds `Player 1..4`, each with
//!   a `ViewportAnchor` (Player 1 at viewport (0.2, 0.9)), `HealthUI` (five bars: a `tk2dSprite`
//!   background with a `tk2dClippedSprite` "Bar" child), `VersusScoreUI` and
//!   `WeaponIndicatorUI`. `Player.SetUp` activates the player's object and sets them up.
//!
//! TODO: the Versus score label (a `tk2dTextMesh`) and the off-screen `Pointers` are not drawn;
//! only the local player has a HUD ("Player N" for its slot; one local player).

use std::collections::HashMap;

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::entity_disabling::Disabled;
use bevy::camera::{ClearColorConfig, ScalingMode};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::draw::{DrawQueue, clipped_sprite_geometry, sprite_geometry};
use unity_compat::{Vec3 as SimVec3, mathf};
use unity_data::interactive::{InteractiveScene, load_interactive_scene};
use unity_data::math::Affine;
use unity_data::sprites::{SpriteDef, SpriteLibrary};

use crate::FrameSet;
use crate::config::GameConfig;
use crate::enemy::Enemy;
use crate::figure::{ManArt, Palette};
use crate::helicopter::Helicopter;
use crate::vehicles::Piloting;
use crate::life::{Mortal, PlayerLives};
use crate::player::{ControlledBy, FrameClock, Player};
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{QueueSet, TextureLoad, UnityLitMaterial, UnityTextures, upload_runs};
use crate::weapon::Armament;

/// The Bevy render layer standing for Unity layer 5 ("UI"), the only one the UI camera draws.
const UI_LAYER: usize = 1;
/// `tk2d/BlendVertexColor` combines `texture * primary` without doubling.
const UNLIT_TINT: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
/// `HealthUI.SetUp`: the background is the bar colour × 107/180.
const BACKGROUND_BRIGHTNESS: f32 = 107.0 / 180.0;
/// `HealthUI.Fade` duration.
const FADE_SECONDS: f32 = 1.0;
/// `ScaleWithAspectRatioToMaintainWidthFraction.standardAspectRatio`.
const STANDARD_ASPECT_RATIO: f32 = 1.777_777_8;

/// Everything the HUD spawns (removed with the game).
#[derive(Component)]
pub struct Hud;

/// Mesh entities of the HUD's draw queue.
#[derive(Component)]
struct HudRun;

/// The Bevy camera standing for the `UI Camera`.
#[derive(Component)]
struct HudCamera;

/// The F1 diagnostics line (filled in by `debug`).
#[derive(Component)]
pub struct DebugText;

struct HudSprite {
    object: usize,
    def: SpriteDef,
    texture: u32,
    scale: [f32; 3],
    /// `tk2dBaseSprite.color` (scripts change it).
    color: [f32; 4],
    /// `tk2dClippedSprite` clip rectangle.
    clip: Option<[[f32; 2]; 2]>,
}

struct Fade {
    bar: usize,
    start: f32,
    start_alpha: f32,
}

/// `HealthUI` / `WeaponIndicatorUI` state of the local player's `Player Info` object.
struct PlayerSlot {
    bars: Vec<usize>,
    /// The bar the current life is shown in (`currentBar`).
    current: usize,
    lives_seen: i32,
    fades: Vec<Fade>,
    indicators: Option<usize>,
    /// Indicator objects and their names (the weapon each stands for).
    indicator_names: Vec<(usize, String)>,
    enabled_color: [f32; 4],
    disabled_color: [f32; 4],
    weapon_seen: Option<Option<String>>,
}

#[derive(Resource)]
struct HudScene {
    scene: InteractiveScene,
    active: Vec<bool>,
    local_position: Vec<[f32; 3]>,
    local_scale: Vec<[f32; 3]>,
    sprites: Vec<HudSprite>,
    sprite_of: HashMap<usize, usize>,
    player_info: usize,
    /// `ViewportAnchor`s under `Player Info`: object and viewport point.
    anchors: Vec<(usize, [f32; 2])>,
    /// Game-space position of the `UI Camera`.
    camera: [f32; 3],
    slot: Option<PlayerSlot>,
    queue: DrawQueue,
    pool: Vec<(Entity, Handle<Mesh>)>,
}

impl HudScene {
    fn descendants(&self, root: usize) -> Vec<usize> {
        let mut out = vec![root];
        let mut i = 0;
        while i < out.len() {
            out.extend(self.scene.objects[out[i]].children.iter().copied());
            i += 1;
        }
        out
    }

    /// `GetComponentInChildren<tk2dClippedSprite>(true)`: the object itself, then its children
    /// depth first.
    fn clipped_in(&self, object: usize) -> Option<usize> {
        if let Some(&s) = self.sprite_of.get(&object) {
            if self.sprites[s].clip.is_some() {
                return Some(s);
            }
        }
        self.scene.objects[object].children.iter().find_map(|&c| self.clipped_in(c))
    }

    fn set_color(&mut self, object: usize, color: [f32; 4]) {
        if let Some(&s) = self.sprite_of.get(&object) {
            self.sprites[s].color = color;
        }
    }

    /// World transform from the run-time local positions and scales; `anchored` objects have
    /// their world position set (`ViewportAnchor`).
    fn world_of(&self, object: usize, anchored: &HashMap<usize, [f32; 3]>, cache: &mut HashMap<usize, Affine>) -> Affine {
        if let Some(w) = cache.get(&object) {
            return *w;
        }
        let node = &self.scene.objects[object];
        let parent = node.parent.map_or(Affine::IDENTITY, |p| self.world_of(p, anchored, cache));
        let r = node.local_rotation;
        let mut world = parent.mul(&Affine::from_trs(self.local_position[object], (r[0], r[1], r[2], r[3]), self.local_scale[object]));
        if let Some(p) = anchored.get(&object) {
            world.t = *p;
        }
        cache.insert(object, world);
        world
    }
}

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), load_hud)
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<HudScene>())
            .add_systems(Update, update_hud.in_set(FrameSet::Present).run_if(resource_exists::<HudScene>))
            .add_systems(Update, draw_hud.in_set(QueueSet::Draw).run_if(resource_exists::<HudScene>));
    }
}

/// Load the Permanent System scene's `Player Info` and spawn the UI camera.
#[allow(clippy::too_many_arguments)]
fn load_hud(
    mut commands: Commands,
    config: Res<GameConfig>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    commands.spawn((
        Name::new("Debug text"),
        Hud,
        DebugText,
        Text::new(""),
        TextFont::from_font_size(16.0),
        TextColor(Color::srgb(0.8, 1.0, 0.8)),
        Node { position_type: PositionType::Absolute, left: Val::Px(16.0), bottom: Val::Px(12.0), ..default() },
        Visibility::Hidden,
    ));
    let scene = match load_interactive_scene(&config.export_root, "Assets/Game/Permanent System.unity") {
        Ok(scene) => scene,
        Err(e) => {
            error!("Permanent System.unity: {e}; no HUD");
            return;
        }
    };
    let find = |name: &str| scene.objects.iter().position(|o| o.name == name);
    let (Some(player_info), Some(ui_camera)) = (find("Player Info"), find("UI Camera")) else {
        error!("Permanent System.unity has no Player Info / UI Camera; no HUD");
        return;
    };
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut libraries: HashMap<String, SpriteLibrary> = HashMap::new();
    let mut hud = HudScene {
        active: scene.objects.iter().map(|o| o.active_self).collect(),
        local_position: scene.objects.iter().map(|o| o.local_position).collect(),
        local_scale: scene.objects.iter().map(|o| o.local_scale).collect(),
        sprites: Vec::new(),
        sprite_of: HashMap::new(),
        player_info,
        anchors: Vec::new(),
        camera: scene.objects[ui_camera].world.t,
        slot: None,
        queue: DrawQueue::default(),
        pool: Vec::new(),
        scene,
    };
    let subtree = hud.descendants(player_info);
    for sprite in &hud.scene.sprites {
        let Some(object) = hud.scene.object_of(sprite.object) else { continue };
        if !subtree.contains(&object) {
            continue;
        }
        if !libraries.contains_key(&sprite.collection) {
            match SpriteLibrary::load(&assets_dir, &sprite.collection) {
                Ok(lib) => {
                    libraries.insert(sprite.collection.clone(), lib);
                }
                Err(e) => {
                    warn!("HUD sprite collection {}: {e}", sprite.collection);
                    continue;
                }
            }
        }
        let lib = &libraries[&sprite.collection];
        let Some(def) = lib.collection.sprites.get(sprite.sprite_id) else { continue };
        let Some(path) = lib.texture_of(def) else { continue };
        hud.sprite_of.insert(object, hud.sprites.len());
        hud.sprites.push(HudSprite {
            object,
            def: def.clone(),
            texture: textures.tinted(path, UNLIT_TINT, &mut load),
            scale: sprite.scale,
            color: sprite.color,
            clip: sprite.clipped,
        });
    }
    hud.anchors = hud
        .scene
        .scripts_named("ViewportAnchor")
        .filter(|s| s.enabled && subtree.contains(&s.object))
        .map(|s| (s.object, s.fields.get("point").and_then(unity_data::yaml::Value::as_vec2).map_or([0.0, 0.0], |(x, y)| [x, y])))
        .collect();
    info!("HUD: {} sprites under Player Info, {} anchors", hud.sprites.len(), hud.anchors.len());
    let [x, y, z] = hud.camera;
    commands.spawn((
        Name::new("UI Camera"),
        Hud,
        HudCamera,
        Camera3d::default(),
        Camera { order: 10, clear_color: ClearColorConfig::None, ..default() },
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Orthographic(OrthographicProjection { scaling_mode: ScalingMode::FixedVertical { viewport_height: 2.0 }, ..OrthographicProjection::default_3d() }),
        RenderLayers::layer(UI_LAYER),
        // Game z is negated for Bevy.
        Transform::from_xyz(x, y, -z),
    ));
    commands.insert_resource(hud);
}

fn color_of(rgba: [u8; 4]) -> [f32; 4] {
    rgba.map(|v| f32::from(v) / 255.0)
}

/// `Player.SetUp` for the local player, then every frame what `HealthUI` and
/// `WeaponIndicatorUI` react to (`Health.Changed`, `Lives.Lost`, `WeaponChanged`).
#[allow(clippy::type_complexity)]
fn update_hud(
    mut hud: ResMut<HudScene>,
    session: Res<Session>,
    clock: Res<FrameClock>,
    art: Option<Res<ManArt>>,
    players: Query<(&Player, Option<&PlayerLives>)>,
    // `Has<Disabled>` keeps the avatar in view while its player flies a vehicle.
    avatars: Query<(&Mortal, &ControlledBy, &Armament, Has<Disabled>, Option<&Piloting>, Option<&Palette>), Without<Enemy>>,
    helicopters: Query<&Helicopter>,
) {
    let Ok((mortal, controlled_by, armament, disabled, piloting, palette)) = avatars.single() else { return };
    // `Player.SetUp` for a vehicle hands `HealthUI` the vehicle's `Health` and
    // `WeaponIndicatorUI` no character controller.
    let vehicle = piloting.and_then(|p| helicopters.get(p.vehicle).ok());
    let Ok((player, lives)) = players.get(controlled_by.0) else { return };
    let Some(art) = art else { return };
    let hud = &mut *hud;
    // Lives only exist in the campaign (`Player.SetUp` destroys them otherwise).
    let lives = if session.mode == GameMode::Campaign { lives } else { None };
    let weapon_name = |index: usize| art.model().rig().weapons.get(index).map(|w| w.name.clone());

    if hud.slot.is_none() {
        // `Player.SetUp`: `playerInfoPosition` is the player index, or in Team Versus its place
        // when the players are grouped by team.
        let position = if session.mode == GameMode::TeamVersus {
            session.teams.player_position_when_grouped(player.index, |p| p == session.player_index, session.player_count).unwrap_or(player.index)
        } else {
            player.index
        };
        let Some(player) = hud.scene.child(hud.player_info, &format!("Player {}", position + 1)) else { return };
        // `playerInfo.SetActive(true)` (which runs `WeaponIndicatorUI.Awake`).
        hud.active[player] = true;
        let Some(health_ui) = hud.scene.script(player, "HealthUI").cloned() else { return };
        let bars: Vec<usize> = hud.scene.field_objects(&health_ui, "bars").into_iter().flatten().collect();
        // `HealthUI.SetUp(lives, health, colors.Body)`.
        let mut bar_color = color_of(palette.map_or_else(|| art.body_color(), |p| p.body));
        bar_color[3] = 0.5;
        let mut background = bar_color.map(|c| c * BACKGROUND_BRIGHTNESS);
        background[3] = bar_color[3];
        let remaining = lives.map_or(0, |l| l.remaining());
        for (i, &bar) in bars.iter().enumerate() {
            hud.active[bar] = true;
            if let Some(clipped) = hud.clipped_in(bar) {
                hud.sprites[clipped].clip = Some([[0.0, 0.0], [1.0, 1.0]]);
                hud.sprites[clipped].color = bar_color;
            }
            hud.set_color(bar, background);
            hud.active[bar] = remaining >= i as i32;
        }
        if let (Some(below), [first, second, ..]) = (hud.scene.field_object(&health_ui, "belowBars"), bars.as_slice()) {
            let step = hud.local_position[*second][1] - hud.local_position[*first][1];
            hud.local_position[below] = [0.0, step * (remaining + 1) as f32, 0.0];
        }
        let current = remaining.clamp(0, bars.len() as i32 - 1) as usize;
        // `VersusScoreUI.SetUp`: the score label shows in Versus only.
        if let Some(label) = hud.scene.script(player, "VersusScoreUI").and_then(|s| hud.scene.field_object(s, "label")) {
            hud.active[label] = session.mode == GameMode::Versus;
        }
        // `WeaponIndicatorUI.Awake` / `SetUp`: allowed weapons' indicators side by side.
        let indicators = hud.scene.script(player, "WeaponIndicatorUI").and_then(|s| hud.scene.field_object(s, "indicators"));
        let mut indicator_names = Vec::new();
        let (mut enabled_color, mut disabled_color) = ([1.0; 4], [1.0; 4]);
        if let Some(container) = indicators {
            let children = hud.scene.objects[container].children.clone();
            if let [first, second, ..] = children.as_slice() {
                let offset = [0, 1, 2].map(|k| hud.local_position[*second][k] - hud.local_position[*first][k]);
                let color = |o: usize| hud.sprite_of.get(&o).map_or([1.0; 4], |&s| hud.sprites[s].color);
                enabled_color = color(*first);
                disabled_color = color(*second);
                let mut position = [0.0f32; 3];
                for &child in &children {
                    let name = hud.scene.objects[child].name.clone();
                    let allowed = armament.available().iter().any(|&i| weapon_name(i).as_deref() == Some(name.as_str()));
                    hud.active[child] = allowed;
                    if allowed {
                        hud.local_position[child] = position;
                        for k in 0..3 {
                            position[k] += offset[k];
                        }
                    }
                    indicator_names.push((child, name));
                }
            }
        }
        hud.slot = Some(PlayerSlot {
            bars,
            current,
            lives_seen: remaining,
            fades: Vec::new(),
            indicators,
            indicator_names,
            enabled_color,
            disabled_color,
            // TODO(UNCERTAIN): whether `WeaponChanged` fires after `SetUp` for the starting
            // weapon; the indicators are coloured from the first frame on.
            weapon_seen: None,
        });
    }

    let Some(mut slot) = hud.slot.take() else { return };
    // `Lives.Lost` → `HealthUI.LifeLost`: fade out the bar of the life just lost.
    if let Some(lives) = lives {
        while slot.lives_seen > lives.remaining() {
            slot.lives_seen -= 1;
            let remaining = slot.lives_seen;
            if let Some(&bar) = slot.bars.get((remaining + 1).max(0) as usize) {
                let start_alpha = hud.sprite_of.get(&bar).map_or(1.0, |&s| hud.sprites[s].color[3]);
                slot.fades.push(Fade { bar, start: clock.time, start_alpha });
            }
            if remaining >= 0 {
                slot.current = (remaining as usize).min(slot.bars.len().saturating_sub(1));
            }
        }
    }
    // `HealthUI.Fade` (a coroutine: one step a frame).
    let mut finished = Vec::new();
    for (i, fade) in slot.fades.iter().enumerate() {
        let since = clock.time - fade.start;
        if since >= FADE_SECONDS {
            hud.active[fade.bar] = false;
            finished.push(i);
            continue;
        }
        let fraction = since / FADE_SECONDS;
        if let Some(&s) = hud.sprite_of.get(&fade.bar) {
            hud.sprites[s].color[3] = (1.0 - fraction) * fade.start_alpha;
        }
    }
    for i in finished.into_iter().rev() {
        slot.fades.remove(i);
    }
    // `Health.Changed` → `HealthChanged`: the current bar is clipped to the health left.
    // TODO(UNCERTAIN): applied every frame instead of on the event (same result as long as the
    // event fires on every change).
    if let Some(&bar) = slot.bars.get(slot.current) {
        if let Some(clipped) = hud.clipped_in(bar) {
            let normalized = vehicle.map_or_else(|| mortal.health.normalized(), Helicopter::health_normalized);
            hud.sprites[clipped].clip = Some([[0.0, 0.0], [normalized, 1.0]]);
        }
    }
    // `WeaponChanged` → `OnWeaponChanged`.
    let equipped = armament.equipped().and_then(weapon_name);
    if slot.weapon_seen.as_ref() != Some(&equipped) {
        for (object, name) in &slot.indicator_names {
            let color = if Some(name) == equipped.as_ref() { slot.enabled_color } else { slot.disabled_color };
            hud.set_color(*object, color);
        }
        slot.weapon_seen = Some(equipped);
    }
    // `WeaponIndicatorUI.Update`: the indicators show while the avatar is active.
    if let Some(indicators) = slot.indicators {
        hud.active[indicators] = !disabled && mortal.avatar_active();
    }
    hud.slot = Some(slot);
}

/// Lay the HUD out for this frame's window (`BigScreenOrthographicCameraScaler`,
/// `ScaleWithAspectRatioToMaintainWidthFraction`, `ViewportAnchor`), queue its sprites as the UI
/// camera sorts them and upload them.
#[allow(clippy::too_many_arguments)]
fn draw_hud(
    mut commands: Commands,
    mut hud: ResMut<HudScene>,
    textures: Res<UnityTextures>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut cameras: Query<&mut Projection, With<HudCamera>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut runs_query: Query<(&mut Transform, &mut Visibility, &mut MeshMaterial3d<UnityLitMaterial>), With<HudRun>>,
) {
    if textures.materials.is_empty() {
        return;
    }
    let Ok(window) = windows.single() else { return };
    let hud = &mut *hud;
    let (width, height) = (window.physical_width() as f32, window.physical_height() as f32);
    if width <= 0.0 || height <= 0.0 {
        return;
    }
    let aspect = width / height;
    // `Utility.GetWindowDiagonalInches`: pixels over `Screen.dpi`.
    // TODO(UNCERTAIN): `Screen.dpi` on Windows taken as 96 × the window's scale factor.
    let dpi = 96.0 * window.scale_factor();
    let inches = mathf::sqrt(width * width + height * height) / dpi;
    let ortho = 0.5 * mathf::clamp01((inches - 10.0) / 11.5) + 1.0;
    for mut projection in &mut cameras {
        if let Projection::Orthographic(o) = &mut *projection {
            o.scaling_mode = ScalingMode::FixedVertical { viewport_height: 2.0 * ortho };
        }
    }
    let mut scale = aspect / STANDARD_ASPECT_RATIO;
    if (scale - 1.0).abs() < 0.01 {
        scale = 1.0;
    }
    hud.local_scale[hud.player_info] = [scale, scale, 1.0];

    // `ViewportAnchor.Update`: `camera.ViewportToWorldPoint(point, z)` of the orthographic camera.
    let camera = hud.camera;
    let mut anchored = HashMap::new();
    for &(object, point) in &hud.anchors {
        let z = hud.world_of(object, &anchored, &mut HashMap::new()).t[2];
        let x = camera[0] + (point[0] * 2.0 - 1.0) * ortho * aspect;
        let y = camera[1] + (point[1] * 2.0 - 1.0) * ortho;
        anchored.insert(object, [x, y, z]);
    }

    hud.queue.clear();
    let mut cache = HashMap::new();
    for i in 0..hud.sprites.len() {
        let object = hud.sprites[i].object;
        if !hud.scene.active_in_hierarchy(object, &hud.active) {
            continue;
        }
        let world = hud.world_of(object, &anchored, &mut cache);
        let s = &hud.sprites[i];
        let geometry = match s.clip {
            Some(clip) => clipped_sprite_geometry(&s.def, s.scale, clip, s.color, &world),
            None => sprite_geometry(&s.def, s.scale, s.color, &world),
        };
        if !geometry.is_empty() {
            hud.queue.push_frame(s.texture, geometry);
        }
    }
    hud.queue.sort(SimVec3::new(camera[0], camera[1], camera[2]));
    let runs = hud.queue.runs(&[], None);
    let base = Vec3::new(camera[0], camera[1], -camera[2]);
    upload_runs(&mut commands, &mut meshes, &textures, &mut hud.pool, &runs, base, |e| {
        e.insert((Hud, HudRun, RenderLayers::layer(UI_LAYER)));
    }, &mut runs_query);
}
