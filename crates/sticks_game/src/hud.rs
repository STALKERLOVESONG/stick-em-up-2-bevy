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
//! * The rest of `In-Game UI` (pause menus, tips, end of level, game over) is drawn from the
//!   same scene and camera: its `tk2dSlicedSprite` panels and button backgrounds
//!   (`draw::sliced_sprite_geometry`) and its `tk2dTextMesh`es; what those screens do is
//!   `ingame_ui`.
//!
//! * Every player has its `Player N` (Team Versus: its place grouped by team), its
//!   `VersusScoreUI` frags label in Versus, and in Team Versus the `Team Info` panels
//!   (`TeamVersusScoreUI`) show each team's frags.
//!
//! * The level's own UI-layer objects (Moon Race's "Timing UI", Office's "Enemies Remaining
//!   UI") are seen by the same UI camera: they are grafted into the HUD scene when the level
//!   loads (`InteractiveScene::graft`) and drawn with it; `LayoutControls` stacks their active
//!   children and `TimerUI` writes minutes, seconds and milliseconds (`level_end` drives them).
//!
//! TODO: the off-screen `Pointers` (`Pointer.Get(PlayerIndex)`, coloured in `Player.SetUp`) are
//! not drawn.

use std::collections::HashMap;

use bevy::camera::visibility::RenderLayers;
use bevy::ecs::entity_disabling::Disabled;
use bevy::camera::{ClearColorConfig, ScalingMode};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::draw::{DrawQueue, clipped_sprite_geometry, sliced_sprite_geometry, sprite_geometry};
use sticks_sim::text::{TextLayout, format_text, place, text_local_geometry};
use unity_data::fonts::FontLibrary;
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
use crate::life::{Mortal, PlayerLives, PlayerScore};
use crate::player::{ControlledBy, FrameClock, Player};
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{QueueSet, TextureLoad, UnityLitMaterial, UnityTextures, upload_runs};
use crate::weapon::Armament;

/// The Bevy render layer standing for Unity layer 5 ("UI"), the only one the UI camera draws.
const UI_LAYER: usize = 1;
/// `tk2d/BlendVertexColor` combines `texture * primary` without doubling (the UI atlas and the
/// UI font share that material).
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

/// `HealthUI` / `VersusScoreUI` / `WeaponIndicatorUI` state of a player's `Player Info`
/// object.
struct PlayerSlot {
    /// The `Player N` object.
    info: usize,
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
    /// `VersusScoreUI.label` (Versus only) and `lastFrags`.
    score_label: Option<usize>,
    frags_seen: Option<i32>,
}

/// A `tk2dSlicedSprite` of `In-Game UI`.
pub(crate) struct HudSliced {
    pub object: usize,
    def: SpriteDef,
    texture: u32,
    scale: [f32; 3],
    pub dimensions: [f32; 2],
    borders: [f32; 4],
    anchor: i64,
    border_only: bool,
    /// `tk2dSlicedSprite.color` (fades change its alpha).
    pub color: [f32; 4],
}

/// A `tk2dTextMesh` of `In-Game UI`.
pub(crate) struct HudText {
    pub object: usize,
    font: String,
    pub text: String,
    pub color: [f32; 4],
    color2: [f32; 4],
    use_gradient: bool,
    layout: TextLayout,
}

#[derive(Resource)]
pub(crate) struct HudScene {
    pub scene: InteractiveScene,
    pub active: Vec<bool>,
    pub local_position: Vec<[f32; 3]>,
    pub local_scale: Vec<[f32; 3]>,
    sprites: Vec<HudSprite>,
    sprite_of: HashMap<usize, usize>,
    pub sliced: Vec<HudSliced>,
    pub sliced_of: HashMap<usize, usize>,
    pub texts: Vec<HudText>,
    pub text_of: HashMap<usize, usize>,
    fonts: FontLibrary,
    font_textures: HashMap<String, u32>,
    /// `TagManager` sorting layers (unique ids in draw order).
    sorting_layers: Vec<i64>,
    player_info: usize,
    /// `In-Game UI` (menus and end-of-level screens).
    pub in_game_ui: Option<usize>,
    /// `ViewportAnchor`s under `Player Info` and `In-Game UI`: object and viewport point.
    anchors: Vec<(usize, [f32; 2])>,
    /// Objects with `ScaleWithAspectRatioToMaintainWidthFraction`.
    scalers: Vec<usize>,
    /// Game-space position of the `UI Camera`.
    pub camera: [f32; 3],
    /// This frame's layout: half the orthographic height, the aspect ratio and the anchored
    /// world positions (`ViewportAnchor.Update`).
    pub ortho: f32,
    pub aspect: f32,
    anchored: HashMap<usize, [f32; 3]>,
    /// Set-up `Player Info`s by player index.
    slots: HashMap<usize, PlayerSlot>,
    /// `Team Info` (Team Versus team frags).
    team_info: Option<usize>,
    queue: DrawQueue,
    pool: Vec<(Entity, Handle<Mesh>)>,
    /// The level's grafted UI-layer objects by their file id in the level scene.
    pub level_ui: HashMap<unity_data::scene::FileId, usize>,
    /// `LayoutControls`: the object and its child spacing (`Awake`: child 1 − child 0).
    layouts: Vec<(usize, [f32; 3])>,
    /// `TimerUI`: its object → the minutes, seconds and milliseconds texts.
    timers: HashMap<usize, [Option<usize>; 3]>,
    /// The "Enter Vehicle" `ControlPrompt`'s "Keyboard And Mouse" and "Xboxish Gamepad" prompts
    /// and its other children (the "Use" label).
    enter_prompt: Option<(Option<usize>, Option<usize>, Vec<usize>)>,
}

impl HudScene {
    /// `TimerUI.TimeInSeconds`: `TimeSpan.FromSeconds` (rounded to the millisecond, as .NET
    /// does) split into minutes `0`, seconds `00` and milliseconds `000`.
    pub fn set_timer(&mut self, timer: usize, seconds: f32) {
        let Some(&[minutes, secs, millis]) = self.timers.get(&timer) else { return };
        let total = (f64::from(seconds) * 1000.0 + if seconds >= 0.0 { 0.5 } else { -0.5 }).trunc() as i64;
        let ms = total.rem_euclid(1000);
        let s = (total / 1000).rem_euclid(60);
        let m = (total / 60_000).rem_euclid(60);
        for (object, text) in [(minutes, m.to_string()), (secs, format!("{s:02}")), (millis, format!("{ms:03}"))] {
            if let Some(o) = object {
                self.set_text(o, &text);
            }
        }
    }

    /// `GetComponentInChildren<TimerUI>()` under `object` (itself first).
    pub fn timer_in(&self, object: usize) -> Option<usize> {
        self.descendants(object).into_iter().find(|o| self.timers.contains_key(o))
    }

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

    /// Every object under `root` (itself included).
    pub fn subtree(&self, root: usize) -> Vec<usize> {
        self.descendants(root)
    }

    /// `tk2dTextMesh.text` of the text on `object`.
    pub fn set_text(&mut self, object: usize, text: &str) {
        if let Some(&t) = self.text_of.get(&object) {
            if self.texts[t].text != text {
                self.texts[t].text = text.to_owned();
            }
        }
    }

    pub fn text_color(&self, object: usize) -> Option<[f32; 4]> {
        self.text_of.get(&object).map(|&t| self.texts[t].color)
    }

    pub fn set_text_color(&mut self, object: usize, color: [f32; 4]) {
        if let Some(&t) = self.text_of.get(&object) {
            self.texts[t].color = color;
        }
    }

    pub fn active_in_hierarchy(&self, object: usize) -> bool {
        self.scene.active_in_hierarchy(object, &self.active)
    }

    /// The renderer's sorting layer rank and order (Unity sorts by them before depth).
    fn sorting_of(&self, object: usize) -> (i32, i32) {
        let (layer, order) = self.scene.sorting.get(&object).copied().unwrap_or((0, 0));
        (unity_data::settings::sorting_rank(&self.sorting_layers, layer), order as i32)
    }

    /// The object's world transform this frame (after this frame's layout).
    pub fn world(&self, object: usize) -> Affine {
        self.world_of(object, &self.anchored, &mut HashMap::new())
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

/// [`layout_hud`]: the UI's world transforms for this frame (the menus pick against them).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct HudLayout;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), load_hud)
            .add_systems(OnExit(AppState::InGame), |mut commands: Commands| commands.remove_resource::<HudScene>())
            .add_systems(Update, (update_hud, show_enter_prompt).in_set(FrameSet::Present).run_if(resource_exists::<HudScene>))
            .add_systems(Update, layout_hud.in_set(HudLayout).after(FrameSet::Present).before(QueueSet::Draw).run_if(in_state(AppState::InGame)).run_if(resource_exists::<HudScene>))
            .add_systems(Update, draw_hud.in_set(QueueSet::Draw).run_if(resource_exists::<HudScene>));
    }
}

/// Load the Permanent System scene's `Player Info` and spawn the UI camera.
#[allow(clippy::too_many_arguments)]
pub(crate) fn load_hud(
    mut commands: Commands,
    config: Res<GameConfig>,
    source: Res<crate::level::LevelSource>,
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
    let mut scene = match load_interactive_scene(&config.export_root, "Assets/Game/Permanent System.unity") {
        Ok(scene) => scene,
        Err(e) => {
            error!("Permanent System.unity: {e}; no HUD");
            return;
        }
    };
    // The level's own root objects on layer 5 ("UI"): drawn by the same UI camera.
    let mut level_ui = HashMap::new();
    let mut level_roots = Vec::new();
    let mut layouts = Vec::new();
    let mut timers = HashMap::new();
    match load_interactive_scene(&config.export_root, &source.0) {
        Ok(level_scene) => {
            let roots: Vec<usize> = (0..level_scene.objects.len()).filter(|&i| level_scene.objects[i].parent.is_none() && level_scene.objects[i].layer == 5).collect();
            let map = scene.graft(&level_scene, &roots);
            for (&old, &new) in &map {
                level_ui.insert(level_scene.objects[old].id, new);
            }
            level_roots = roots.iter().filter_map(|r| map.get(r).copied()).collect();
            for s in level_scene.scripts.iter().filter(|s| s.enabled && map.contains_key(&s.object)) {
                let object = map[&s.object];
                match s.class.as_str() {
                    "LayoutControls" => {
                        let children = &level_scene.objects[s.object].children;
                        if let (Some(&a), Some(&b)) = (children.first(), children.get(1)) {
                            let (pa, pb) = (level_scene.objects[a].local_position, level_scene.objects[b].local_position);
                            layouts.push((object, [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]]));
                        }
                    }
                    "TimerUI" => {
                        let text = |field: &str| level_scene.field_object(s, field).and_then(|o| map.get(&o).copied());
                        timers.insert(object, [text("minutes"), text("seconds"), text("milliseconds")]);
                    }
                    _ => {}
                }
            }
            if !level_roots.is_empty() {
                info!("HUD: {} UI object(s) from the level", map.len());
            }
        }
        Err(e) => warn!("{}: {e}; no level UI", source.0),
    }
    let find = |name: &str| scene.objects.iter().position(|o| o.name == name);
    let (Some(player_info), Some(ui_camera)) = (find("Player Info"), find("UI Camera")) else {
        error!("Permanent System.unity has no Player Info / UI Camera; no HUD");
        return;
    };
    let in_game_ui = find("In-Game UI");
    let team_info = find("Team Info");
    // `Input/Non-Touch Control Prompts` (its `Offset/Enter Vehicle` `ControlPrompt`).
    let prompts = find("Non-Touch Control Prompts");
    let enter_vehicle = prompts.and_then(|p| scene.child(p, "Offset")).and_then(|o| scene.child(o, "Enter Vehicle"));
    if let Some(ev) = enter_vehicle {
        // `LayoutControls`: its children one (child 1 − child 0) apart.
        let children = &scene.objects[ev].children;
        if let (Some(&a), Some(&b)) = (children.first(), children.get(1)) {
            let (pa, pb) = (scene.objects[a].local_position, scene.objects[b].local_position);
            layouts.push((ev, [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]]));
        }
    }
    let enter_prompt = enter_vehicle.map(|ev| {
        let named = |name: &str| scene.child(ev, name);
        let others = scene.objects[ev].children.iter().copied().filter(|&c| !matches!(scene.objects[c].name.as_str(), "Keyboard And Mouse" | "Xboxish Gamepad" | "Steam Controller")).collect();
        (named("Keyboard And Mouse"), named("Xboxish Gamepad"), others)
    });
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);
    let mut libraries: HashMap<String, SpriteLibrary> = HashMap::new();
    let mut hud = HudScene {
        active: scene.objects.iter().map(|o| o.active_self).collect(),
        local_position: scene.objects.iter().map(|o| o.local_position).collect(),
        local_scale: scene.objects.iter().map(|o| o.local_scale).collect(),
        sprites: Vec::new(),
        sprite_of: HashMap::new(),
        sliced: Vec::new(),
        sliced_of: HashMap::new(),
        texts: Vec::new(),
        text_of: HashMap::new(),
        fonts: FontLibrary::new(&assets_dir),
        font_textures: HashMap::new(),
        sorting_layers: std::fs::read_to_string(config.export_root.join("ProjectSettings/TagManager.asset"))
            .map(|t| unity_data::settings::parse_sorting_layers(&t))
            .unwrap_or_default(),
        player_info,
        in_game_ui,
        anchors: Vec::new(),
        scalers: Vec::new(),
        camera: scene.objects[ui_camera].world.t,
        ortho: 1.0,
        aspect: STANDARD_ASPECT_RATIO,
        anchored: HashMap::new(),
        slots: HashMap::new(),
        team_info,
        queue: DrawQueue::default(),
        pool: Vec::new(),
        level_ui,
        layouts,
        timers,
        enter_prompt,
        scene,
    };
    let mut subtree = hud.descendants(player_info);
    if let Some(t) = team_info {
        subtree.extend(hud.descendants(t));
    }
    let mut menus = in_game_ui.map(|root| hud.descendants(root)).unwrap_or_default();
    for &root in &level_roots {
        menus.extend(hud.descendants(root));
    }
    if let Some(p) = prompts {
        menus.extend(hud.descendants(p));
    }
    subtree.extend(menus.iter().copied());
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
    // `In-Game UI`'s sliced panels and its texts.
    for sliced in &hud.scene.sliced {
        let Some(object) = hud.scene.object_of(sliced.object) else { continue };
        if !menus.contains(&object) {
            continue;
        }
        if !libraries.contains_key(&sliced.collection) {
            match SpriteLibrary::load(&assets_dir, &sliced.collection) {
                Ok(lib) => {
                    libraries.insert(sliced.collection.clone(), lib);
                }
                Err(e) => {
                    warn!("UI sprite collection {}: {e}", sliced.collection);
                    continue;
                }
            }
        }
        let lib = &libraries[&sliced.collection];
        let Some(def) = lib.collection.sprites.get(sliced.sprite_id) else { continue };
        let Some(path) = lib.texture_of(def) else { continue };
        hud.sliced_of.insert(object, hud.sliced.len());
        hud.sliced.push(HudSliced {
            object,
            def: def.clone(),
            texture: textures.tinted(path, UNLIT_TINT, &mut load),
            scale: sliced.scale,
            dimensions: sliced.dimensions,
            borders: sliced.borders,
            anchor: sliced.anchor,
            border_only: sliced.border_only,
            color: sliced.color,
        });
    }
    let mut texts = Vec::new();
    for t in &hud.scene.texts {
        let Some(object) = hud.scene.object_of(t.object) else { continue };
        if !subtree.contains(&object) || t.shadow_of.is_some() {
            continue;
        }
        if !hud.font_textures.contains_key(&t.font) {
            let Some(path) = hud.fonts.get(&t.font).and_then(|f| f.texture.clone()) else {
                warn!("UI font {} not found", t.font);
                continue;
            };
            hud.font_textures.insert(t.font.clone(), textures.tinted(&path, UNLIT_TINT, &mut load));
        }
        texts.push(HudText {
            object,
            font: t.font.clone(),
            text: t.text.clone(),
            color: t.color,
            color2: t.color2,
            use_gradient: t.use_gradient,
            layout: TextLayout::of(t),
        });
    }
    for t in texts {
        hud.text_of.insert(t.object, hud.texts.len());
        hud.texts.push(t);
    }
    hud.anchors = hud
        .scene
        .scripts_named("ViewportAnchor")
        .filter(|s| s.enabled && subtree.contains(&s.object))
        .map(|s| (s.object, s.fields.get("point").and_then(unity_data::yaml::Value::as_vec2).map_or([0.0, 0.0], |(x, y)| [x, y])))
        .collect();
    hud.scalers = hud.scene.scripts_named("ScaleWithAspectRatioToMaintainWidthFraction").filter(|s| s.enabled && subtree.contains(&s.object)).map(|s| s.object).collect();
    for root in std::iter::once(player_info).chain(team_info) {
        if !hud.scalers.contains(&root) {
            hud.scalers.push(root);
        }
    }
    info!(
        "HUD: {} sprites, {} sliced sprites, {} texts, {} anchors",
        hud.sprites.len(),
        hud.sliced.len(),
        hud.texts.len(),
        hud.anchors.len()
    );
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

/// A player's avatar as the HUD reads it.
struct PlayerView<'a> {
    player: &'a Player,
    lives: Option<&'a PlayerLives>,
    mortal: &'a Mortal,
    armament: &'a Armament,
    disabled: bool,
    /// The health left of the vehicle the player drives (`Health.Normalized`), if any.
    vehicle: Option<f32>,
    palette: Option<&'a Palette>,
    /// `VersusScoreUI`: the definitive total once the game has ended, else `PlayerScore.Frags`.
    frags: i32,
}

/// `Player.SetUp` for every player, then every frame what `HealthUI`, `VersusScoreUI` and
/// `WeaponIndicatorUI` react to (`Health.Changed`, `Lives.Lost`, `WeaponChanged`, the frags),
/// and the Team Versus `TeamVersusScoreUI`s.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_hud(
    mut hud: ResMut<HudScene>,
    session: Res<Session>,
    clock: Res<FrameClock>,
    art: Option<Res<ManArt>>,
    versus: Option<Res<crate::versus::VersusState>>,
    players: Query<(&Player, Option<&PlayerLives>, &PlayerScore)>,
    // `Has<Disabled>` keeps the avatar in view while its player flies a vehicle.
    avatars: Query<(&Mortal, &ControlledBy, &Armament, Has<Disabled>, Option<&Piloting>, Option<&Palette>), Without<Enemy>>,
    helicopters: Query<&Helicopter>,
    cars: Query<&crate::cars::Car>,
    level: Option<Res<crate::level::ActiveLevel>>,
) {
    let Some(art) = art else { return };
    let hud = &mut *hud;
    // `HidePlayerInfo.OnEnable` / `OnDisable`: the "Complete" scene switches `Player Info` off.
    let player_info = hud.player_info;
    hud.active[player_info] = !level.as_ref().is_some_and(|l| l.0.hides_player_info);
    let mut teams: [Vec<(usize, [f32; 4], i32)>; 2] = [Vec::new(), Vec::new()];
    for (mortal, controlled_by, armament, disabled, piloting, palette) in &avatars {
        let Ok((player, lives, score)) = players.get(controlled_by.0) else { continue };
        // `Player.SetUp` for a vehicle hands `HealthUI` the vehicle's `Health` and
        // `WeaponIndicatorUI` no character controller.
        // A helicopter's or a car's (a car that cannot be destroyed keeps its bar full).
        let vehicle = piloting.and_then(|p| helicopters.get(p.vehicle).ok().map(Helicopter::health_normalized).or_else(|| cars.get(p.vehicle).ok().map(|c| c.health_normalized())));
        let frags = versus.as_ref().and_then(|v| v.definitive.total(player.index)).unwrap_or(score.0.frags());
        let view = PlayerView { player, lives, mortal, armament, disabled, vehicle, palette, frags };
        let Some(info) = update_player_info(hud, &session, &clock, &art, &view) else { continue };
        if let Some(members) = teams.get_mut(player.team) {
            let body = color_of(palette.map_or_else(|| art.body_color(), |p| p.body));
            members.push((info, body, frags));
        }
    }
    if session.mode == GameMode::TeamVersus {
        update_team_info(hud, &teams);
    }
}

/// `ControlPrompt.LateUpdate` of the "Enter Vehicle" prompt: each control type's prompt is on
/// while it was used this frame, the other children ("Use") while any is.
fn show_enter_prompt(mut hud: ResMut<HudScene>, prompt: Res<crate::vehicles::EnterPrompt>) {
    let hud = &mut *hud;
    let Some((keyboard, gamepad, others)) = hud.enter_prompt.clone() else { return };
    let mut any = false;
    for (object, used) in [(keyboard, prompt.keyboard), (gamepad, prompt.gamepad)] {
        if let Some(o) = object {
            hud.active[o] = used;
            any |= used;
        }
    }
    for o in others {
        hud.active[o] = any;
    }
}

/// `TeamVersusScoreUI` (`Team Info/Offset/<team>`): switched on by its first player's
/// `SetUp`, coloured like its players, centred over their `Player Info`s, showing their frags.
fn update_team_info(hud: &mut HudScene, teams: &[Vec<(usize, [f32; 4], i32)>; 2]) {
    let Some(offset) = hud.team_info.and_then(|t| hud.scene.child(t, "Offset")) else { return };
    for (team, members) in teams.iter().enumerate() {
        if members.is_empty() {
            continue;
        }
        let Some(info) = hud.scene.child(offset, &team.to_string()) else { continue };
        let Some(script) = hud.scene.script(info, "TeamVersusScoreUI").cloned() else { continue };
        let name = hud.scene.field_object(&script, "teamNameLabel");
        let score = hud.scene.field_object(&script, "scoreLabel");
        if !hud.active[info] {
            hud.active[info] = true;
            // `OnEnable`: off to the side until `Update` places it.
            hud.local_position[info][0] = -1000.0;
        }
        if let (Some(label), Some(color)) = (name, sticks_sim::versus::team_color(&members.iter().map(|m| m.1).collect::<Vec<_>>())) {
            hud.set_text_color(label, color);
        }
        // `Update`: `transform.position.x` = the mean x of the players' info objects.
        let mean = members.iter().map(|m| hud.world(m.0).t[0]).sum::<f32>() / members.len() as f32;
        if let Some(parent) = hud.scene.objects[info].parent {
            let world = hud.world(info).t;
            let local = hud.world(parent).inverse().transform_point([mean, world[1], world[2]]);
            hud.local_position[info][0] = local[0];
        }
        if let Some(label) = score {
            let total: i32 = members.iter().map(|m| m.2).sum();
            hud.set_text(label, &total.to_string());
        }
    }
}

/// One player's `Player Info`: its set-up on first sight, then this frame. Returns the info
/// object.
fn update_player_info(hud: &mut HudScene, session: &Session, clock: &FrameClock, art: &ManArt, view: &PlayerView<'_>) -> Option<usize> {
    let PlayerView { player, lives, mortal, armament, disabled, vehicle, palette, frags } = *view;
    // Lives only exist in the campaign (`Player.SetUp` destroys them otherwise).
    let lives = if session.mode == GameMode::Campaign { lives } else { None };
    let weapon_name = |index: usize| art.model().rig().weapons.get(index).map(|w| w.name.clone());

    if !hud.slots.contains_key(&player.index) {
        // `Player.SetUp`: `playerInfoPosition` is the player index, or in Team Versus its place
        // when the players are grouped by team.
        let position = if session.mode == GameMode::TeamVersus {
            session.teams.player_position_when_grouped(player.index, |p| session.has_controls(p), session.player_count).unwrap_or(player.index)
        } else {
            player.index
        };
        let info = hud.scene.child(hud.player_info, &format!("Player {}", position + 1))?;
        // `playerInfo.SetActive(true)` (which runs `WeaponIndicatorUI.Awake`).
        hud.active[info] = true;
        let health_ui = hud.scene.script(info, "HealthUI").cloned()?;
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
        // `VersusScoreUI.SetUp`: the score label shows (and updates) in Versus only.
        let score_label = hud.scene.script(info, "VersusScoreUI").and_then(|s| hud.scene.field_object(s, "label"));
        if let Some(label) = score_label {
            hud.active[label] = session.mode == GameMode::Versus;
        }
        // `WeaponIndicatorUI.Awake` / `SetUp`: allowed weapons' indicators side by side.
        let indicators = hud.scene.script(info, "WeaponIndicatorUI").and_then(|s| hud.scene.field_object(s, "indicators"));
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
        hud.slots.insert(
            player.index,
            PlayerSlot {
                info,
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
                score_label: score_label.filter(|_| session.mode == GameMode::Versus),
                frags_seen: None,
            },
        );
    }

    let mut slot = hud.slots.remove(&player.index)?;
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
            let normalized = vehicle.unwrap_or_else(|| mortal.health.normalized());
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
    // `VersusScoreUI.Update`.
    if let Some(label) = slot.score_label {
        if slot.frags_seen != Some(frags) {
            hud.set_text(label, &frags.to_string());
            slot.frags_seen = Some(frags);
        }
    }
    let info = slot.info;
    hud.slots.insert(player.index, slot);
    Some(info)
}

/// Lay the HUD out for this frame's window (`BigScreenOrthographicCameraScaler`,
/// `ScaleWithAspectRatioToMaintainWidthFraction`, `ViewportAnchor`).
fn layout_hud(mut hud: ResMut<HudScene>, windows: Query<&Window, With<PrimaryWindow>>, mut cameras: Query<&mut Projection, With<HudCamera>>) {
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
    for i in 0..hud.scalers.len() {
        let object = hud.scalers[i];
        hud.local_scale[object] = [scale, scale, 1.0];
    }
    hud.ortho = ortho;
    hud.aspect = aspect;
    // `LayoutControls.LateUpdate`: active children one spacing apart.
    for i in 0..hud.layouts.len() {
        let (object, offset) = hud.layouts[i];
        let mut at = [0.0f32; 3];
        for child in hud.scene.objects[object].children.clone() {
            if hud.active[child] {
                hud.local_position[child] = at;
                at = [at[0] + offset[0], at[1] + offset[1], at[2] + offset[2]];
            }
        }
    }

    // `ViewportAnchor.Update`: `camera.ViewportToWorldPoint(point, z)` of the orthographic camera.
    // Parents first (`Player Info`'s players under nothing anchored; the menus under `In-Game UI`).
    let camera = hud.camera;
    let mut anchored = HashMap::new();
    for &(object, point) in &hud.anchors {
        let z = hud.world_of(object, &anchored, &mut HashMap::new()).t[2];
        let x = camera[0] + (point[0] * 2.0 - 1.0) * ortho * aspect;
        let y = camera[1] + (point[1] * 2.0 - 1.0) * ortho;
        anchored.insert(object, [x, y, z]);
    }
    hud.anchored = anchored;
}

/// Queue the HUD's sprites, panels and texts as the UI camera sorts them and upload them.
#[allow(clippy::too_many_arguments)]
fn draw_hud(
    mut commands: Commands,
    mut hud: ResMut<HudScene>,
    textures: Res<UnityTextures>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut runs_query: Query<(&mut Transform, &mut Visibility, &mut MeshMaterial3d<UnityLitMaterial>), With<HudRun>>,
) {
    if textures.materials.is_empty() {
        return;
    }
    let hud = &mut *hud;
    let camera = hud.camera;
    hud.queue.clear();
    let mut cache = HashMap::new();
    let anchored = std::mem::take(&mut hud.anchored);
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
            let sorting = hud.sorting_of(object);
            hud.queue.push_frame_sorted(s.texture, geometry, sorting);
        }
    }
    for i in 0..hud.sliced.len() {
        let object = hud.sliced[i].object;
        if !hud.scene.active_in_hierarchy(object, &hud.active) {
            continue;
        }
        let world = hud.world_of(object, &anchored, &mut cache);
        let s = &hud.sliced[i];
        let geometry = sliced_sprite_geometry(&s.def, s.scale, s.dimensions, s.borders, s.anchor, s.border_only, s.color, &world);
        if !geometry.is_empty() {
            let sorting = hud.sorting_of(object);
            hud.queue.push_frame_sorted(s.texture, geometry, sorting);
        }
    }
    for i in 0..hud.texts.len() {
        let object = hud.texts[i].object;
        if hud.texts[i].text.is_empty() || !hud.scene.active_in_hierarchy(object, &hud.active) {
            continue;
        }
        let world = hud.world_of(object, &anchored, &mut cache);
        let t = &hud.texts[i];
        let (Some(font), Some(&texture)) = (hud.fonts.fonts.get(&t.font), hud.font_textures.get(&t.font)) else { continue };
        let formatted = format_text(font, &t.layout, &t.text);
        let local = text_local_geometry(font, &t.layout, &formatted, t.color, t.color2, t.use_gradient);
        let geometry = place(&local, &world);
        if !geometry.is_empty() {
            let sorting = hud.sorting_of(object);
            hud.queue.push_frame_sorted(texture, geometry, sorting);
        }
    }
    hud.anchored = anchored;
    hud.queue.sort(SimVec3::new(camera[0], camera[1], camera[2]));
    let runs = hud.queue.runs(&[], None);
    let base = Vec3::new(camera[0], camera[1], -camera[2]);
    upload_runs(&mut commands, &mut meshes, &textures, &mut hud.pool, &runs, base, |e| {
        e.insert((Hud, HudRun, RenderLayers::layer(UI_LAYER)));
    }, &mut runs_query);
}
