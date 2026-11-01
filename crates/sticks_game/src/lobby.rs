//! The Lobby: the original's title and game set-up, which is one scene (`Lobby.unity`), not a
//! menu UI. Every screen (splash, title, control assignment, game summary, mode select, level
//! select, ...) sits at its own spot of one large paper world; `ActivateExclusively` switches
//! the screen objects and their `CameraPositionInfluencer` pulls the camera (a
//! `CameraPositioner`, as in the levels) across to the active one. Buttons are tk2d text and
//! `PolygonColliderRenderer` shapes with a 3D `BoxCollider` that the mouse clicks
//! (`OnMouseUpAsButton`); any lobby input's back (Escape, a gamepad's B or Back) presses the
//! visible back button. With a gamepad connected the buttons highlight (`Button.isDefault`,
//! `up`/`down`/`left`/`right`, `returnToLastSelected`) and a gamepad's A presses the
//! highlighted one.
//!
//! Everything drawn comes from the scene (sprites, polygon surfaces, tk2d text meshes and their
//! shadows, lit by the camera-borne point light) through the same queue as the levels. What the
//! scripts do is transcribed for the offline game with the keyboard and mouse and gamepads
//! (`crate::input`; every available one is a `LobbyInput`):
//!
//! * `LobbyStartupDirector`, `SplashScreenAdvance`, `NoControllers`, `ActivateExclusively`,
//!   `CameraPositionInfluencer`, `Button` (mouse events and the pressed z tween), `BackButton`,
//!   `ActivateSomethingOnButtonPress`, `ExitButton`;
//! * `NetworkSetup` (offline: "Play Offline"; with no server found on the LAN the "No Network
//!   Games Found" label shows);
//! * `ControlAssignment` (`sticks_sim::control_assignment`): a card per input under the column
//!   it controls; its left/right or a drag moves it; `TeamArrangement` likewise for the players'
//!   teams;
//! * `GameSummary`, `GameModeDisplay`, `LevelNameDisplay`, `DifficultyDisplay`,
//!   `CycleDifficulty`, `GameModeSelect`, `MakeGameModeConsistentWithPlayerCount`,
//!   `SmoothDampTowards`;
//! * `LevelSelect`: index, unlocking (`HighScores.IsLevelUnlocked` with no saved scores) and
//!   the level image.
//!
//! Audio: `PlaySoundOnButtonPress` and the photo pile's "Paper Slide" are queued in
//! [`Lobby::sounds`] for the audio module.
//!
//! Player slots: each control-assignment column with an available input is a player
//! (`Player.PlayerIndex`); starting a game hands the players and their inputs, the player count
//! and `LobbyTeams`' lists ([`LobbyMemory::teams`], kept like the original's statics) to the
//! [`Session`], from which the level spawns them (`PlayerSlotAssignments.OnLoadedLevelEverywhere`
//! spawn-point assignment, `PlayerSpawner.Spawn` team, default colours and processors,
//! `Player.SetUp` HUD slots).
//! "Readiness" is the original's: the Level Select's start button loads the level as soon as at
//! least one slot has controls (`ControlAssignment.PlayerCount() >= 1`); there is no per-player
//! ready flag in the game.
//!
//! Network games: [`network`] (hosting and joining on the LAN, the server list, slot claims,
//! shared settings and colours, disconnection errors, the working "Send Link" button) with
//! `crate::net`, and the room panel and dialogs above the screens ([`room`], not in the
//! original). Settings are the launcher's; the game has no settings screen.
//!
//! The other screens live in [`screens`]: colour customisation (`CustomizeColor`, with the
//! posed `Man` models of [`figures`] faded by `ColorModels`), the summary's models and team
//! labels, high scores (`HighScoresDisplay`; the level cards' top scores, difficulty stars and
//! high-score button), achievements (`GoalsDisplay`, `OverallGoalProgressDisplay`), the error
//! screen (`LobbyError`) and the credits' links, version label and hidden cheats. Scores,
//! colours, achievements and seen tips are saved like the original's `DataManager` files
//! (`crate::save`).
//! Not reproduced: touch input (and with it the swatch arrows).

mod figures;
pub mod network;
mod room;
mod screens;

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
use crate::input::{Gamepads, InputDevice};
use sticks_sim::control_assignment::{ControlAssignment, DEACTIVATED_COLUMN, InputFrame};
use sticks_sim::devices::LobbyMoves;
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
    /// `ControlAssignment.allPlayerInputs` / `deactivatedInputs`.
    pub assignment: ControlAssignment<InputDevice>,
    /// The lobby inputs available this frame (`LobbyInput.IsAvailable`).
    pub available: Vec<InputDevice>,
    /// `LobbyTeams`' static team lists.
    pub teams: sticks_sim::lobby::LobbyTeams,
    /// `LobbyLevelSelect.campaignLevels` as last read (the end-of-level button needs the count).
    pub campaign_levels: Vec<String>,
    /// `HighScores.scoreLists` (kept for the session; `DataManager` saving is not reproduced).
    pub high_scores: sticks_sim::level_end::HighScores,
    /// `LevelTips.tipsSeenByPlayerIDs`.
    pub tips_seen: sticks_sim::level_end::TipsSeen,
    /// `LocalPlayerColorAssignments.colors`.
    pub colors: sticks_sim::lobby::ColorAssignments,
    /// `Goals.progress`.
    pub goals: sticks_sim::goals::Goals,
    /// `AllLevelUnlockCheat.Enabled`.
    pub all_levels_unlocked: bool,
    /// `TrivialFPS.enabled` (toggled by `TrivialFPSEnabler`).
    pub fps_display: bool,
    /// Save files to write at the end of the frame (`DataManager.Save*`).
    pub save: crate::save::SaveFlags,
    /// The player's settings (not in the original; `crate::settings`).
    pub settings: crate::settings::Settings,
}

impl Default for LobbyMemory {
    fn default() -> Self {
        LobbyMemory {
            been_here_before: false,
            mode: GameMode::Campaign,
            difficulty: 0,
            level_index: 0,
            assignment: ControlAssignment::default(),
            available: vec![InputDevice::KeyboardMouse],
            teams: sticks_sim::lobby::LobbyTeams::default(),
            campaign_levels: Vec::new(),
            high_scores: sticks_sim::level_end::HighScores::default(),
            tips_seen: sticks_sim::level_end::TipsSeen::default(),
            colors: sticks_sim::lobby::ColorAssignments::default(),
            goals: sticks_sim::goals::Goals::default(),
            all_levels_unlocked: false,
            fps_display: false,
            save: crate::save::SaveFlags::default(),
            settings: crate::settings::Settings::default(),
        }
    }
}

impl LobbyMemory {
    /// The defaults with whatever the save files hold (`DataManager.LoadAll`).
    pub fn loaded() -> LobbyMemory {
        let mut memory = LobbyMemory::default();
        crate::save::load_all(&mut memory);
        memory.settings = crate::settings::Settings::load();
        memory
    }

    fn device_available(&self, device: InputDevice) -> bool {
        self.available.contains(&device)
    }

    /// `ControlAssignment.PlayerCount` (offline: the local players).
    pub fn player_count(&self) -> usize {
        self.assignment.player_count(|d| self.device_available(d))
    }

    /// `ControlAssignment.IsPlayerActiveLocally`: one of the column's inputs is available.
    pub fn is_player_active(&self, slot: usize) -> bool {
        self.assignment.is_player_active(slot, |d| self.device_available(d))
    }

    /// `ControlAssignment.LobbyInputsForPlayer` that are available.
    pub fn player_devices(&self, slot: usize) -> Vec<InputDevice> {
        self.assignment.column(slot).iter().copied().filter(|&d| self.device_available(d)).collect()
    }

    /// The players a game starts with (`ControlTypesForPlayer` of every slot).
    pub fn session_players(&self) -> Vec<crate::state::SessionPlayer> {
        (0..sticks_sim::lobby::MAX_PLAYERS)
            .map(|index| crate::state::SessionPlayer { index, devices: self.player_devices(index), remote: false })
            .filter(|p| !p.devices.is_empty())
            .collect()
    }

    /// `ControlAssignment.GetLocalPlayerID` offline: every slot's anonymous local id (slot 0
    /// would be the Steam user's, which falls back to the same without Steam).
    pub fn player_id(slot: usize) -> sticks_sim::level_end::PlayerId {
        sticks_sim::level_end::PlayerId::anonymous_local(slot)
    }

    /// `ControlAssignment.GetPlayerColors`.
    pub fn player_colors(&self, slot: usize) -> sticks_sim::lobby::CharacterColors {
        self.colors.get(&Self::player_id(slot))
    }

    /// `Goals.CampaignLevelCount`.
    pub fn campaign_level_count(&self) -> usize {
        self.campaign_levels.len()
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

#[derive(Clone)]
enum Art {
    Sprite { geometry: Geometry, texture: u32 },
    SpriteShadow(LevelShadow),
    Surface { surface: Surface, geometry: Geometry, texture: u32 },
    Text(usize),
}

#[derive(Clone)]
struct Renderable {
    object: usize,
    art: Art,
}

/// A `tk2dTextMesh` of the scene with what scripts change at run time.
#[derive(Clone)]
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

#[derive(Clone)]
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
    /// `Button.up` / `down` / `left` / `right` (button objects).
    neighbours: [Option<usize>; 4],
    /// `isDefault` (changed at run time with `returnToLastSelected`), `returnToLastSelected`.
    is_default: bool,
    return_to_last_selected: bool,
    was_highlighted: bool,
    /// Active in the hierarchy last frame (`OnDisable` hands the highlight on).
    was_active: bool,
}

/// `Button.highlightedTextColor` (255, 143, 56).
const HIGHLIGHTED_TEXT: [f32; 4] = [1.0, 143.0 / 255.0, 56.0 / 255.0, 1.0];

/// A `SmoothDampTowards` mover (local position, target, velocity).
#[derive(Clone)]
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
    /// `tk2dTextMesh.color` set at run time (the top scores' regular or golden colour).
    color: Option<[f32; 4]>,
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
    /// `completedDifficultyStarLocalPositions`.
    star_positions: Vec<[f32; 3]>,
    star_offscreen: [f32; 3],
    high_scores: Option<usize>,
    /// `highScoresButtonEnabledLocalPosition`.
    high_scores_enabled: [f32; 3],
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
    /// Objects the mouse went down on this frame (`OnMouseDown`).
    mouse_downs: Vec<usize>,
    /// Translation added to instantiated copies placed under another parent.
    rebase: HashMap<usize, [f32; 3]>,
    /// The posed models.
    figures: Option<figures::Figures>,
    /// State of the other screens' scripts.
    screens: screens::Screens,
    /// `Application.version` and `Application.unityVersion` (`VersionNumberLabel`).
    versions: Option<(String, String)>,
    /// `ControlAssignment.StartGUI`: one card per lobby input.
    input_cards: Option<CardColumns<InputDevice>>,
    /// The control-assignment screen was up last frame (`OnEnable` sets
    /// `firstUpdateSinceEnabled`).
    assignment_was_active: bool,
    /// `TeamArrangement.Start`: one card per player slot.
    player_cards: Option<CardColumns<usize>>,
    /// `Button.highlighted` (an index into `buttons`), for gamepads.
    highlighted: Option<usize>,
    /// The network screens' state.
    network: network::NetworkScreens,
}

/// Draggable cards made from a prototype (`ControlAssignment` / `TeamArrangement`), stacked
/// under column headings.
struct CardColumns<K> {
    cards: Vec<(K, usize)>,
    prototype: usize,
    /// `buttonStartY`, `buttonDistanceY` (the prototype's `BoxCollider` height × 1.2),
    /// `offBottomOfScreen.localPosition.y`.
    start_y: f32,
    distance_y: f32,
    off_bottom_y: f32,
}

impl<K: Copy + PartialEq> CardColumns<K> {
    /// `Start`: the prototype's place and size; the prototype itself is destroyed.
    fn start(lobby: &mut Lobby, prototype: usize, off_bottom: Option<usize>) -> CardColumns<K> {
        let world = lobby.world(prototype);
        let height = lobby.scene.boxes.iter().find(|b| b.object == prototype).map_or(0.0, |b| {
            // `BoxCollider.bounds.size.y`: the world box's vertical extent.
            let mut lo = f32::INFINITY;
            let mut hi = f32::NEG_INFINITY;
            for corner in 0..8 {
                let sign = |bit: usize| if corner & bit == 0 { -0.5 } else { 0.5 };
                let p = [b.center[0] + sign(1) * b.size[0], b.center[1] + sign(2) * b.size[1], b.center[2] + sign(4) * b.size[2]];
                let y = world.transform_point(p)[1];
                lo = lo.min(y);
                hi = hi.max(y);
            }
            hi - lo
        });
        let start_y = lobby.local[prototype][1];
        lobby.active[prototype] = false;
        CardColumns { cards: Vec::new(), prototype, start_y, distance_y: height * 1.2, off_bottom_y: off_bottom.map_or(-10.0, |o| lobby.local[o][1]), }
    }

    /// The card of `key` (instantiated, inactive, on first need).
    fn card(&mut self, lobby: &mut Lobby, key: K) -> usize {
        if let Some(&(_, card)) = self.cards.iter().find(|(k, _)| *k == key) {
            return card;
        }
        let local = lobby.local[self.prototype];
        let card = lobby.instantiate(self.prototype, None, local);
        lobby.active[card] = false;
        self.cards.push((key, card));
        card
    }

    /// `ButtonY(rowIndex)`.
    fn row_y(&self, row: usize) -> f32 {
        self.start_y - self.distance_y * row as f32
    }
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
            let rebase = self.rebase.get(&i).copied().unwrap_or([0.0; 3]);
            for k in 0..3 {
                world.t[k] += self.local[i][k] - o.local_position[k] + rebase[k];
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

    /// `Button.Up` / `Down` / `Left` / `Right` (`direction` 0..3): the neighbour, passing over
    /// switched-off ones in the same direction.
    fn neighbour(&self, button: usize, direction: usize) -> Option<usize> {
        let mut current = button;
        for _ in 0..self.buttons.len() {
            let next = self.buttons[current].neighbours[direction].and_then(|o| self.button_index(o))?;
            if self.active_in_hierarchy(self.buttons[next].object) {
                return Some(next);
            }
            current = next;
        }
        None
    }

    /// `Object.Instantiate(prototype)` placed under `parent` (`None`: the prototype's own parent)
    /// at `local`: a copy of the subtree with its renderers, texts, buttons, movers, scripts and
    /// mouse colliders. The copies keep the prototype's scene-file world transforms (a copy
    /// under another parent is rebased by the parents' difference), so everything drawn from the
    /// scene follows their run-time positions like any other object. Returns the copy's root.
    fn instantiate(&mut self, prototype: usize, parent: Option<usize>, local: [f32; 3]) -> usize {
        let old_parent = self.scene.objects[prototype].parent;
        let new_parent = parent.or(old_parent);
        let mut order = vec![prototype];
        let mut k = 0;
        while k < order.len() {
            order.extend(self.scene.objects[order[k]].children.iter().copied());
            k += 1;
        }
        let first = self.scene.objects.len();
        let map: HashMap<usize, usize> = order.iter().enumerate().map(|(i, &o)| (o, first + i)).collect();
        for &o in &order {
            let mut node = self.scene.objects[o].clone();
            if o != prototype {
                node.parent = node.parent.map(|p| map[&p]);
            }
            node.children = node.children.iter().map(|c| map[c]).collect();
            self.scene.objects.push(node);
            self.active.push(self.active[o]);
            self.local.push(self.local[o]);
        }
        let root = map[&prototype];
        self.scene.objects[root].parent = new_parent;
        if let Some(parent) = new_parent {
            self.scene.objects[parent].children.push(root);
        }
        if new_parent != old_parent {
            let at = |p: Option<usize>| p.map_or([0.0; 3], |p| self.scene.objects[p].world.t);
            let (a, b) = (at(new_parent), at(old_parent));
            self.rebase.insert(root, [a[0] - b[0], a[1] - b[1], a[2] - b[2]]);
        }
        self.local[root] = local;
        // Texts (a shadow copy keeps following its own caster's copy).
        let mut item_map = HashMap::new();
        for i in 0..self.texts.len() {
            if let Some(&o) = map.get(&self.texts[i].object) {
                let mut t = self.texts[i].clone();
                t.object = o;
                item_map.insert(i, self.texts.len());
                if t.shadow_of.is_none() {
                    self.text_of_object.insert(o, self.texts.len());
                }
                self.texts.push(t);
            }
        }
        for &new in item_map.values() {
            if let Some(shadow) = self.texts[new].shadow_of.as_mut() {
                if let Some(&caster) = item_map.get(&shadow.0) {
                    shadow.0 = caster;
                }
            }
        }
        for i in 0..self.art.len() {
            if let Some(&o) = map.get(&self.art[i].object) {
                let mut r = self.art[i].clone();
                r.object = o;
                if let Art::Text(t) = &mut r.art {
                    *t = item_map.get(t).copied().unwrap_or(*t);
                }
                self.art.push(r);
            }
        }
        for i in 0..self.buttons.len() {
            if let Some(&o) = map.get(&self.buttons[i].object) {
                let mut b = self.buttons[i].clone();
                b.object = o;
                b.background = map.get(&b.background).copied().unwrap_or(b.background);
                b.text = b.text.map(|t| map.get(&t).copied().unwrap_or(t));
                self.buttons.push(b);
            }
        }
        for i in 0..self.movers.len() {
            if let Some(&o) = map.get(&self.movers[i].object) {
                let mut m = self.movers[i].clone();
                m.object = o;
                m.target = self.local[o];
                self.movers.push(m);
            }
        }
        let scripts: Vec<ScriptInstance> = self.scene.scripts.iter().filter(|s| map.contains_key(&s.object)).cloned().collect();
        for mut s in scripts {
            s.object = map[&s.object];
            self.scene.scripts.push(s);
        }
        let boxes: Vec<_> = self.scene.boxes.iter().filter(|b| map.contains_key(&b.object)).cloned().collect();
        for mut b in boxes {
            b.object = map[&b.object];
            self.scene.boxes.push(b);
        }
        for (&o, &n) in &map {
            if let Some(&sorting) = self.scene.sorting.get(&o) {
                self.scene.sorting.insert(n, sorting);
            }
        }
        root
    }

    /// `tk2dSprite.color` of the sprites drawn for `object` (not their shadow copies).
    fn set_sprite_color(&mut self, object: usize, color: [f32; 4]) {
        let c = sticks_sim::visual::to_color32(color).map(|v| f32::from(v) / 255.0);
        for r in &mut self.art {
            if r.object == object {
                if let Art::Sprite { geometry, .. } = &mut r.art {
                    for v in &mut geometry.colors {
                        *v = c;
                    }
                }
            }
        }
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
        app.insert_resource(LobbyMemory::loaded())
            .init_resource::<LobbyClicks>()
            .init_resource::<room::RoomUi>()
            .add_systems(Startup, room::load_art)
            .add_systems(OnEnter(AppState::Lobby), enter_lobby)
            .add_systems(OnExit(AppState::Lobby), leave_lobby)
            .add_systems(
                Update,
                (room::room_actions, room::room_view, room::room_labels, lobby_mouse, lobby_update, lobby_camera)
                    .chain()
                    .run_if(in_state(AppState::Lobby))
                    .after(sticks_paper::PaperSet)
                    .before(QueueSet::Light),
            )
            .add_systems(OnExit(AppState::Lobby), room::hide)
            .add_systems(Update, queue_lobby.in_set(QueueSet::Collect).run_if(in_state(AppState::Lobby)))
            .add_systems(Update, trivial_fps);
    }
}

/// `TrivialFPS`' label (the Permanent System's, so it stays up in levels too).
#[derive(Component)]
struct FpsLabel;

/// `TrivialFPS.Update` / `OnGUI` while `TrivialFPSEnabler` has switched it on: frames counted
/// over each 0.5 s of unscaled time, shown at the top left (100 × 32).
/// TODO(UNCERTAIN): the label's `GUIStyle` (font, size, colour) is not recovered; Bevy's default
/// text style is used.
fn trivial_fps(
    mut commands: Commands,
    memory: Res<LobbyMemory>,
    time: Res<Time<Real>>,
    mut labels: Query<(Entity, &mut Text), With<FpsLabel>>,
    mut counter: Local<(u32, i64)>,
) {
    const INTERVAL: f32 = 0.5;
    let Ok((entity, mut text)) = labels.single_mut() else {
        if memory.fps_display {
            commands.spawn((
                FpsLabel,
                Text::new(""),
                Node { position_type: PositionType::Absolute, left: Val::Px(0.0), top: Val::Px(0.0), width: Val::Px(100.0), height: Val::Px(32.0), ..default() },
            ));
        }
        return;
    };
    if !memory.fps_display {
        commands.entity(entity).try_despawn();
        return;
    }
    counter.0 += 1;
    let interval = (time.elapsed_secs() / INTERVAL) as i64;
    if interval != counter.1 {
        let fps = counter.0 as f32 / INTERVAL;
        counter.0 = 0;
        counter.1 = interval;
        text.0 = format!("{}", fps.round() as i32);
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
        let neighbours = ["up", "down", "left", "right"].map(|f| scene.field_object(s, f));
        buttons.push(ButtonState {
            object,
            background,
            text,
            unhighlighted,
            pressed_z,
            tween_velocity: 0.0,
            mouse_down: false,
            mouse_entered: false,
            neighbours,
            is_default: s.fields.bool_of("isDefault").unwrap_or(false),
            return_to_last_selected: s.fields.bool_of("returnToLastSelected").unwrap_or(false),
            was_highlighted: false,
            was_active: false,
        });
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
    memory.campaign_levels = levels.campaign.clone();
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

    let figures = figures::Figures::load(&scene, root, "Assets/Game/Lobby/Lobby.unity", &mut textures, &mut load);
    if figures.is_none() {
        warn!("lobby: the posed models could not be loaded");
    }
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
        mouse_downs: Vec::new(),
        rebase: HashMap::new(),
        figures,
        screens: screens::Screens::default(),
        versions: application_versions(root),
        input_cards: None,
        assignment_was_active: false,
        player_cards: None,
        highlighted: None,
        network: network::NetworkScreens::default(),
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

/// `Application.version` (`PlayerSettings.bundleVersion`) and `Application.unityVersion` from
/// the export's project settings.
fn application_versions(export_root: &std::path::Path) -> Option<(String, String)> {
    let settings = std::fs::read_to_string(export_root.join("ProjectSettings/ProjectSettings.asset")).ok()?;
    let version = settings.lines().find_map(|l| l.trim().strip_prefix("bundleVersion:")).map(|v| v.trim().to_owned())?;
    let project = std::fs::read_to_string(export_root.join("ProjectSettings/ProjectVersion.txt")).ok()?;
    let unity = project.lines().find_map(|l| l.trim().strip_prefix("m_EditorVersion:")).map(|v| v.trim().to_owned())?;
    Some((version, unity))
}

fn leave_lobby(mut commands: Commands, cameras: Query<Entity, With<LobbyCamera>>) {
    for entity in &cameras {
        commands.entity(entity).try_despawn();
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
    paper: Res<sticks_paper::PaperInput>,
) {
    let lobby = &mut *lobby;
    // The room's panel and dialogs take the pointer from the screens below them.
    let blocked = paper.pointer_captured || lobby.network.dialog.is_some();
    let ray = scripted.0.or_else(|| windows.single().ok().and_then(|w| w.cursor_position()).filter(|_| !blocked)).and_then(|cursor| {
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
        lobby.mouse_downs.extend(now);
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
    mut pads: ResMut<Gamepads>,
    mut net: ResMut<crate::net::Net>,
    paper: Res<sticks_paper::PaperInput>,
) {
    let dt = time.delta_secs().min(1.0 / 3.0);
    let lobby = &mut *lobby;
    let net = &mut *net;
    // Typing in the room's chat (or a dialog) is not Lobby input.
    let keys_free = !paper.keyboard_captured && lobby.network.dialog.is_none();
    let mouse_free = !paper.pointer_captured && lobby.network.dialog.is_none();
    // Every available `LobbyInput` this frame: `KeyboardAndMouseLobbyInput` (A/D move,
    // Escape is back, any key or mouse button counts as a press; always idle) and
    // `XboxishGamepadLobbyInput` per gamepad.
    let devices = pads.devices();
    let moves: Vec<(InputDevice, LobbyMoves)> = devices
        .iter()
        .filter_map(|&d| match d {
            InputDevice::KeyboardMouse => Some((
                d,
                LobbyMoves {
                    left: keys_free && keys.just_pressed(KeyCode::KeyA),
                    right: keys_free && keys.just_pressed(KeyCode::KeyD),
                    back: keys_free && keys.just_pressed(KeyCode::Escape),
                    anything_pressed: (keys_free && keys.get_just_pressed().next().is_some()) || (mouse_free && mouse.get_just_pressed().next().is_some()),
                    ..LobbyMoves::default()
                },
            )),
            InputDevice::Gamepad(k) => pads.frame_mut(k).map(|f| (d, f.lobby.process(&f.state, &f.edges))),
        })
        .collect();
    memory.available = devices;
    let any = |test: fn(&LobbyMoves) -> bool| moves.iter().any(|(_, m)| test(m));
    let any_pressed = any(|m| m.anything_pressed);
    // `LobbyInput.All.Any(input => input.MoveLeft)` and friends.
    let move_left = any(|m| m.left);
    let move_right = any(|m| m.right);
    let back = any(|m| m.back);
    let gamepad_available = moves.iter().any(|(d, _)| matches!(d, InputDevice::Gamepad(_)));
    // Each player's own inputs' moves (`LobbyInputsForPlayer(i)`).
    let mut player_moves = [(0i32, 0i32); sticks_sim::lobby::MAX_PLAYERS];
    for (slot, pm) in player_moves.iter_mut().enumerate() {
        for d in memory.player_devices(slot) {
            if let Some((_, m)) = moves.iter().find(|(x, _)| *x == d) {
                pm.0 += i32::from(m.left);
                pm.1 += i32::from(m.right);
            }
        }
    }
    let mouse_player = (0..sticks_sim::lobby::MAX_PLAYERS).find(|&i| memory.player_devices(i).contains(&InputDevice::KeyboardMouse));
    let view = rigs.single().ok().map(|rig| camera_view(rig, &windows));

    // The models' physics steps (`FixedUpdate` comes before `Update`).
    let mut model_delta = dt;
    if let Some(mut figures) = lobby.figures.take() {
        model_delta = figures.fixed_steps(time.delta_secs(), |root| lobby.scene.active_in_hierarchy(root, &lobby.active));
        lobby.figures = Some(figures);
    }

    // `OnMouseDown` of the credits' hidden counters.
    for object in std::mem::take(&mut lobby.mouse_downs) {
        screens::mouse_down(lobby, &mut memory, object);
    }
    // `ShowEndingCheat` → `LevelLoader.GameComplete()` (offline: the lobby's players and mode).
    if std::mem::take(&mut lobby.screens.show_ending) && net.is_offline() {
        let players = network::total_players(&memory, net).max(1);
        *session = Session { mode: memory.mode, difficulty: memory.difficulty, player_count: players, players: memory.session_players(), teams: memory.teams.clone() };
        source.0 = crate::net::COMPLETE_SCENE.to_owned();
        next.set(AppState::Loading);
        return;
    }

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
        press(lobby, &mut memory, net, object, &mut session, &mut source, &config, &mut next, &mut exit);
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

    // The network screens (`NetworkSetup`, `ServerList`, `LobbyEnder`, ...) and the shared
    // settings; a client's start request reaches the host here.
    network::update(lobby, &mut memory, net, dt);
    if net.is_host() {
        for kind in std::mem::take(&mut net.load_requests) {
            if kind == sticks_net::LoadKind::Normal {
                start_game(lobby, &mut memory, net, None, &mut session, &mut source, &config, &mut next);
            }
        }
    }

    // `ControlAssignment.Start` / `Update` / `UpdateGUI`.
    if let Some(s) = lobby.named_script("ControlAssignment").cloned() {
        let active = lobby.active_in_hierarchy(s.object);
        if active {
            let first_update = !lobby.assignment_was_active;
            drag_update(lobby, dt);
            update_control_assignment(lobby, &s, &mut memory, &moves, first_update);
        }
        lobby.assignment_was_active = active;
    }
    network::resolve_assignments(&mut memory, net);
    // `TeamArrangement.Start` / `Update`.
    if let Some(s) = lobby.named_script("TeamArrangement").cloned() {
        if lobby.active_in_hierarchy(s.object) {
            drag_update(lobby, dt);
            update_team_arrangement(lobby, &s, &mut memory, &player_moves);
        }
    }

    // `MakeGameModeConsistentWithPlayerCount.Update`.
    let players = network::total_players(&memory, net);
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

    // The other screens, then the presses they simulated (`Button.SimulatePress`).
    screens::update(lobby, &mut memory, &player_moves, mouse_player, model_delta);
    for object in std::mem::take(&mut lobby.screens.simulated) {
        press(lobby, &mut memory, net, object, &mut session, &mut source, &config, &mut next, &mut exit);
    }
    // The models' `Update` and animator; their transforms follow their bodies.
    if let Some(mut figures) = lobby.figures.take() {
        figures.frame_update(model_delta, &mut lobby.local);
        lobby.figures = Some(figures);
    }

    // `Button.Update`: the gamepad highlight (`Select` presses the highlighted button), the text
    // colour and the pressed z tween.
    let select = any(|m| m.select);
    let direction = if any(|m| m.up) {
        Some(0)
    } else if any(|m| m.down) {
        Some(1)
    } else if any(|m| m.left) {
        Some(2)
    } else if any(|m| m.right) {
        Some(3)
    } else {
        None
    };
    let mut selected = Vec::new();
    for i in 0..lobby.buttons.len() {
        let object = lobby.buttons[i].object;
        let active = lobby.active_in_hierarchy(object);
        if !active && lobby.buttons[i].was_active && lobby.highlighted == Some(i) {
            // `OnDisable`: the highlight goes up, else down, left, right.
            lobby.highlighted = (0..4).find_map(|d| lobby.neighbour(i, d));
        }
        lobby.buttons[i].was_active = active;
        if !active {
            continue;
        }
        let visible = view.as_ref().is_some_and(|v| lobby.button_visible(i, v));
        if lobby.buttons[i].was_highlighted {
            if !visible || !gamepad_available {
                lobby.highlighted = None;
                if lobby.buttons[i].return_to_last_selected {
                    lobby.buttons[i].is_default = true;
                }
            } else {
                if let Some(d) = direction {
                    lobby.highlighted = lobby.neighbour(i, d);
                }
                if lobby.highlighted.is_none() {
                    lobby.highlighted = Some(i);
                }
            }
        } else if lobby.buttons[i].is_default && lobby.highlighted.is_none() && visible && gamepad_available {
            lobby.highlighted = Some(i);
            if lobby.buttons[i].return_to_last_selected {
                lobby.buttons[i].is_default = false;
            }
        }
        let highlighted = lobby.highlighted == Some(i);
        if highlighted && select {
            selected.push(object);
        }
        lobby.buttons[i].was_highlighted = highlighted;
    }
    for object in selected {
        press(lobby, &mut memory, net, object, &mut session, &mut source, &config, &mut next, &mut exit);
    }
    for i in 0..lobby.buttons.len() {
        let (object, down, target_down, text, colour) = {
            let b = &lobby.buttons[i];
            let colour = if lobby.highlighted == Some(i) { HIGHLIGHTED_TEXT } else { b.unhighlighted };
            (b.object, b.mouse_down && b.mouse_entered, b.pressed_z, b.text, colour)
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

/// `HighScores.IsLevelUnlocked`: the first level and every versus level; a campaign level once
/// the one before it has a score (this session's scores: saving is not reproduced).
/// TODO: `RemoteLevelAvailability` (network) and the starter edition.
fn level_unlocked(index: usize, memory: &LobbyMemory, campaign_levels: &[String]) -> bool {
    // `AllLevelUnlockCheat.Enabled` opens every level but the first check (index 0 is open).
    if index != 0 && memory.all_levels_unlocked {
        return true;
    }
    memory.high_scores.is_level_unlocked(index, memory.mode == GameMode::Campaign, campaign_levels)
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

/// `ControlAssignment.GetApplicableInstructions` for the available inputs: left/right for
/// gamepads and the keyboard, drag and drop for the mouse (and touch); both with the keyboard
/// and mouse.
fn applicable_instructions(available: &[InputDevice]) -> &'static str {
    let left_and_right = !available.is_empty();
    let drag_and_drop = available.contains(&InputDevice::KeyboardMouse);
    match (left_and_right, drag_and_drop) {
        (true, false) => "instructionsLeftAndRight",
        (false, true) => "instructionsDragAndDrop",
        _ => "instructionsBoth",
    }
}

/// The column (0..=3 players, then the unused one) whose heading is nearest a card's world x.
fn nearest_heading(lobby: &Lobby, card: usize, headings: &[Option<usize>]) -> Option<usize> {
    let x = lobby.world(card).t[0];
    let mut best: Option<(usize, f32)> = None;
    for (column, h) in headings.iter().enumerate() {
        let Some(h) = *h else { continue };
        let distance = (lobby.world(h).t[0] - x).abs();
        if best.is_none_or(|(_, d)| distance < d) {
            best = Some((column, distance));
        }
    }
    best.map(|(c, _)| c)
}

/// A card at rest: `SmoothDampTowards` on, towards its place; a card switched on now starts
/// below the screen.
fn rest_card(lobby: &mut Lobby, card: usize, target_x: f32, target_y: f32, off_bottom_y: f32) {
    let z = lobby.local[card][2];
    if !lobby.active[card] {
        lobby.local[card] = [target_x, off_bottom_y, z];
    }
    if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == card) {
        m.enabled = true;
        m.target = [target_x, target_y, z];
    }
}

/// The card the mouse is dragging, or let go of this frame (`IsBeingDragged`,
/// `WasDroppedThisFrame`).
fn card_drag(lobby: &Lobby, card: usize) -> Option<bool> {
    lobby.drag.as_ref().filter(|d| d.object == card && (d.mouse_down || d.dropped)).map(|d| d.dropped)
}

/// `ControlAssignment.Start` (once), `Update` and `UpdateGUI`: one card per lobby input under
/// the heading of the column it is in; left/right or a drop moves it.
fn update_control_assignment(lobby: &mut Lobby, s: &ScriptInstance, memory: &mut LobbyMemory, moves: &[(InputDevice, LobbyMoves)], first_update: bool) {
    let frames: Vec<InputFrame<InputDevice>> = moves
        .iter()
        .map(|&(input, m)| InputFrame { input, available: true, gamepad: matches!(input, InputDevice::Gamepad(_)), move_left: m.left, move_right: m.right })
        .collect();
    // `Start` waits for `LobbyInputManager.Ready`, then (the first time only) assigns the
    // keyboard and a lone gamepad to Player 1; `StartGUI` makes the cards.
    memory.assignment.start(&frames);
    if lobby.input_cards.is_none() {
        let Some(prototype) = lobby.scene.field_object(s, "prototypeInputButton") else { return };
        let off_bottom = lobby.scene.field_object(s, "offBottomOfScreen");
        lobby.input_cards = Some(CardColumns::start(lobby, prototype, off_bottom));
        // One "assigned elsewhere" label per player column, under its heading (for network
        // games), then `Destroy(prototypeAssignedElsewhereLabel)`.
        if let Some(label) = lobby.scene.field_object(s, "prototypeAssignedElsewhereLabel") {
            let prototype_local = lobby.local[label];
            let headings: Vec<Option<usize>> = lobby.scene.field_objects(s, "playerHeadings");
            let mut labels = Vec::new();
            for heading in headings.into_iter().take(4) {
                let x = heading.map_or(prototype_local[0], |h| lobby.local[h][0]);
                let copy = lobby.instantiate(label, None, [x, prototype_local[1], prototype_local[2]]);
                lobby.active[copy] = false;
                labels.push(copy);
            }
            lobby.network.elsewhere_labels = labels;
            lobby.active[label] = false;
        }
    }
    let elsewhere = lobby.network.elsewhere;
    memory.assignment.update_beside(&frames, first_update, elsewhere);

    // `UpdateGUI`: one available input offline shows the one-controller instructions; in a
    // network game with nobody else yet, the network game's.
    let one = memory.available.len() == 1;
    let instructions = if one && !lobby.network.online {
        "instructionsOneController"
    } else if one && !elsewhere.iter().any(|&e| e) {
        "instructionsOneControllerNetworkGame"
    } else {
        applicable_instructions(&memory.available)
    };
    for field in ["instructionsDragAndDrop", "instructionsLeftAndRight", "instructionsBoth", "instructionsOneController", "instructionsOneControllerNetworkGame"] {
        if let Some(o) = lobby.scene.field_object(s, field) {
            lobby.active[o] = field == instructions;
        }
    }
    let mut headings: Vec<Option<usize>> = lobby.scene.field_objects(s, "playerHeadings");
    headings.truncate(4);
    while headings.len() < 4 {
        headings.push(None);
    }
    headings.push(lobby.scene.field_object(s, "deactivatedHeading"));
    // Cards only drop on columns this machine plays.
    let drop_headings: Vec<Option<usize>> = headings.iter().enumerate().map(|(c, h)| if c < 4 && elsewhere[c] { None } else { *h }).collect();
    // `Start`: regular = playerHeadings[0].color, drag target = playerHeadings[1].color,
    // deactivated = deactivatedHeading.color (static, taken once).
    let color_of = |lobby: &Lobby, o: Option<usize>| o.and_then(|o| lobby.text_of_object.get(&o)).map(|&t| lobby.texts[t].color);
    if lobby.heading_colors.is_none() {
        if let (Some(regular), Some(target), Some(off)) = (color_of(lobby, headings[0]), color_of(lobby, headings[1]), color_of(lobby, headings[4])) {
            lobby.heading_colors = Some((regular, target, off));
        }
    }
    let Some(mut cards) = lobby.input_cards.take() else { return };
    let mut should_be_active = Vec::new();
    let mut target_heading = None;
    let mut dropped_handled = Vec::new();
    for column in 0..=DEACTIVATED_COLUMN {
        // `if (SlotAssignedToAnotherClient(i)) continue`.
        if column < 4 && elsewhere[column] {
            continue;
        }
        let mut row = 0;
        while row < memory.assignment.column(column).len() {
            let input = memory.assignment.column(column)[row];
            let card = cards.card(lobby, input);
            should_be_active.push(card);
            match card_drag(lobby, card) {
                Some(dropped) => {
                    if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == card) {
                        m.enabled = false;
                    }
                    let nearest = nearest_heading(lobby, card, &drop_headings);
                    target_heading = nearest.and_then(|c| headings[c]);
                    if dropped && !dropped_handled.contains(&card) {
                        dropped_handled.push(card);
                        if let Some(target) = nearest.filter(|&t| t != column) {
                            memory.assignment.drop_into(input, target);
                            // The rows below move up into its place.
                            if let Some(text) = lobby.scene.child(card, "Text") {
                                lobby.set_text(text, &input.label());
                            }
                            continue;
                        }
                    }
                }
                None => {
                    let x = headings[column].map_or(lobby.local[card][0], |h| lobby.local[h][0]);
                    let y = cards.row_y(row);
                    rest_card(lobby, card, x, y, cards.off_bottom_y);
                }
            }
            if let Some(text) = lobby.scene.child(card, "Text") {
                lobby.set_text(text, &input.label());
            }
            row += 1;
        }
    }
    if let Some(d) = lobby.drag.as_mut() {
        d.dropped = false;
    }
    // Heading colours: the drag target; player columns with inputs regular, the rest (and the
    // unused column) deactivated.
    if let Some((regular, drag_target, off)) = lobby.heading_colors {
        for (column, h) in headings.iter().enumerate() {
            let Some(h) = *h else { continue };
            let color = if Some(h) == target_heading {
                drag_target
            } else if column < DEACTIVATED_COLUMN && (elsewhere[column] || !memory.assignment.column(column).is_empty()) {
                regular
            } else {
                off
            };
            lobby.set_text_color(h, color);
        }
    }
    for &(_, card) in &cards.cards {
        lobby.active[card] = should_be_active.contains(&card);
    }
    for (column, &label) in lobby.network.elsewhere_labels.clone().iter().enumerate() {
        lobby.active[label] = elsewhere[column];
    }
    lobby.input_cards = Some(cards);
}

/// `TeamArrangement.Start` and `Update`: a card per player with controls under its team's
/// heading; dragging it to the other heading, or the player's own left/right, moves it.
fn update_team_arrangement(lobby: &mut Lobby, s: &ScriptInstance, memory: &mut LobbyMemory, player_moves: &[(i32, i32); sticks_sim::lobby::MAX_PLAYERS]) {
    use sticks_sim::lobby::{MAX_PLAYERS, MAX_TEAMS};
    if lobby.player_cards.is_none() {
        let Some(prototype) = lobby.scene.field_object(s, "prototypePlayerButton") else { return };
        let off_bottom = lobby.scene.field_object(s, "offBottomOfScreen");
        let mut cards = CardColumns::start(lobby, prototype, off_bottom);
        for player in 0..MAX_PLAYERS {
            cards.card(lobby, player);
        }
        lobby.player_cards = Some(cards);
    }
    let Some(cards) = lobby.player_cards.take() else { return };
    let headings: Vec<Option<usize>> = (0..MAX_TEAMS).map(|t| lobby.scene.field_objects(s, "teamHeadings").get(t).copied().flatten()).collect();
    let mut target_heading = None;
    for &(player, card) in &cards.cards {
        if let Some(dropped) = card_drag(lobby, card) {
            let nearest = nearest_heading(lobby, card, &headings);
            target_heading = nearest.and_then(|t| headings[t]);
            if let (true, Some(team)) = (dropped, nearest) {
                memory.teams.move_to_team(player, team);
            }
        }
        // `LobbyInputsForPlayer(i)`: each of the player's inputs moves it one team.
        let Some(current) = memory.teams.team_of(player) else { continue };
        let (left, right) = player_moves[player];
        let wanted = (current as i32 - left + right).clamp(0, MAX_TEAMS as i32 - 1) as usize;
        if wanted != current {
            memory.teams.move_to_team(player, wanted);
        }
    }
    if let Some(d) = lobby.drag.as_mut() {
        d.dropped = false;
    }
    for (team, heading) in headings.iter().enumerate() {
        let x = heading.map_or(0.0, |h| lobby.local[h][0]);
        let mut y = cards.start_y;
        for player in memory.teams.team(team).to_vec() {
            let Some(&(_, card)) = cards.cards.iter().find(|(p, _)| *p == player) else { continue };
            if !memory.is_player_active(player) && !lobby.network.elsewhere[player] {
                lobby.active[card] = false;
                continue;
            }
            let dragged = lobby.drag.as_ref().is_some_and(|d| d.object == card && d.mouse_down);
            if !lobby.active[card] {
                let z = lobby.local[card][2];
                lobby.local[card] = [x, cards.off_bottom_y, z];
                lobby.active[card] = true;
            }
            if let Some(m) = lobby.movers.iter_mut().find(|m| m.object == card) {
                m.enabled = !dragged;
                if !dragged {
                    m.target = [x, y, m.target[2]];
                }
            }
            if let Some(text) = lobby.scene.child(card, "Text") {
                lobby.set_text(text, &LobbyMemory::player_id(player).name);
            }
            y -= cards.distance_y;
        }
    }
    // Headings: the drag target, else `ControlAssignment.regularPlayerHeadingColor`.
    if let Some((regular, drag_target, _)) = lobby.heading_colors {
        for h in headings.iter().flatten() {
            lobby.set_text_color(*h, if Some(*h) == target_heading { drag_target } else { regular });
        }
    }
    let instructions = applicable_instructions(&memory.available);
    for field in ["instructionsDragAndDrop", "instructionsLeftAndRight", "instructionsBoth"] {
        if let Some(o) = lobby.scene.field_object(s, field) {
            lobby.active[o] = field == instructions;
        }
    }
    lobby.player_cards = Some(cards);
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
                labels.push(SlidingLabel { prototype: item, prototype_object: object, text, color: None, local: offscreen, velocity: [0.0; 3], target: offscreen, smooth_time });
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
    let star_positions: Vec<[f32; 3]> = stars.iter().map(|&star| lobby.local[star]).collect();
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
        star_positions,
        star_offscreen,
        high_scores: field(lobby, "highScoresButton"),
        high_scores_enabled: local_of(lobby, field(lobby, "highScoresButton")),
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
        } else if level_unlocked(clamp(lobby.display_level) as usize, memory, &lobby.levels.campaign) {
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
            let unlocked = level_unlocked(level.max(0) as usize, memory, &lobby.levels.campaign);
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
    // The campaign levels' top scores for this player count and difficulty.
    let players = memory.player_count();
    for (j, &label) in st.scores.iter().enumerate() {
        let level = lobby.levels.campaign.get(j).cloned().unwrap_or_default();
        let best = memory.high_scores.table(&level, players, memory.difficulty).best().cloned();
        let l = &mut lobby.labels[label];
        match best {
            Some(score) => {
                l.text = score.value.to_string();
                l.color = Some(if score.golden { screens::GOLDEN_SCORE } else { screens::REGULAR_SCORE });
            }
            None => l.text.clear(),
        }
    }
    let set_target = |lobby: &mut Lobby, object: Option<usize>, target: [f32; 3]| {
        if let Some(m) = object.and_then(|o| lobby.movers.iter_mut().find(|m| m.object == o)) {
            m.target = target;
        }
    };
    // Trailer / upgrade buttons: Starter Edition only (this is the full version).
    let _ = (st.trailer_enabled, st.upgrade_enabled);
    set_target(lobby, st.trailer, st.buttons_offscreen);
    set_target(lobby, st.upgrade, st.buttons_offscreen);
    // A star for each difficulty the selected campaign level has a score on; the high-score
    // button when the selected difficulty has one.
    let level = lobby.levels.levels(memory.mode).get(st.current.max(0) as usize).cloned().unwrap_or_default();
    for (k, &star) in st.stars.iter().enumerate() {
        let difficulty = k as i32 - 1;
        let scored = memory.mode == GameMode::Campaign && !memory.high_scores.table(&level, players, difficulty).scores().is_empty();
        let target = if scored { st.star_positions[k] } else { st.star_offscreen };
        set_target(lobby, Some(star), target);
        if difficulty == memory.difficulty {
            set_target(lobby, st.high_scores, if scored { st.high_scores_enabled } else { st.buttons_offscreen });
        }
    }
    if let Some(label) = st.count_label {
        lobby.set_text(label, &format!("/ {count}"));
    }
}

/// `Button.Pressed` handlers of the scripts on (or pointing at) `object`.
#[allow(clippy::too_many_arguments)]
fn press(
    lobby: &mut Lobby,
    memory: &mut LobbyMemory,
    net: &mut crate::net::Net,
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
    screens::press(lobby, object);
    if network::press(lobby, net, object) {
        return;
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
    if let Some(gm) = lobby.named_script("GameModeSelect").cloned() {
        let players = network::total_players(memory, net);
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
        start_game(lobby, memory, net, Some(object), session, source, config, next);
    }
}

/// `GameSummary`'s start button → `LevelLoader.StartGame()`: offline the selected level loads
/// at once; in a network game the host loads it on every machine (a client asks the host).
#[allow(clippy::too_many_arguments)]
fn start_game(
    lobby: &mut Lobby,
    memory: &mut LobbyMemory,
    net: &mut crate::net::Net,
    button: Option<usize>,
    session: &mut Session,
    source: &mut LevelSource,
    config: &GameConfig,
    next: &mut NextState<AppState>,
) {
    let players = network::total_players(memory, net);
    if players < 1 {
        if let (Some(err), Some(summary)) = (lobby.field("GameSummary", "errorScreen"), lobby.named_script("GameSummary").map(|s| s.object)) {
            lobby.show_error(err, "\"You can't start a game unless there's at least one player.\" - Shakespeare probably", summary);
        }
        return;
    }
    if net.is_client() {
        net.request_load(sticks_net::LoadKind::Normal);
        return;
    }
    let levels = lobby.levels.levels(memory.mode);
    let Some(id) = levels.get(memory.level_index).cloned() else { return };
    if let Some(button) = button {
        lobby.set_text(lobby.scene.child(button, "Text").unwrap_or(button), "Loading\u{2026}");
    }
    let Some(path) = find_level_scene(&config.assets_dir(), &id) else {
        warn!("no scene file for level {id}");
        return;
    };
    info!("starting {id} ({:?}, difficulty {}, {} player(s))", memory.mode, memory.difficulty, players);
    if net.is_host() {
        net.host_load(path, memory.mode, memory.difficulty, players);
        return;
    }
    *session = Session {
        mode: memory.mode,
        difficulty: memory.difficulty,
        player_count: players,
        players: memory.session_players(),
        teams: memory.teams.clone(),
    };
    source.0 = path;
    next.set(AppState::Loading);
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
fn lobby_camera(
    lobby: Res<Lobby>,
    time: Res<Time>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut rigs: Query<(&mut Rig, &mut Transform, &Projection), With<LobbyCamera>>,
    mut logged: Local<Option<(Vec<usize>, u32)>>,
) {
    let Ok((mut rig, mut transform, projection)) = rigs.single_mut() else { return };
    let mut influencers = Vec::new();
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
        influencers.push(s.object);
    }
    let aspect = windows.single().map_or(16.0 / 9.0, |w| w.width() / w.height());
    rig.late_update(aspect, time.delta_secs().min(1.0 / 3.0));
    // Diagnostics for the framing (logged about a second after the influencers change).
    let frames = match logged.as_mut() {
        Some((seen, frames)) if *seen == influencers => {
            *frames += 1;
            *frames
        }
        _ => {
            *logged = Some((influencers.clone(), 0));
            0
        }
    };
    if frames == 60 {
        let names: Vec<&str> = influencers.iter().map(|&o| lobby.scene.objects[o].name.as_str()).collect();
        let window = windows.single().map(|w| (w.width(), w.height(), w.physical_width(), w.physical_height())).ok();
        let fov = match projection {
            Projection::Perspective(p) => p.fov.to_degrees(),
            _ => 0.0,
        };
        info!("lobby camera: influencers {names:?}, aspect {aspect:.3}, window {window:?}, fov {fov:.1}, position {:?}", rig.position);
    }
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
    // The posed models and the achievements' progress marks (`LinesRenderer` with the "Line"
    // sprite of the models' atlas).
    if let Some(figures) = &lobby.figures {
        let geometries = figures.renderers(|root| lobby.scene.active_in_hierarchy(root, &lobby.active), light.for_shadows);
        for g in geometries {
            if view.may_see(g.bounds_center(), extent(&g)) {
                queue.0.push_frame(figures.art.texture, g);
            }
        }
        for (renderer, lines) in &lobby.screens.goal_marks {
            let visual = sticks_sim::visual::lines_with_shadows(lines, *renderer, 0.0, light.for_shadows);
            for g in visual.renderers(&figures.art.art).into_iter().take(1) {
                if !g.is_empty() {
                    queue.0.push_frame(figures.art.texture, g);
                }
            }
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
    let (item, object, text, local, color_override) = (label.prototype, label.prototype_object, label.text.clone(), label.local, label.color);
    let o = &lobby.scene.objects[object];
    let parent = o.parent.map_or(Affine::IDENTITY, |p| lobby.world(p));
    let r = o.local_rotation;
    let world = parent.mul(&Affine::from_trs(local, (r[0], r[1], r[2], r[3]), o.local_scale));
    let caster = &lobby.texts[item];
    let (font_guid, layout, color, color2, gradient) = (caster.font.clone(), caster.layout.clone(), color_override.unwrap_or(caster.color), caster.color2, caster.use_gradient);
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
