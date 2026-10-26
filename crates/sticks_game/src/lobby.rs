//! The Lobby: the original's title and game set-up, which is one scene (`Lobby.unity`), not a
//! menu UI. Every screen (splash, title, control assignment, game summary, mode select, level
//! select, ...) sits at its own spot of one large paper world; `ActivateExclusively` switches
//! the screen objects and their `CameraPositionInfluencer` pulls the camera (a
//! `CameraPositioner`, as in the levels) across to the active one. Buttons are tk2d text and
//! `PolygonColliderRenderer` shapes with a 3D `BoxCollider` that the mouse clicks
//! (`OnMouseUpAsButton`); Escape presses the visible back button.
//!
//! Everything drawn comes from the scene (sprites, polygon surfaces, tk2d text meshes and their
//! shadows, lit by the camera-borne point light) through the same queue as the levels. What the
//! scripts do is transcribed for the offline game with keyboard and mouse:
//!
//! * `LobbyStartupDirector`, `SplashScreenAdvance`, `NoControllers`, `ActivateExclusively`,
//!   `CameraPositionInfluencer`, `Button` (mouse events and the pressed z tween), `BackButton`,
//!   `ActivateSomethingOnButtonPress`, `ExitButton`;
//! * `NetworkSetup` (offline: "Play Offline"; with no server found on the LAN the "No Network
//!   Games Found" label shows);
//! * `ControlAssignment` with one controller (keyboard and mouse in Player 1; left/right moves
//!   it between columns);
//! * `GameSummary`, `GameModeDisplay`, `LevelNameDisplay`, `DifficultyDisplay`,
//!   `CycleDifficulty`, `GameModeSelect`, `MakeGameModeConsistentWithPlayerCount`,
//!   `SmoothDampTowards`;
//! * `LevelSelect`: index, unlocking (`HighScores.IsLevelUnlocked` with no saved scores) and
//!   the level image.
//!
//! Audio: `PlaySoundOnButtonPress` and the photo pile's "Paper Slide" are queued in
//! [`Lobby::sounds`] for the audio module.
//!
//! Player slots: the keyboard column chosen in control assignment is the local player's slot
//! (`Player.PlayerIndex`); starting a game hands it, the player count and `LobbyTeams`' lists
//! ([`LobbyMemory::teams`], kept like the original's statics) to the [`Session`], from which the
//! level spawns the player (`PlayerSlotAssignments.OnLoadedLevelEverywhere` spawn-point
//! assignment, `PlayerSpawner.Spawn` team and default colours, `Player.SetUp` HUD slot).
//! "Readiness" is the original's: the Level Select's start button loads the level as soon as at
//! least one slot has controls (`ControlAssignment.PlayerCount() >= 1`); there is no per-player
//! ready flag in the game.
//!
//! Not reproduced: the network lobby (`NetworkSetup` hosting/joining, `ServerDiscovery`,
//! `PlayerSlotAssignments`' claims and broadcasts, `RemoteColorAssignments`,
//! `LobbyTeams.SendTeams`, `LobbyClientDisconnection`): Unity's legacy RakNet networking has no
//! counterpart here and the port is offline only. The team arrangement screen (Team Versus
//! needs more local controllers than the one keyboard-and-mouse input supported).
//! TODO: the colour customisation screens and the posed `Man` models, achievements, credits,
//! high scores, the level cards' flip animation and sliding labels, gamepad highlighting.

use std::collections::HashMap;
use std::path::PathBuf;

use bevy::app::AppExit;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use sticks_sim::camera::{CameraPosition, CameraRig};
use sticks_sim::draw::{Geometry, LevelArt, LevelShadow, surface_geometry, surface_shadow_geometry};
use sticks_sim::text::{TextLayout, format_text, place, text_local_geometry};
use unity_compat::Vec3 as SimVec3;
use unity_compat::mathf::smooth_damp;
use unity_data::fonts::FontLibrary;
use unity_data::interactive::{InteractiveScene, ScriptInstance, load_interactive_scene};
use unity_data::level::{PositionerSettings, Shape, Surface};
use unity_data::material::MaterialLibrary;
use unity_data::math::Affine;
use unity_data::sprites::SpriteLibrary;

use crate::camera::{LevelCamera, Rig};
use crate::config::GameConfig;
use crate::level::LevelSource;
use crate::state::{AppState, GameMode, Session};
use crate::unity_render::{FrameQueue, QueueSet, SceneLight, SceneSetup, ShadowTexture, TextureLoad, UnityLitMaterial, UnityTextures, camera_view, extent, surface_style};

/// `Button.highlightedTextColor` and friends.
const NORMAL_TEXT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
const UNAVAILABLE_TEXT: [f32; 4] = [52.0 / 85.0, 52.0 / 85.0, 52.0 / 85.0, 1.0];
/// `Utility.FlashingLabelColor`.
const FLASHING_LABEL: [f32; 4] = [1.0, 0.92156863, 0.015686275, 1.0];
/// `DifficultyDisplay.difficultyNames`.
const DIFFICULTY_NAMES: [&str; 3] = ["Beginner", "Regular", "Expert"];

/// What survives leaving the Lobby (the original keeps these in static fields).
#[derive(Resource)]
pub struct LobbyMemory {
    /// `LobbyStartupDirector.beenHereBefore`.
    been_here_before: bool,
    /// `LobbyGameMode.mode` (its default is the enum's first value).
    pub mode: GameMode,
    /// `LobbyDifficulty.difficultyLevel` (default 0).
    pub difficulty: i32,
    /// `LobbyLevelSelect.index`.
    pub level_index: usize,
    /// Column of the keyboard-and-mouse input in control assignment: 0..=3 players, 4 unused.
    pub keyboard_slot: usize,
    /// `LobbyTeams`' static team lists.
    pub teams: sticks_sim::lobby::LobbyTeams,
}

impl Default for LobbyMemory {
    fn default() -> Self {
        LobbyMemory { been_here_before: false, mode: GameMode::Campaign, difficulty: 0, level_index: 0, keyboard_slot: 0, teams: sticks_sim::lobby::LobbyTeams::default() }
    }
}

impl LobbyMemory {
    /// `ControlAssignment.PlayerCount`.
    pub fn player_count(&self) -> usize {
        usize::from(self.keyboard_slot < 4)
    }
}

/// `LobbyLevelSelect.campaignLevels` / `versusLevels` (from the Permanent System scene) and
/// `LevelSelect.campaignLevelImages` / `versusLevelImages`.
struct LevelLists {
    campaign: Vec<String>,
    versus: Vec<String>,
    campaign_images: Vec<u32>,
    versus_images: Vec<u32>,
}

impl LevelLists {
    fn levels(&self, mode: GameMode) -> &[String] {
        if mode == GameMode::Campaign { &self.campaign } else { &self.versus }
    }
}

/// `LobbyLevelSelect.GetLevelName`.
pub fn level_display_name(id: &str) -> &str {
    match id {
        "Skyline" => "Skyline (Tutorial)",
        "Moon" => "Apollo",
        "Moon Race" => "Space Race",
        "Skyline 2" => "Skyline Redux",
        "Office" => "Headquarters",
        "Skyline VS" => "Skyline",
        "Japan VS" => "Japan",
        "Treetops VS" => "Treetops",
        "Japan" | "Pirates" | "Treetops" | "Corruption 2" | "Platforms" => id,
        _ => "Unknown",
    }
}

enum Art {
    Sprite { geometry: Geometry, texture: u32 },
    SpriteShadow(LevelShadow),
    Surface { surface: Surface, geometry: Geometry, texture: u32 },
    Text(usize),
}

struct Renderable {
    object: usize,
    art: Art,
}

/// A `tk2dTextMesh` of the scene with what scripts change at run time.
struct TextItem {
    object: usize,
    font: String,
    text: String,
    color: [f32; 4],
    color2: [f32; 4],
    use_gradient: bool,
    layout: TextLayout,
    /// Shadow copies: the text they copy, their font and shadow plane.
    shadow_of: Option<(usize, Option<String>, f32)>,
}

struct ButtonState {
    object: usize,
    /// `Button.background`: the "Background" child, or the button itself.
    background: usize,
    text: Option<usize>,
    unhighlighted: [f32; 4],
    pressed_z: f32,
    tween_velocity: f32,
    mouse_down: bool,
    mouse_entered: bool,
}

/// A `SmoothDampTowards` mover (local position, target, velocity).
struct Mover {
    object: usize,
    smooth_time: f32,
    target: [f32; 3],
    velocity: [f32; 3],
    /// `SmoothDampTowards.enabled` (`ControlAssignment` switches it off while a card is dragged).
    enabled: bool,
}

/// A label `LevelSelect.Awake` instantiates from a prototype (one per level: name, number, top
/// score), moved by its own `SmoothDampTowards`.
struct SlidingLabel {
    /// Text item of the prototype (font, layout, colour, shadow).
    prototype: usize,
    /// The prototype object (its parent, rotation and scale are the label's).
    prototype_object: usize,
    text: String,
    local: [f32; 3],
    velocity: [f32; 3],
    target: [f32; 3],
    smooth_time: f32,
}

/// `LevelSelect`'s state after `Awake`.
struct LevelSelectState {
    /// The object carrying `LevelSelect` (its transform places the "Paper Slide" sound).
    object: Option<usize>,
    cards: Vec<usize>,
    card_images: Vec<Option<usize>>,
    resting_positions: Vec<[f32; 2]>,
    resting_z: Vec<f32>,
    resting_rotations: Vec<[f32; 4]>,
    overlays_standard: Vec<Option<usize>>,
    overlays_starter: Vec<Option<usize>>,
    stack: Vec<usize>,
    current: i32,
    last: i32,
    animating: bool,
    animating_out: bool,
    animating_forward: bool,
    animation_time: f32,
    animation_position: [f32; 2],
    forward_destination: [f32; 2],
    backward_destination: [f32; 2],
    last_seen_index: i32,
    was_active: bool,
    campaign_names: Vec<usize>,
    versus_names: Vec<usize>,
    numbers: Vec<usize>,
    scores: Vec<usize>,
    name_selected: [f32; 3],
    name_offscreen: [f32; 3],
    number_selected: [f32; 3],
    number_offscreen: [f32; 3],
    score_selected: [f32; 3],
    stars: Vec<usize>,
    star_offscreen: [f32; 3],
    high_scores: Option<usize>,
    trailer: Option<usize>,
    upgrade: Option<usize>,
    buttons_offscreen: [f32; 3],
    trailer_enabled: [f32; 3],
    upgrade_enabled: [f32; 3],
    count_label: Option<usize>,
}

/// A `Draggable` (the input cards of Control Assignment).
struct DragState {
    object: usize,
    /// `OnMouseDown` .. `OnMouseUp`.
    mouse_down: bool,
    was_mouse_down: bool,
    offset: [f32; 2],
    tween_velocity: f32,
    dropped: bool,
}

/// A label flashing between two colours (`GameModeSelect.FlashBackAndForth`).
struct Flash {
    text: usize,
    from: [f32; 4],
    to: [f32; 4],
    time: f32,
}

#[derive(Resource)]
pub struct Lobby {
    /// Sounds the scripts asked MasterAudio for this frame (`PlaySound3DAtTransformAndForget`:
    /// group, object position); the audio module plays and clears them.
    pub(crate) sounds: Vec<(String, SimVec3)>,
    scene: InteractiveScene,
    /// `activeSelf` of every object.
    active: Vec<bool>,
    /// Local position of every object (scripts move some).
    local: Vec<[f32; 3]>,
    art: Vec<Renderable>,
    texts: Vec<TextItem>,
    text_of_object: HashMap<usize, usize>,
    buttons: Vec<ButtonState>,
    movers: Vec<Mover>,
    flashes: Vec<Flash>,
    fonts: FontLibrary,
    font_textures: HashMap<String, u32>,
    levels: LevelLists,
    /// `SplashScreenAdvance.timeElapsed`.
    splash_time: f32,
    /// Object under the mouse, and the one pressed on.
    hovered: Option<usize>,
    pressed_on: Option<usize>,
    /// `LevelSelect.displayLevelIndex` (raw; read through `LobbyLevelSelect.Clamp`).
    display_level: i32,
    /// The cards' "Image" surfaces, and the level image they show (`cardMaterials[i].mainTexture`).
    /// `cardMaterials[i].mainTexture` of each card's "Image".
    card_textures: HashMap<usize, u32>,
    /// Rotations set at run time (the cards take the resting rotation of their place).
    rotation_override: HashMap<usize, [f32; 4]>,
    level_select: Option<LevelSelectState>,
    labels: Vec<SlidingLabel>,
    /// `ControlAssignment.regularPlayerHeadingColor` / `deactivatedPlayerHeadingColor`, taken from
    /// the headings in `Start` (before anything recolours them).
    heading_colors: Option<([f32; 4], [f32; 4], [f32; 4])>,
    /// The `Draggable` the mouse went down on, and where the mouse is on the z = 0 plane
    /// (`ScreenToWorldPoint(mouse, |camera z|)`).
    drag: Option<DragState>,
    mouse_world: Option<[f32; 2]>,
}

impl Lobby {
    fn active_in_hierarchy(&self, object: usize) -> bool {
        self.scene.active_in_hierarchy(object, &self.active)
    }

    /// World transform with the run-time local positions (lobby objects are not rotated or
    /// scaled along the way, so local moves are world moves).
    fn world(&self, object: usize) -> Affine {
        if !self.rotation_override.is_empty() && self.chain_rotated(object) {
            // Recompose from the local transforms (a rotation changed somewhere up the chain).
            let o = &self.scene.objects[object];
            let parent = o.parent.map_or(Affine::IDENTITY, |p| self.world(p));
            let r = self.rotation_override.get(&object).copied().unwrap_or(o.local_rotation);
            return parent.mul(&Affine::from_trs(self.local[object], (r[0], r[1], r[2], r[3]), o.local_scale));
        }
        let mut world = self.scene.objects[object].world;
        let mut current = Some(object);
        while let Some(i) = current {
            let o = &self.scene.objects[i];
            for k in 0..3 {
                world.t[k] += self.local[i][k] - o.local_position[k];
            }
            current = o.parent;
        }
        world
    }

    fn chain_rotated(&self, object: usize) -> bool {
        let mut current = Some(object);
        while let Some(i) = current {
            if self.rotation_override.contains_key(&i) {
                return true;
            }
            current = self.scene.objects[i].parent;
        }
        false
    }

    fn world_position(&self, object: usize) -> SimVec3 {
        let t = self.world(object).t;
        SimVec3::new(t[0], t[1], t[2])
    }

    fn named_script(&self, class_name: &'static str) -> Option<&ScriptInstance> {
        self.scene.scripts_named(class_name).next()
    }

    fn field(&self, class_name: &'static str, field: &str) -> Option<usize> {
        self.named_script(class_name).and_then(|s| self.scene.field_object(s, field))
    }

    /// `GameObject.SetActive(true)`, with `ActivateExclusively.OnEnable`.
    fn activate(&mut self, object: usize) {
        debug!("activate {:?}", self.scene.objects[object].name);
        if self.scene.script(object, "ActivateExclusively").is_some() {
            for s in self.scene.scripts_named("ActivateExclusively") {
                self.active[s.object] = false;
            }
        }
        self.active[object] = true;
    }

    fn set_text(&mut self, object: usize, text: &str) {
        if let Some(&t) = self.text_of_object.get(&object) {
            if self.texts[t].text != text {
                self.texts[t].text = text.to_owned();
            }
        }
    }

    fn set_text_color(&mut self, object: usize, color: [f32; 4]) {
        if let Some(&t) = self.text_of_object.get(&object) {
            self.texts[t].color = color;
        }
    }

    fn button_text(&self, button: usize) -> String {
        self.scene.child(button, "Text").and_then(|t| self.text_of_object.get(&t)).map(|&t| self.texts[t].text.clone()).unwrap_or_default()
    }

    fn button_index(&self, object: usize) -> Option<usize> {
        self.buttons.iter().position(|b| b.object == object)
    }

    /// `Button.IsVisible`: its text (or background) renderer is drawn by the camera.
    fn button_visible(&self, button: usize, view: &sticks_sim::draw::View) -> bool {
        let b = &self.buttons[button];
        let probe = b.text.unwrap_or(b.background);
        self.active_in_hierarchy(probe) && view.may_see(self.world_position(probe), 1.0)
    }
}

pub struct LobbyPlugin;

impl Plugin for LobbyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LobbyMemory>()
            .init_resource::<LobbyClicks>()
            .add_systems(OnEnter(AppState::Lobby), enter_lobby)
            .add_systems(OnExit(AppState::Lobby), leave_lobby)
            .add_systems(Update, (lobby_mouse, lobby_update, lobby_camera).chain().run_if(in_state(AppState::Lobby)).before(QueueSet::Light))
            .add_systems(Update, queue_lobby.in_set(QueueSet::Collect).run_if(in_state(AppState::Lobby)));
    }
}

/// The Lobby scene's camera.
#[derive(Component)]
struct LobbyCamera;

/// Load the scene and build its renderables (`Awake`/`Start` of everything that draws).
#[allow(clippy::too_many_arguments)]
fn enter_lobby(
    mut commands: Commands,
    config: Res<GameConfig>,
    mut memory: ResMut<LobbyMemory>,
    asset_server: Res<AssetServer>,
    mut textures: ResMut<UnityTextures>,
    mut materials: ResMut<Assets<UnityLitMaterial>>,
    mut images: ResMut<Assets<Image>>,
) {
    let root = &config.export_root;
    let scene = load_interactive_scene(root, "Assets/Game/Lobby/Lobby.unity").unwrap_or_else(|e| panic!("Lobby.unity: {e}"));
    let assets_dir = config.assets_dir();
    let mut load = TextureLoad::new(&assets_dir, &asset_server, &mut images, &mut materials);

    // Sprites and their shadow copies.
    let mut libraries: HashMap<String, SpriteLibrary> = HashMap::new();
    for sprite in &scene.sprites {
        if !libraries.contains_key(&sprite.collection) {
            if let Ok(lib) = SpriteLibrary::load(&assets_dir, &sprite.collection) {
                libraries.insert(sprite.collection.clone(), lib);
            }
        }
    }
    let sprite_art = LevelArt::build(&scene.sprites, |_| false, |g| libraries.get(g), |p| textures.id(p, &mut load));
    if !sprite_art.moving.is_empty() {
        warn!("lobby: {} camera-moved sprites are not drawn", sprite_art.moving.len());
    }
    let mut art = Vec::new();
    for (sprite, &source) in sprite_art.sprites.into_iter().zip(&sprite_art.sprite_sources) {
        if let Some(object) = scene.object_of(scene.sprites[source].object) {
            art.push(Renderable { object, art: Art::Sprite { geometry: sprite.geometry, texture: sprite.texture } });
        }
    }
    for (shadow, &source) in sprite_art.shadows.into_iter().zip(&sprite_art.shadow_sources) {
        if let Some(object) = scene.object_of(scene.sprites[source].object) {
            art.push(Renderable { object, art: Art::SpriteShadow(shadow) });
        }
    }
    // `PolygonColliderRenderer` surfaces.
    let library = MaterialLibrary::scan(&assets_dir);
    for collider in &scene.colliders {
        let (Some(surface), Shape::Polygon(_), Some(object)) = (&collider.surface, &collider.shape, scene.object_of(collider.object)) else { continue };
        let (texture, color, st) = surface_style(surface, &library, &mut textures, &mut load);
        let mut geometry = surface_geometry(surface, color);
        sticks_sim::draw::apply_texture_transform(&mut geometry, st[0], st[1]);
        if !geometry.is_empty() {
            art.push(Renderable { object, art: Art::Surface { surface: surface.clone(), geometry, texture } });
        }
    }
    let white = textures.polygon_shadow(&mut load);
    debug!(
        "lobby surfaces: {} of {} colliders",
        art.iter().filter(|r| matches!(r.art, Art::Surface { .. })).count(),
        scene.colliders.len()
    );
    // Text meshes.
    let mut fonts = FontLibrary::new(&assets_dir);
    let mut font_textures = HashMap::new();
    let mut texts: Vec<TextItem> = Vec::new();
    let mut text_of_object = HashMap::new();
    let mut item_of_scene_text: Vec<Option<usize>> = Vec::new();
    for t in &scene.texts {
        let Some(object) = scene.object_of(t.object) else {
            item_of_scene_text.push(None);
            continue;
        };
        item_of_scene_text.push(Some(texts.len()));
        for guid in std::iter::once(&t.font).chain(t.shadow_of.as_ref().and_then(|s| s.1.as_ref())) {
            if !font_textures.contains_key(guid) {
                if let Some(path) = fonts.get(guid).and_then(|f| f.texture.clone()) {
                    font_textures.insert(guid.clone(), textures.id(&path, &mut load));
                }
            }
        }
        if t.shadow_of.is_none() {
            text_of_object.insert(object, texts.len());
        }
        art.push(Renderable { object, art: Art::Text(texts.len()) });
        texts.push(TextItem {
            object,
            font: t.font.clone(),
            text: t.text.clone(),
            color: t.color,
            color2: t.color2,
            use_gradient: t.use_gradient,
            layout: TextLayout::of(t),
            shadow_of: t.shadow_of.clone(),
        });
    }
    // Shadow copies refer to scene text indices; turn those into item indices.
    for item in &mut texts {
        if let Some(shadow) = item.shadow_of.as_mut() {
            shadow.0 = item_of_scene_text.get(shadow.0).copied().flatten().unwrap_or(usize::MAX);
        }
    }

    // Buttons.
    let mut buttons = Vec::new();
    for s in scene.scripts_named("Button") {
        let object = s.object;
        let background = scene.child(object, "Background").unwrap_or(object);
        let text = scene.child(object, "Text");
        let unhighlighted = text.and_then(|t| text_of_object.get(&t)).map_or(NORMAL_TEXT, |&i: &usize| texts[i].color);
        let pressed_z = -0.04 - scene.objects[background].local_position[2];
        buttons.push(ButtonState { object, background, text, unhighlighted, pressed_z, tween_velocity: 0.0, mouse_down: false, mouse_entered: false });
    }
    let movers = scene
        .scripts_named("SmoothDampTowards")
        .map(|s| Mover {
            object: s.object,
            smooth_time: s.fields.f32_of("smoothTime").unwrap_or(0.1),
            target: scene.objects[s.object].local_position,
            velocity: [0.0; 3],
            enabled: true,
        })
        .collect();

    // Level lists (Permanent System scene) and images.
    let permanent = load_interactive_scene(root, "Assets/Game/Permanent System.unity").ok();
    let names = |key: &str| -> Vec<String> {
        permanent
            .as_ref()
            .and_then(|p| p.scripts_named("LobbyLevelSelect").next().cloned())
            .and_then(|s| s.fields.get(key).map(|v| v.as_seq().iter().filter_map(|n| n.as_str().map(str::to_owned)).collect()))
            .unwrap_or_default()
    };
    let image_ids = |key: &str, textures: &mut UnityTextures, load: &mut TextureLoad<'_, '_>| -> Vec<u32> {
        scene
            .scripts_named("LevelSelect")
            .next()
            .and_then(|s| s.fields.get(key))
            .map(|v| v.as_seq().iter().filter_map(|t| t.guid().and_then(|g| library.texture(g))).map(|p| textures.id(&p, load)).collect())
            .unwrap_or_default()
    };
    let levels = LevelLists {
        campaign: names("campaignLevels"),
        versus: names("versusLevels"),
        campaign_images: image_ids("campaignLevelImages", &mut textures, &mut load),
        versus_images: image_ids("versusLevelImages", &mut textures, &mut load),
    };
    info!(
        "lobby: {} objects, {} renderers, {} texts, {} buttons, {} campaign and {} versus levels",
        scene.objects.len(),
        art.len(),
        texts.len(),
        buttons.len(),
        levels.campaign.len(),
        levels.versus.len()
    );

    // The camera (`CameraPositioner`) with the Lobby's settings and the light under it.
    let camera = scene.level.camera.clone().expect("the Lobby has a camera");
    let [r, g, b, _] = camera.background;
    commands.insert_resource(ClearColor(Color::srgb(r, g, b)));
    let settings = camera.positioner.clone().unwrap_or_else(PositionerSettings::default);
    let [x, y, z] = camera.position;
    commands.spawn((
        Name::new("Lobby camera"),
        LobbyCamera,
        LevelCamera,
        Rig(CameraRig::new(settings, camera.field_of_view, CameraPosition { x, y, z })),
        Camera3d::default(),
        bevy::core_pipeline::tonemapping::Tonemapping::None,
        Projection::Perspective(PerspectiveProjection { fov: camera.field_of_view.to_radians(), near: camera.near, far: camera.far, ..default() }),
        Transform::from_xyz(x, y, -z),
    ));
    let setup = SceneSetup { light: scene.level.light_setup.clone(), ambient: scene.level.ambient };
    commands.insert_resource(SceneLight::new(&setup));
    commands.insert_resource(setup);
    commands.insert_resource(ShadowTexture(white));

    let active = scene.objects.iter().map(|o| o.active_self).collect();
    let local = scene.objects.iter().map(|o| o.local_position).collect();
    let display_level = memory.level_index as i32;
    let mut lobby = Lobby {
        sounds: Vec::new(),
        scene,
        active,
        local,
        art,
        texts,
        text_of_object,
        buttons,
        movers,
        flashes: Vec::new(),
        fonts,
        font_textures,
        levels,
        splash_time: 0.0,
        hovered: None,
        pressed_on: None,
        display_level,
        card_textures: HashMap::new(),
        rotation_override: HashMap::new(),
        level_select: None,
        labels: Vec::new(),
        heading_colors: None,
        drag: None,
        mouse_world: None,
    };

    // `GameModeSelect.Start` captions.
    if let Some(s) = lobby.named_script("GameModeSelect").cloned() {
        for (field, text) in [
            ("campaignCaption", "Play co-operatively with 1-4 players"),
            ("versusCaption", "For 2-4 players, 10 frags to win"),
            ("teamVersusCaption", "For 3-4 players, 10 team frags to win"),
        ] {
            if let Some(o) = lobby.scene.field_object(&s, field) {
                lobby.set_text(o, text);
            }
        }
    }
    // `LobbyStartupDirector.Start`.
    let first = if memory.been_here_before { "summaryScreen" } else { "splashScreen" };
    if let Some(screen) = lobby.field("LobbyStartupDirector", first) {
        lobby.activate(screen);
    }
    memory.been_here_before = true;
    commands.insert_resource(lobby);
}

fn leave_lobby(mut commands: Commands, cameras: Query<Entity, With<LobbyCamera>>) {
    for entity in &cameras {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Lobby>();
}

/// Inverse of an affine transform (for picking in a box collider's local space).
fn inverse(a: &Affine) -> Option<Affine> {
    let m = a.m;
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let c = |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let inv = [
        [c(1, 1, 2, 2) / det, -c(0, 1, 2, 2) / det, c(0, 1, 1, 2) / det],
        [-c(1, 0, 2, 2) / det, c(0, 0, 2, 2) / det, -c(0, 0, 1, 2) / det],
        [c(1, 0, 2, 1) / det, -c(0, 0, 2, 1) / det, c(0, 0, 1, 1) / det],
    ];
    let t = [0, 1, 2].map(|r| -(inv[r][0] * a.t[0] + inv[r][1] * a.t[1] + inv[r][2] * a.t[2]));
    Some(Affine { m: inv, t })
}

/// Ray parameter where the ray enters the box `center ± size/2` (slab test).
fn ray_box(origin: [f32; 3], direction: [f32; 3], center: [f32; 3], size: [f32; 3]) -> Option<f32> {
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for k in 0..3 {
        let (lo, hi) = (center[k] - size[k] * 0.5, center[k] + size[k] * 0.5);
        if direction[k].abs() < 1e-12 {
            if origin[k] < lo || origin[k] > hi {
                return None;
            }
            continue;
        }
        let (a, b) = ((lo - origin[k]) / direction[k], (hi - origin[k]) / direction[k]);
        t0 = t0.max(a.min(b));
        t1 = t1.min(a.max(b));
        if t0 > t1 {
            return None;
        }
    }
    Some(t0)
}

/// Unity's `OnMouse*` events: the nearest `BoxCollider` of an active object under the cursor gets
/// `OnMouseEnter`/`Exit`/`Down`/`Up`; `OnMouseUpAsButton` when the button is released over the
/// object it went down on.
fn lobby_mouse(
    mut lobby: ResMut<Lobby>,
    buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    cameras: Query<(&Camera, &GlobalTransform), With<LobbyCamera>>,
    mut pressed: Local<Vec<usize>>,
    mut clicks: ResMut<LobbyClicks>,
    scripted: Res<crate::devscript::ScriptedCursor>,
) {
    let lobby = &mut *lobby;
    let ray = scripted.0.or_else(|| windows.single().ok().and_then(|w| w.cursor_position())).and_then(|cursor| {
        let (camera, transform) = cameras.single().ok()?;
        camera.viewport_to_world(transform, cursor).ok()
    });
    let mut hit: Option<(f32, usize)> = None;
    if let Some(ray) = ray {
        // Back to game space (z negated).
        let o = [ray.origin.x, ray.origin.y, -ray.origin.z];
        let d = [ray.direction.x, ray.direction.y, -ray.direction.z];
        for b in &lobby.scene.boxes {
            if !b.enabled || !lobby.active_in_hierarchy(b.object) {
                continue;
            }
            let Some(inv) = inverse(&lobby.world(b.object)) else { continue };
            let lo = inv.transform_point(o);
            let ld = inv.transform_vector(d);
            if let Some(t) = ray_box(lo, ld, b.center, b.size) {
                if hit.is_none_or(|(best, _)| t < best) {
                    hit = Some((t, b.object));
                }
            }
        }
    }
    let now = hit.map(|h| h.1);
    lobby.mouse_world = ray.and_then(|ray| {
        let (oz, dz) = (-ray.origin.z, -ray.direction.z);
        if dz.abs() < 1e-6 {
            return None;
        }
        let t = (0.0 - oz) / dz;
        Some([ray.origin.x + ray.direction.x * t, ray.origin.y + ray.direction.y * t])
    });
    if buttons.just_pressed(MouseButton::Left) {
        if let Some(object) = now.filter(|&o| lobby.scene.script(o, "Draggable").is_some()) {
            match lobby.drag.as_mut().filter(|d| d.object == object) {
                Some(d) => d.mouse_down = true,
                None => lobby.drag = Some(DragState { object, mouse_down: true, was_mouse_down: false, offset: [0.0, 0.0], tween_velocity: 0.0, dropped: false }),
            }
        }
    }
    if buttons.just_released(MouseButton::Left) {
        if let Some(d) = lobby.drag.as_mut() {
            d.mouse_down = false;
        }
    }
    let previous = lobby.hovered;
    if previous != now {
        if let Some(i) = previous.and_then(|o| lobby.button_index(o)) {
            lobby.buttons[i].mouse_entered = false;
        }
        if let Some(i) = now.and_then(|o| lobby.button_index(o)) {
            lobby.buttons[i].mouse_entered = true;
        }
        lobby.hovered = now;
    }
    if buttons.just_pressed(MouseButton::Left) {
        lobby.pressed_on = now;
        if let Some(i) = now.and_then(|o| lobby.button_index(o)) {
            lobby.buttons[i].mouse_down = true;
        }
    }
    pressed.clear();
    if buttons.just_released(MouseButton::Left) {
        if let Some(down) = lobby.pressed_on.take() {
            if let Some(i) = lobby.button_index(down) {
                lobby.buttons[i].mouse_down = false;
            }
            if now == Some(down) {
                pressed.push(down);
            }
        }
    }
    clicks.0.extend(pressed.iter().copied());
}

/// Buttons clicked this frame (`OnMouseUpAsButton` → `SimulatePress`), handed from the mouse
/// system to the update in the same frame.
#[derive(Resource, Default)]
struct LobbyClicks(Vec<usize>);

/// The frame's `Update` of every Lobby script.
#[allow(clippy::too_many_arguments)]
fn lobby_update(
    mut lobby: ResMut<Lobby>,
    mut memory: ResMut<LobbyMemory>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rigs: Query<&Rig, With<LobbyCamera>>,
    mut session: ResMut<Session>,
    mut source: ResMut<LevelSource>,
    config: Res<GameConfig>,
    mut next: ResMut<NextState<AppState>>,
    mut exit: MessageWriter<AppExit>,
    mut clicks: ResMut<LobbyClicks>,
) {
    let dt = time.delta_secs().min(1.0 / 3.0);
    let lobby = &mut *lobby;
    // `KeyboardAndMouseLobbyInput.ProcessInput` (the only lobby input without gamepads).
    let any_pressed = keys.get_just_pressed().next().is_some() || mouse.get_just_pressed().next().is_some();
    // `Keyboard.LeftPressedThisFrame` / `RightPressedThisFrame`: `GetKeyDown(KeyCode.A / D)`.
    let move_left = keys.just_pressed(KeyCode::KeyA);
    let move_right = keys.just_pressed(KeyCode::KeyD);
    let back = keys.just_pressed(KeyCode::Escape);
    let view = rigs.single().ok().map(|rig| camera_view(rig, &windows));

    // Presses: mouse clicks, and Escape on a visible `BackButton`.
    let mut presses: Vec<usize> = std::mem::take(&mut clicks.0);
    if back {
        if let Some(view) = &view {
            for s in lobby.scene.scripts_named("BackButton") {
                if let Some(i) = lobby.button_index(s.object) {
                    if lobby.button_visible(i, view) {
                        presses.push(s.object);
                    }
                }
            }
        }
    }
    presses.retain(|&o| lobby.active_in_hierarchy(o));
    for object in presses {
        press(lobby, &mut memory, object, &mut session, &mut source, &config, &mut next, &mut exit);
    }

    // `SplashScreenAdvance.Update` → `NoControllers.Update` (the keyboard is always available).
    if let Some(splash) = lobby.named_script("SplashScreenAdvance").cloned() {
        if lobby.active_in_hierarchy(splash.object) {
            lobby.splash_time += dt;
            let limit = splash.fields.f32_of("timeLimit").unwrap_or(3.0);
            if any_pressed || lobby.splash_time > limit {
                if let Some(next_screen) = lobby.scene.field_object(&splash, "noControllers").and_then(|nc| lobby.scene.script(nc, "NoControllers").cloned()).and_then(|nc| lobby.scene.field_object(&nc, "nextScreen")) {
                    lobby.activate(next_screen);
                }
            }
        }
    }

    // `NetworkSetup.Update`, offline with no server on the LAN.
    if let Some(s) = lobby.named_script("NetworkSetup").cloned() {
        if lobby.active_in_hierarchy(s.object) {
            for field in ["joinNetworkGameButton", "showNetworkGamesButton", "connectToWiFiLabel", "connectToWiFiStarterEditionlabel"] {
                if let Some(o) = lobby.scene.field_object(&s, field) {
                    lobby.active[o] = false;
                }
            }
            if let Some(o) = lobby.scene.field_object(&s, "noNetworkGamesFoundLabel") {
                lobby.active[o] = true;
            }
            // The Steam build is the full version: no lock on "Start Network Game".
            if let Some(lock) = lobby.scene.field_object(&s, "startNetworkGameButton").and_then(|b| lobby.scene.child(b, "Lock")) {
                lobby.active[lock] = false;
            }
        }
    }

    // `ControlAssignment` with the keyboard and mouse as the only input.
    if let Some(s) = lobby.named_script("ControlAssignment").cloned() {
        if lobby.active_in_hierarchy(s.object) {
            if move_left && memory.keyboard_slot > 0 {
                memory.keyboard_slot -= 1;
            }
            if move_right && memory.keyboard_slot < 4 {
                memory.keyboard_slot += 1;
            }
            drag_update(lobby, dt);
            update_control_assignment(lobby, &s, &mut memory.keyboard_slot);
        }
    }

    // `LobbyEnder.Update`, offline (`NetworkPeerType.Disconnected`): only "Back".
    for s in lobby.scene.scripts_named("LobbyEnder").cloned().collect::<Vec<_>>() {
        for (field, on) in [("backButton", true), ("stopServerButton", false), ("disconnectButton", false)] {
            if let Some(o) = lobby.scene.field_object(&s, field) {
                lobby.active[o] = on;
            }
        }
    }

    // `MakeGameModeConsistentWithPlayerCount.Update`.
    let players = memory.player_count();
    if memory.mode == GameMode::TeamVersus && players < 3 {
        memory.mode = GameMode::Versus;
    }
    if memory.mode == GameMode::Versus && players < 2 {
        memory.mode = GameMode::Campaign;
    }

    // `GameModeSelect.Update`: availability colours (no Starter Edition locks).
    if let Some(s) = lobby.named_script("GameModeSelect").cloned() {
        for icon in lobby.scene.field_objects(&s, "lockIcons").into_iter().flatten() {
            lobby.active[icon] = false;
        }
        for (field, available) in [("versusButton", players > 1), ("teamVersusButton", players > 2)] {
            if let Some(i) = lobby.scene.field_object(&s, field).and_then(|b| lobby.button_index(b)) {
                lobby.buttons[i].unhighlighted = if available { NORMAL_TEXT } else { UNAVAILABLE_TEXT };
            }
        }
    }

    // Displays on the summary screen.
    if let Some(s) = lobby.named_script("GameModeDisplay").cloned() {
        let button = match memory.mode {
            GameMode::Campaign => "campaignButton",
            GameMode::Versus => "versusButton",
            GameMode::TeamVersus => "teamVersusButton",
        };
        let text = lobby.scene.field_object(&s, button).map(|b| lobby.button_text(b)).unwrap_or_default();
        if let Some(label) = lobby.scene.field_object(&s, "label") {
            lobby.set_text(label, &text);
        }
        if let Some(row) = lobby.scene.field_object(&s, "difficultyRow") {
            lobby.active[row] = memory.mode == GameMode::Campaign;
        }
        if let Some(row) = lobby.scene.field_object(&s, "teamsRow") {
            lobby.active[row] = memory.mode == GameMode::TeamVersus;
        }
    }
    let levels = lobby.levels.levels(memory.mode).to_vec();
    memory.level_index = memory.level_index.min(levels.len().saturating_sub(1));
    let level_id = levels.get(memory.level_index).cloned().unwrap_or_default();
    for s in lobby.scene.scripts_named("LevelNameDisplay").cloned().collect::<Vec<_>>() {
        if let Some(label) = lobby.scene.field_object(&s, "label") {
            lobby.set_text(label, level_display_name(&level_id));
        }
    }
    for s in lobby.scene.scripts_named("DifficultyDisplay").cloned().collect::<Vec<_>>() {
        let index = (memory.difficulty + 1).clamp(0, 2) as usize;
        if let Some(label) = lobby.scene.field_object(&s, "label") {
            lobby.set_text(label, DIFFICULTY_NAMES[index]);
        }
        // Stars: the selected one moves to star 1's place, the others to star 0's.
        let stars: Vec<usize> = lobby.scene.field_objects(&s, "stars").into_iter().flatten().collect();
        if stars.len() >= 2 {
            let deselected = lobby.scene.objects[stars[0]].local_position;
            let selected = lobby.scene.objects[stars[1]].local_position;
            for (i, &star) in stars.iter().enumerate() {
                if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == star) {
                    m.target = if i == index { selected } else { deselected };
                }
            }
        }
    }

    // `LevelSelect.Awake` / `OnEnable` / `Update`.
    if let Some(s) = lobby.named_script("LevelSelect").cloned() {
        let active = lobby.active_in_hierarchy(s.object);
        if active && lobby.level_select.is_none() {
            level_select_awake(lobby, &s);
        }
        if let Some(mut state) = lobby.level_select.take() {
            if active {
                if !state.was_active {
                    // `OnEnable`.
                    state.last_seen_index = -1;
                }
                level_select_update(lobby, &mut state, &mut memory, move_left, move_right, dt);
            }
            state.was_active = active;
            lobby.level_select = Some(state);
        }
    }

    // `Button.Update`: the text colour and the pressed z tween.
    for i in 0..lobby.buttons.len() {
        let (object, down, target_down, text, colour) = {
            let b = &lobby.buttons[i];
            (b.object, b.mouse_down && b.mouse_entered, b.pressed_z, b.text, b.unhighlighted)
        };
        if !lobby.active_in_hierarchy(object) {
            let b = &mut lobby.buttons[i];
            b.mouse_down = false;
            b.mouse_entered = false;
            continue;
        }
        let z = lobby.local[object][2];
        let mut velocity = lobby.buttons[i].tween_velocity;
        let target = if down { target_down } else { 0.0 };
        lobby.local[object][2] = smooth_damp(z, target, &mut velocity, 0.025, f32::INFINITY, dt);
        lobby.buttons[i].tween_velocity = velocity;
        if let Some(t) = text {
            if !lobby.flashes.iter().any(|f| lobby.texts[f.text].object == t) {
                lobby.set_text_color(t, colour);
            }
        }
    }

    // `SmoothDampTowards.Update`.
    for k in 0..lobby.movers.len() {
        let object = lobby.movers[k].object;
        if !lobby.movers[k].enabled || !lobby.active_in_hierarchy(object) {
            continue;
        }
        let m = &mut lobby.movers[k];
        let p = lobby.local[m.object];
        let mut out = p;
        for k in 0..3 {
            out[k] = smooth_damp(p[k], m.target[k], &mut m.velocity[k], m.smooth_time, f32::INFINITY, dt);
        }
        lobby.local[m.object] = out;
    }

    for k in 0..lobby.labels.len() {
        let parent = lobby.scene.objects[lobby.labels[k].prototype_object].parent;
        if parent.is_some_and(|p| !lobby.active_in_hierarchy(p)) {
            continue;
        }
        let l = &mut lobby.labels[k];
        for a in 0..3 {
            l.local[a] = smooth_damp(l.local[a], l.target[a], &mut l.velocity[a], l.smooth_time, f32::INFINITY, dt);
        }
    }

    // `FlashBackAndForth`: start → mid over 0.15 s, then back.
    lobby.flashes.retain_mut(|f| {
        f.time += dt;
        f.time < 0.3
    });
    for f in &lobby.flashes {
        let (a, b, t) = if f.time < 0.15 { (f.from, f.to, f.time / 0.15) } else { (f.to, f.from, (f.time - 0.15) / 0.15) };
        let c = [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t.clamp(0.0, 1.0));
        let item = f.text;
        lobby.texts[item].color = c;
    }
}

/// `HighScores.IsLevelUnlocked` with no scores saved: the first level, and every versus level.
/// TODO: saved high scores and `RemoteLevelAvailability` unlock further campaign levels.
fn level_unlocked(index: usize, mode: GameMode) -> bool {
    index == 0 || mode != GameMode::Campaign
}

/// `Draggable.Update`: a card held by the mouse follows it (keeping the grab offset) and lifts
/// towards z -0.13; `WasDroppedThisFrame` on the frame it is let go.
/// TODO(UNCERTAIN): `Draggable.Update` is taken to run before `ControlAssignment.Update`.
fn drag_update(lobby: &mut Lobby, dt: f32) {
    let mouse = lobby.mouse_world;
    let Some(object) = lobby.drag.as_ref().map(|d| d.object) else { return };
    let world = lobby.world(object).t;
    let mut d = lobby.drag.take().unwrap();
    if d.mouse_down {
        if let Some(m) = mouse {
            if !d.was_mouse_down {
                // `-transform.InverseTransformPoint(mouse)` (cards are not rotated or scaled).
                d.offset = [world[0] - m[0], world[1] - m[1]];
            } else {
                let target = [m[0] + d.offset[0], m[1] + d.offset[1]];
                lobby.local[object][0] += target[0] - world[0];
                lobby.local[object][1] += target[1] - world[1];
            }
        }
    }
    d.dropped = !d.mouse_down && d.was_mouse_down;
    let z = lobby.local[object][2];
    let target_z = if d.mouse_down { -0.13 } else { 0.0 };
    lobby.local[object][2] = smooth_damp(z, target_z, &mut d.tween_velocity, 0.025, f32::INFINITY, dt);
    d.was_mouse_down = d.mouse_down;
    lobby.drag = Some(d);
}

/// `ControlAssignment.Update` + `UpdateGUI` with the keyboard and mouse as the only input
/// (`slot` 0..3 = Player 1..4, 4 = unused).
fn update_control_assignment(lobby: &mut Lobby, s: &ScriptInstance, slot: &mut usize) {
    // One available input, offline: the "one controller" instructions.
    for (field, on) in [
        ("instructionsDragAndDrop", false),
        ("instructionsLeftAndRight", false),
        ("instructionsBoth", false),
        ("instructionsOneController", true),
        ("instructionsOneControllerNetworkGame", false),
    ] {
        if let Some(o) = lobby.scene.field_object(s, field) {
            lobby.active[o] = on;
        }
    }
    let headings: Vec<usize> = lobby.scene.field_objects(s, "playerHeadings").into_iter().flatten().collect();
    let deactivated = lobby.scene.field_object(s, "deactivatedHeading");
    // `Start`: regular = playerHeadings[0].color, drag target = playerHeadings[1].color,
    // deactivated = deactivatedHeading.color (static, taken once).
    let color_of = |lobby: &Lobby, o: Option<usize>| o.and_then(|o| lobby.text_of_object.get(&o)).map(|&t| lobby.texts[t].color);
    if lobby.heading_colors.is_none() {
        if let (Some(regular), Some(target), Some(off)) = (color_of(lobby, headings.first().copied()), color_of(lobby, headings.get(1).copied()), color_of(lobby, deactivated)) {
            lobby.heading_colors = Some((regular, target, off));
        }
    }
    let heading = |i: usize| if i < 4 { headings.get(i).copied() } else { deactivated };
    let Some(button) = lobby.scene.field_object(s, "prototypeInputButton") else { return };
    lobby.active[button] = true;
    // The card being dragged or dropped: the nearest heading (by world x) is the target.
    let mut target_heading = None;
    let dragging = lobby.drag.as_ref().filter(|d| d.object == button).map(|d| (d.mouse_down, d.dropped));
    let mover = lobby.movers.iter().position(|m| m.object == button);
    match dragging {
        Some((down, dropped)) if down || dropped => {
            if let Some(m) = mover {
                lobby.movers[m].enabled = false;
            }
            let x = lobby.world(button).t[0];
            let mut best = (usize::MAX, f32::INFINITY);
            for column in 0..5 {
                let Some(h) = heading(column) else { continue };
                let distance = (lobby.world(h).t[0] - x).abs();
                if distance < best.1 {
                    best = (column, distance);
                }
            }
            if best.0 != usize::MAX {
                target_heading = heading(best.0);
                if dropped && best.0 != *slot {
                    *slot = best.0;
                }
            }
        }
        _ => {
            // `SmoothDampTowards` back on, towards the column's place.
            if let (Some(m), Some(column)) = (mover, heading(*slot)) {
                lobby.movers[m].enabled = true;
                lobby.movers[m].target[0] = lobby.scene.objects[column].local_position[0];
            }
        }
    }
    if let Some(d) = lobby.drag.as_mut() {
        d.dropped = false;
    }
    // Heading colours: the drag target; Player columns with an input regular, the rest (and the
    // unused-controllers column) deactivated.
    if let Some((regular, drag_target, off)) = lobby.heading_colors {
        for column in 0..5 {
            let Some(h) = heading(column) else { continue };
            let color = if Some(h) == target_heading {
                drag_target
            } else if column < 4 && column == *slot {
                regular
            } else {
                off
            };
            lobby.set_text_color(h, color);
        }
    }
    if let Some(text) = lobby.scene.child(button, "Text") {
        lobby.set_text(text, "Keyboard and Mouse");
    }
}

/// `LevelSelect.Awake`: remember the cards' resting places, instantiate one sliding label per
/// level (name: campaign and versus; number; top score) at its offscreen spot, hide the
/// prototypes, and send the difficulty stars offscreen.
fn level_select_awake(lobby: &mut Lobby, s: &ScriptInstance) {
    let field = |lobby: &Lobby, name: &str| lobby.scene.field_object(s, name);
    let cards: Vec<usize> = lobby.scene.field_objects(s, "cards").into_iter().flatten().collect();
    let local_of = |lobby: &Lobby, o: Option<usize>| o.map_or([0.0; 3], |o| lobby.local[o]);
    let mut labels = Vec::new();
    let make = |lobby: &Lobby, labels: &mut Vec<SlidingLabel>, prototype: Option<usize>, offscreen: [f32; 3], texts: Vec<String>| -> Vec<usize> {
        let Some(object) = prototype else { return Vec::new() };
        let Some(&item) = lobby.text_of_object.get(&object) else { return Vec::new() };
        let smooth_time = lobby.scene.script(object, "SmoothDampTowards").and_then(|m| m.fields.f32_of("smoothTime")).unwrap_or(0.1);
        texts
            .into_iter()
            .map(|text| {
                labels.push(SlidingLabel { prototype: item, prototype_object: object, text, local: offscreen, velocity: [0.0; 3], target: offscreen, smooth_time });
                labels.len() - 1
            })
            .collect()
    };
    let name_prototype = field(lobby, "selectedLevelNameLabelPrototype");
    let number_prototype = field(lobby, "selectedLevelNumberLabelPrototype");
    let score_prototype = field(lobby, "selectedLevelTopScoreLabelPrototype");
    let name_offscreen = local_of(lobby, field(lobby, "levelNameLabelOffscreen"));
    let number_offscreen = local_of(lobby, field(lobby, "levelNumberLabelOffscreen"));
    let campaign: Vec<String> = lobby.levels.campaign.iter().map(|id| level_display_name(id).to_owned()).collect();
    let versus: Vec<String> = lobby.levels.versus.iter().map(|id| level_display_name(id).to_owned()).collect();
    let campaign_names = make(lobby, &mut labels, name_prototype, name_offscreen, campaign);
    let versus_names = make(lobby, &mut labels, name_prototype, name_offscreen, versus);
    let count = lobby.levels.campaign.len().max(lobby.levels.versus.len());
    let numbers = make(lobby, &mut labels, number_prototype, number_offscreen, (1..=count).map(|n| n.to_string()).collect());
    // Top scores start at the name label's offscreen spot; with no saved scores they are empty.
    let scores = make(lobby, &mut labels, score_prototype, name_offscreen, vec![String::new(); lobby.levels.campaign.len()]);
    for prototype in [name_prototype, number_prototype, score_prototype].into_iter().flatten() {
        lobby.active[prototype] = false;
    }
    lobby.labels = labels;
    // Stars: remembered, then put offscreen (position and target).
    let star_offscreen = local_of(lobby, field(lobby, "difficultyStarOffscreen"));
    let stars: Vec<usize> = lobby.scene.field_objects(s, "difficultyStars").into_iter().flatten().collect();
    for &star in &stars {
        lobby.local[star] = star_offscreen;
        if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == star) {
            m.target = star_offscreen;
        }
    }
    let trailer = field(lobby, "trailerButton");
    let upgrade = field(lobby, "upgradeButton");
    let state = LevelSelectState {
        object: Some(s.object),
        card_images: cards.iter().map(|&c| lobby.scene.child(c, "Image")).collect(),
        resting_positions: cards.iter().map(|&c| [lobby.local[c][0], lobby.local[c][1]]).collect(),
        resting_z: cards.iter().map(|&c| lobby.local[c][2]).collect(),
        resting_rotations: cards.iter().map(|&c| lobby.scene.objects[c].local_rotation).collect(),
        overlays_standard: cards.iter().map(|&c| lobby.scene.child(c, "Overlay Standard")).collect(),
        overlays_starter: cards.iter().map(|&c| lobby.scene.child(c, "Overlay Starter")).collect(),
        stack: (0..cards.len()).collect(),
        cards,
        current: 0,
        last: -1,
        animating: false,
        animating_out: false,
        animating_forward: false,
        animation_time: 0.0,
        animation_position: [0.0, 0.0],
        forward_destination: { let p = local_of(lobby, field(lobby, "forwardsAnimationDestination")); [p[0], p[1]] },
        backward_destination: { let p = local_of(lobby, field(lobby, "backwardsAnimationDestination")); [p[0], p[1]] },
        last_seen_index: -1,
        was_active: false,
        campaign_names,
        versus_names,
        numbers,
        scores,
        name_selected: local_of(lobby, name_prototype),
        name_offscreen,
        number_selected: local_of(lobby, number_prototype),
        number_offscreen,
        score_selected: local_of(lobby, score_prototype),
        stars,
        star_offscreen,
        high_scores: field(lobby, "highScoresButton"),
        trailer,
        upgrade,
        buttons_offscreen: local_of(lobby, field(lobby, "highScoresButtonOffscreen")),
        trailer_enabled: local_of(lobby, trailer),
        upgrade_enabled: local_of(lobby, upgrade),
        count_label: field(lobby, "levelCountLabel"),
    };
    lobby.level_select = Some(state);
}

impl LevelSelectState {
    /// `GetDestPosition`.
    fn destination(&self) -> [f32; 2] {
        if !self.animating_out {
            [0.0, 0.0]
        } else if self.animating_forward {
            self.forward_destination
        } else {
            self.backward_destination
        }
    }

    /// `FinishAnimatingOut`: the card that left goes to the other end of the pile.
    fn finish_animating_out(&mut self) {
        if self.animating_forward {
            let item = self.stack.pop().unwrap_or(0);
            self.stack.insert(0, item);
        } else if !self.stack.is_empty() {
            let item = self.stack.remove(0);
            self.stack.push(item);
        }
    }
}

/// `LevelSelect.Update` (no saved scores, full version: no stars, no high-score, trailer or
/// upgrade buttons; locked levels show the standard overlay).
fn level_select_update(lobby: &mut Lobby, st: &mut LevelSelectState, memory: &mut LobbyMemory, move_left: bool, move_right: bool, dt: f32) {
    let count = lobby.levels.levels(memory.mode).len() as i32;
    // `LobbyLevelSelect.Clamp`.
    let clamp = |v: i32| v.clamp(0, (count - 1).max(0));
    if move_left {
        lobby.display_level = clamp(lobby.display_level) - 1;
    }
    if move_right {
        lobby.display_level = clamp(lobby.display_level) + 1;
    }
    let index = memory.level_index as i32;
    if clamp(lobby.display_level) != index {
        if index != st.last_seen_index {
            lobby.display_level = index;
        } else if level_unlocked(clamp(lobby.display_level) as usize, memory.mode) {
            memory.level_index = clamp(lobby.display_level) as usize;
        }
    }
    st.last_seen_index = memory.level_index as i32;
    let b = count - 1;
    st.current = st.current.min(b);
    st.last = st.last.min(b);
    let (num, num2) = (st.current, clamp(lobby.display_level));
    if num != num2 {
        let forward = num2 > num;
        let mut reversed = false;
        if st.animating {
            if st.animating_forward ^ forward {
                if st.animating_out {
                    st.animating_out = false;
                    st.animating_forward = !st.animating_forward;
                    std::mem::swap(&mut st.current, &mut st.last);
                    st.animation_time = 0.25 - st.animation_time;
                    reversed = true;
                }
            } else {
                if st.animating_out {
                    st.finish_animating_out();
                    st.animating_out = false;
                }
                st.animation_position = st.destination();
            }
        }
        if !reversed {
            st.last = st.current;
            st.current = num2;
            if st.last != -1 {
                // `MasterAudio.PlaySound3DAtTransformAndForget("Paper Slide", transform)`.
                if let Some(object) = st.object {
                    let at = lobby.world_position(object);
                    lobby.sounds.push(("Paper Slide".to_owned(), at));
                }
                st.animating = true;
                st.animating_forward = forward;
                st.animating_out = true;
                st.animation_time = 0.25;
            }
        }
    }
    let n = st.stack.len();
    let mut moving: i64 = -1;
    if st.animating {
        let mut step = dt;
        st.animation_time -= step;
        if st.animation_time <= 0.0 {
            st.animation_position = st.destination();
            if st.animating_out {
                step = 1e-5f32.max(0.0 - st.animation_time);
                st.animation_time = 0.25;
                st.finish_animating_out();
                st.animating_out = false;
            } else {
                st.animating = false;
            }
        }
        if st.animating {
            let d = st.destination();
            let k = step / (st.animation_time + step);
            st.animation_position = [st.animation_position[0] + (d[0] - st.animation_position[0]) * k, st.animation_position[1] + (d[1] - st.animation_position[1]) * k];
            moving = if st.animating_forward == st.animating_out { n as i64 - 1 } else { 0 };
        }
    }
    let images = if memory.mode == GameMode::Campaign { lobby.levels.campaign_images.clone() } else { lobby.levels.versus_images.clone() };
    for i in 0..n {
        let place = st.stack[i];
        let mut v = st.resting_positions[place];
        if i as i64 == moving {
            v = [v[0] + st.animation_position[0], v[1] + st.animation_position[1]];
        }
        let card = st.cards[i];
        lobby.local[card] = [v[0], v[1], st.resting_z[i]];
        lobby.rotation_override.insert(card, st.resting_rotations[place]);
        if i == n - 1 || i as i64 == moving || i as i64 == moving - 1 {
            let mut level = st.current;
            if st.animating && ((st.animating_forward && i as i64 == moving) || (!st.animating_forward && i as i64 != moving)) {
                level = st.last;
            }
            if let (Some(image), Some(&texture)) = (st.card_images[i], images.get(level.max(0) as usize)) {
                lobby.card_textures.insert(image, texture);
            }
            let unlocked = level_unlocked(level.max(0) as usize, memory.mode);
            if let Some(o) = st.overlays_standard[i] {
                lobby.active[o] = !unlocked;
            }
            if let Some(o) = st.overlays_starter[i] {
                lobby.active[o] = false;
            }
        }
    }
    // Sliding labels: the selected level's name, number and top score in place, the rest away.
    for (mode, names) in [(GameMode::Campaign, &st.campaign_names), (GameMode::Versus, &st.versus_names)] {
        for (l, &label) in names.iter().enumerate() {
            let selected = mode == memory.mode && l as i32 == st.current;
            lobby.labels[label].target = if selected { st.name_selected } else { st.name_offscreen };
        }
    }
    for (m, &label) in st.scores.iter().enumerate() {
        let selected = memory.mode == GameMode::Campaign && m as i32 == st.current;
        lobby.labels[label].target = if selected { st.score_selected } else { st.name_offscreen };
    }
    for (k, &label) in st.numbers.iter().enumerate() {
        lobby.labels[label].target = if k as i32 == st.current { st.number_selected } else { st.number_offscreen };
    }
    // Trailer / upgrade buttons: Starter Edition only; stars and the high-score button: only with
    // a saved score for that difficulty (none are saved yet).
    // TODO: saved high scores (`HighScores.GetScoreTable`).
    let set_target = |lobby: &mut Lobby, object: Option<usize>, target: [f32; 3]| {
        if let Some(m) = object.and_then(|o| lobby.movers.iter_mut().find(|m| m.object == o)) {
            m.target = target;
        }
    };
    let _ = (st.trailer_enabled, st.upgrade_enabled);
    set_target(lobby, st.trailer, st.buttons_offscreen);
    set_target(lobby, st.upgrade, st.buttons_offscreen);
    for &star in &st.stars {
        set_target(lobby, Some(star), st.star_offscreen);
    }
    set_target(lobby, st.high_scores, st.buttons_offscreen);
    if let Some(label) = st.count_label {
        lobby.set_text(label, &format!("/ {count}"));
    }
}

/// `Button.Pressed` handlers of the scripts on (or pointing at) `object`.
#[allow(clippy::too_many_arguments)]
fn press(
    lobby: &mut Lobby,
    memory: &mut LobbyMemory,
    object: usize,
    session: &mut Session,
    source: &mut LevelSource,
    config: &GameConfig,
    next: &mut NextState<AppState>,
    exit: &mut MessageWriter<AppExit>,
) {
    let scripts: Vec<ScriptInstance> = lobby.scene.scripts_of(object).cloned().collect();
    for s in &scripts {
        match s.class.as_str() {
            "ActivateSomethingOnButtonPress" => {
                if let Some(target) = lobby.scene.field_object(s, "toActivate") {
                    lobby.activate(target);
                }
            }
            "CycleDifficulty" => memory.difficulty = if memory.difficulty != 1 { memory.difficulty + 1 } else { -1 },
            "ExitButton" => {
                exit.write(AppExit::Success);
            }
            // `PlaySoundOnButtonPress` (default group "Select").
            "PlaySoundOnButtonPress" => {
                let sound = s.fields.str_of("sound").unwrap_or("Select").to_owned();
                let at = lobby.world_position(object);
                lobby.sounds.push((sound, at));
            }
            _ => {}
        }
    }
    // Buttons other scripts subscribe to.
    let is = |lobby: &Lobby, class_name: &'static str, field: &str| lobby.field(class_name, field) == Some(object);
    // `LobbyEnder.Start`: "Back" (offline) returns to the connection screen.
    for s in lobby.scene.scripts_named("LobbyEnder").cloned().collect::<Vec<_>>() {
        if lobby.scene.field_object(&s, "backButton") == Some(object) {
            if let Some(screen) = lobby.scene.field_object(&s, "connectionScreen") {
                lobby.activate(screen);
            }
        }
    }
    if is(lobby, "NetworkSetup", "playOfflineButton") {
        if let Some(screen) = lobby.field("NetworkSetup", "nextScreen") {
            lobby.activate(screen);
        }
    }
    if is(lobby, "NetworkSetup", "startNetworkGameButton") {
        // TODO: networking. Without a LAN connection the original flashes the Wi-Fi label.
        warn!("network games are not implemented");
    }
    if let Some(gm) = lobby.named_script("GameModeSelect").cloned() {
        let players = memory.player_count();
        let choice = [("campaignButton", GameMode::Campaign, true, "campaignCaption"), ("versusButton", GameMode::Versus, players > 1, "versusCaption"), ("teamVersusButton", GameMode::TeamVersus, players > 2, "teamVersusCaption")]
            .into_iter()
            .find(|(field, ..)| lobby.scene.field_object(&gm, field) == Some(object));
        if let Some((_, mode, available, caption)) = choice {
            if available {
                memory.mode = mode;
                if let Some(screen) = lobby.scene.field_object(&gm, "nextScreen") {
                    lobby.activate(screen);
                }
            } else if let Some(&text) = lobby.scene.field_object(&gm, caption).and_then(|c| lobby.text_of_object.get(&c)) {
                let from = lobby.texts[text].color;
                lobby.flashes.push(Flash { text, from, to: FLASHING_LABEL, time: 0.0 });
            }
        }
    }
    if is(lobby, "LevelSelect", "leftButton") || is(lobby, "LevelSelect", "rightButton") {
        let count = lobby.levels.levels(memory.mode).len() as i32;
        let clamped = lobby.display_level.clamp(0, (count - 1).max(0));
        lobby.display_level = if is(lobby, "LevelSelect", "leftButton") { clamped - 1 } else { clamped + 1 };
    }
    if is(lobby, "GameSummary", "startGameButton") {
        if memory.player_count() < 1 {
            if let Some(err) = lobby.field("GameSummary", "errorScreen") {
                // TODO: `LobbyError.ShowErrorAndReturnTo` message text.
                lobby.activate(err);
            }
            return;
        }
        let levels = lobby.levels.levels(memory.mode);
        let Some(id) = levels.get(memory.level_index).cloned() else { return };
        lobby.set_text(lobby.scene.child(object, "Text").unwrap_or(object), "Loading\u{2026}");
        match find_level_scene(&config.assets_dir(), &id) {
            Some(path) => {
                info!("starting {id} ({:?}, difficulty {}, {} player(s))", memory.mode, memory.difficulty, memory.player_count());
                *session = Session {
                    mode: memory.mode,
                    difficulty: memory.difficulty,
                    player_count: memory.player_count(),
                    player_index: memory.keyboard_slot.min(3),
                    teams: memory.teams.clone(),
                };
                source.0 = path;
                next.set(AppState::Loading);
            }
            None => warn!("no scene file for level {id}"),
        }
    }
}

/// `Application.LoadLevel(name)`: the scene file of a level, relative to the export root.
fn find_level_scene(assets_dir: &std::path::Path, id: &str) -> Option<String> {
    fn search(dir: &std::path::Path, file: &str) -> Option<PathBuf> {
        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Some(found) = search(&path, file) {
                    return Some(found);
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(file) {
                return Some(path);
            }
        }
        None
    }
    let found = search(&assets_dir.join("Game"), &format!("{id}.unity"))?;
    let relative = found.strip_prefix(assets_dir.parent()?).ok()?;
    Some(relative.to_string_lossy().replace('\\', "/"))
}

/// `CameraPositioner.LateUpdate` with the Lobby's influencers: every active
/// `CameraPositionInfluencer` (the active screen's).
fn lobby_camera(lobby: Res<Lobby>, time: Res<Time>, windows: Query<&Window, With<PrimaryWindow>>, mut rigs: Query<(&mut Rig, &mut Transform), With<LobbyCamera>>) {
    let Ok((mut rig, mut transform)) = rigs.single_mut() else { return };
    for s in lobby.scene.scripts_named("CameraPositionInfluencer") {
        if !s.enabled || !lobby.active_in_hierarchy(s.object) {
            continue;
        }
        // The `Man` models' `Target` influencers are removed by `RemovePlayerOnlyParts`.
        if lobby.scene.objects[s.object].name == "Target" {
            continue;
        }
        let p = lobby.world_position(s.object);
        rig.add_influence(unity_compat::Vec2::new(p.x, p.y));
    }
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    rig.late_update(aspect, time.delta_secs().min(1.0 / 3.0));
    // Game z is negated for Bevy.
    let p = rig.position;
    transform.translation = Vec3::new(p.x, p.y, -p.z);
}

/// Push every active renderer of the Lobby into the frame's queue.
fn queue_lobby(
    mut lobby: ResMut<Lobby>,
    light: Res<SceneLight>,
    shadow_texture: Option<Res<ShadowTexture>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    rig_query: Query<&Rig, With<LobbyCamera>>,
    mut queue: ResMut<FrameQueue>,
) {
    let Ok(rig) = rig_query.single() else { return };
    let view = camera_view(rig, &windows);
    let lobby = &mut *lobby;
    let mut offsets: HashMap<usize, [f32; 3]> = HashMap::new();
    let mut offset_of = |lobby: &Lobby, object: usize| -> [f32; 3] {
        *offsets.entry(object).or_insert_with(|| {
            let w = lobby.world(object).t;
            let s = lobby.scene.objects[object].world.t;
            [w[0] - s[0], w[1] - s[1], w[2] - s[2]]
        })
    };
    let translate = |g: &Geometry, d: [f32; 3]| -> Geometry {
        let mut g = g.clone();
        if d != [0.0; 3] {
            for p in &mut g.positions {
                for k in 0..3 {
                    p[k] += d[k];
                }
            }
        }
        g
    };
    for i in 0..lobby.art.len() {
        let object = lobby.art[i].object;
        if !lobby.scene.active_in_hierarchy(object, &lobby.active) {
            continue;
        }
        let d = offset_of(lobby, object);
        if let Art::Text(index) = lobby.art[i].art {
            let world = lobby.world(object);
            if view.may_see(SimVec3::new(world.t[0], world.t[1], world.t[2]), 20.0) {
                if let Some((texture, g)) = text_geometry(lobby, index, light.for_shadows) {
                    queue.0.push_frame(texture, g);
                }
            }
            continue;
        }
        let r = &lobby.art[i];
        match &r.art {
            Art::Sprite { geometry, texture } => {
                let g = translate(geometry, d);
                if view.may_see(g.bounds_center(), extent(&g)) {
                    queue.0.push_frame(*texture, g);
                }
            }
            Art::SpriteShadow(shadow) => {
                if let Some(l) = light.for_shadows {
                    let g = translate(&shadow.geometry(l - SimVec3::new(d[0], d[1], d[2]), view.camera), d);
                    if view.may_see(g.bounds_center(), extent(&g)) {
                        queue.0.push_frame(shadow.texture, g);
                    }
                }
            }
            Art::Surface { surface, geometry, texture } => {
                let texture = lobby.card_textures.get(&r.object).unwrap_or(texture);
                let g = if lobby.chain_rotated(r.object) {
                    // Rotated at run time: the whole change of transform since the scene file.
                    let now = lobby.world(r.object);
                    match inverse(&lobby.scene.objects[r.object].world) {
                        Some(inv) => {
                            let m = now.mul(&inv);
                            let mut g = geometry.clone();
                            for p in &mut g.positions {
                                *p = m.transform_point(*p);
                            }
                            g
                        }
                        None => translate(geometry, d),
                    }
                } else {
                    translate(geometry, d)
                };
                if !view.may_see(g.bounds_center(), extent(&g)) {
                    continue;
                }
                queue.0.push_frame(*texture, g);
                if let (Some(l), Some(tex)) = (light.for_shadows, shadow_texture.as_deref()) {
                    if let Some(shadow) = surface_shadow_geometry(surface, l - SimVec3::new(d[0], d[1], d[2])) {
                        queue.0.push_frame(tex.0, translate(&shadow, d));
                    }
                }
            }
            Art::Text(_) => {}
        }
    }
    // `LevelSelect`'s sliding labels (children of their prototype's parent).
    for k in 0..lobby.labels.len() {
        let parent = lobby.scene.objects[lobby.labels[k].prototype_object].parent;
        if parent.is_some_and(|p| !lobby.active_in_hierarchy(p)) {
            continue;
        }
        for (texture, g) in label_geometry(lobby, k, light.for_shadows) {
            if view.may_see(g.bounds_center(), extent(&g)) {
                queue.0.push_frame(texture, g);
            }
        }
    }
}

/// A sliding label: the prototype's text settings at the label's own place, with the
/// prototype's `TextShadower` copy.
fn label_geometry(lobby: &mut Lobby, k: usize, light: Option<SimVec3>) -> Vec<(u32, Geometry)> {
    let mut out = Vec::new();
    let label = &lobby.labels[k];
    if label.text.is_empty() {
        return out;
    }
    let (item, object, text, local) = (label.prototype, label.prototype_object, label.text.clone(), label.local);
    let o = &lobby.scene.objects[object];
    let parent = o.parent.map_or(Affine::IDENTITY, |p| lobby.world(p));
    let r = o.local_rotation;
    let world = parent.mul(&Affine::from_trs(local, (r[0], r[1], r[2], r[3]), o.local_scale));
    let caster = &lobby.texts[item];
    let (font_guid, layout, color, color2, gradient) = (caster.font.clone(), caster.layout.clone(), caster.color, caster.color2, caster.use_gradient);
    let shadow = lobby.texts.iter().find_map(|t| t.shadow_of.as_ref().filter(|s| s.0 == item).map(|s| (s.1.clone(), s.2)));
    let Some(&texture) = lobby.font_textures.get(&font_guid) else { return out };
    let Some(font) = lobby.fonts.get(&font_guid) else { return out };
    let formatted = format_text(font, &layout, &text);
    let local_geometry = text_local_geometry(font, &layout, &formatted, color, color2, gradient);
    if let (Some(light), Some((shadow_font, plane))) = (light, shadow) {
        let (delta, k) = sticks_sim::draw::shadow_offset_and_scale(light, SimVec3::new(world.t[0], world.t[1], world.t[2]), plane);
        let mut shadow_layout = layout.clone();
        shadow_layout.formatting = false;
        shadow_layout.scale = [layout.scale[0] * k, layout.scale[1] * k, 1.0];
        let guid = shadow_font.unwrap_or(font_guid);
        if let (Some(&shadow_texture), Some(f)) = (lobby.font_textures.get(&guid), lobby.fonts.get(&guid)) {
            let g = text_local_geometry(f, &shadow_layout, &formatted, [0.0, 0.0, 0.0, color[3]], [0.0, 0.0, 0.0, color[3]], false);
            let mut at = world;
            at.t = [world.t[0] + delta[0], world.t[1] + delta[1], world.t[2] + 0.025];
            out.push((shadow_texture, place(&g, &at)));
        }
    }
    out.push((texture, place(&local_geometry, &world)));
    out
}

/// The mesh of text item `index` this frame (a shadow copy follows its text and the light).
fn text_geometry(lobby: &mut Lobby, index: usize, light: Option<SimVec3>) -> Option<(u32, Geometry)> {
    let item = &lobby.texts[index];
    match item.shadow_of.clone() {
        None => {
            let font_guid = item.font.clone();
            let texture = *lobby.font_textures.get(&font_guid)?;
            let font = lobby.fonts.get(&font_guid)?;
            let formatted = format_text(font, &item.layout, &item.text);
            let local = text_local_geometry(font, &item.layout, &formatted, item.color, item.color2, item.use_gradient);
            let world = lobby.world(item.object);
            Some((texture, place(&local, &world)))
        }
        Some((caster_scene_index, shadow_font, mut plane)) => {
            // The caster: the text item made from scene text `caster_scene_index` (same order).
            let caster = lobby.texts.get(caster_scene_index)?;
            let caster_object = caster.object;
            if !lobby.scene.active_in_hierarchy(caster_object, &lobby.active) {
                return None;
            }
            // `Button.Update`: a button caption's shadow falls on the button's background.
            if let Some(b) = lobby.buttons.iter().find(|b| b.text == Some(caster_object)) {
                plane = lobby.world(b.background).t[2];
            }
            let light = light?;
            let caster_world = lobby.world(caster_object);
            let caster_position = SimVec3::new(caster_world.t[0], caster_world.t[1], caster_world.t[2]);
            let (delta, k) = sticks_sim::draw::shadow_offset_and_scale(light, caster_position, plane);
            let caster = &lobby.texts[caster_scene_index];
            let caster_font = caster.font.clone();
            let mut layout = caster.layout.clone();
            let text = {
                let font = lobby.fonts.get(&caster_font)?;
                format_text(font, &layout, &caster.text)
            };
            let alpha = caster.color[3];
            layout.formatting = false;
            layout.scale = [layout.scale[0] * k, layout.scale[1] * k, 1.0];
            let font_guid = shadow_font.unwrap_or(caster_font);
            let texture = *lobby.font_textures.get(&font_guid)?;
            let font = lobby.fonts.get(&font_guid)?;
            let local = text_local_geometry(font, &layout, &text, [0.0, 0.0, 0.0, alpha], [0.0, 0.0, 0.0, alpha], false);
            let mut placed_world = caster_world;
            placed_world.t = [caster_world.t[0] + delta[0], caster_world.t[1] + delta[1], caster_world.t[2] + 0.025];
            Some((texture, place(&local, &placed_world)))
        }
    }
}
