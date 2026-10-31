//! Gameplay-relevant extraction from a level scene: collision geometry, spawn points, camera.

use std::path::Path;

use crate::projectile::DifficultyScaling;
use crate::scene::{FileId, GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// A collider shape in world space (z is discarded; the game is 2D).
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    /// One solid polygon (a `PolygonCollider2D` contributes one `Shape` per path; the original
    /// renders and collides each path as an independent solid).
    Polygon(Vec<[f32; 2]>),
    Circle { center: [f32; 2], radius: f32 },
    /// Open polyline.
    Edge(Vec<[f32; 2]>),
}

#[derive(Clone, Debug)]
pub struct Collider {
    pub object: FileId,
    pub name: String,
    pub layer: u8,
    pub tag: String,
    pub trigger: bool,
    /// True when the object (or an ancestor) has a `Rigidbody2D` and may therefore move.
    pub dynamic: bool,
    /// `Collider2D.enabled`. A disabled `PolygonCollider2D` takes no part in physics, but a
    /// `PolygonColliderRenderer` on its object still draws its path (the Lobby's panels).
    pub enabled: bool,
    /// Script class names attached to the owning GameObject.
    pub behaviours: Vec<String>,
    pub shape: Shape,
    /// How the level draws this polygon (`PolygonColliderRenderer`), when it has one.
    pub surface: Option<Surface>,
}

/// Render settings of a polygon path: the original builds the mesh from the path in *local*
/// space (UV = local position × tiling + offset) and places it with the object's transform.
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    /// GUID of the `.mat` asset.
    pub material: String,
    pub tiling: [f32; 2],
    pub offset: [f32; 2],
    /// Shadow copies are offset twice as far from the path (`isShadow`).
    pub is_shadow: bool,
    /// The path in local space (collider offset applied), index-matched with `Shape::Polygon`.
    pub local_path: Vec<[f32; 2]>,
    /// World position of the object's origin and the images of its local x / y axes in the xy
    /// plane, to place the mesh built from `local_path` (the original keeps the renderer mesh in
    /// local space and applies the object's transform).
    pub origin: [f32; 3],
    pub axis_x: [f32; 2],
    pub axis_y: [f32; 2],
    /// `PolygonColliderRendererShadower.shadowPlaneZ` when the object casts a polygon shadow (a
    /// black "Polygon Shadow" copy placed from the light every frame).
    pub shadow_plane_z: Option<f32>,
    /// `PolygonColliderRenderer.maximumTriangleArea` (default 600; 0 = no refinement).
    pub maximum_triangle_area: f64,
    /// A `FakeParallax` on the object or an ancestor, which moves the drawn mesh with the camera.
    pub parallax: Option<crate::sprites::Parallax>,
}

impl Collider {
    pub fn has_behaviour(&self, class_name: &str) -> bool {
        self.behaviours.iter().any(|b| b == class_name)
    }
}

#[derive(Clone, Debug)]
pub struct SpawnPoint {
    /// Child index under `Player Spawn Points` (the index `PlayerSpawner.SpawnAll` uses).
    pub index: usize,
    pub position: [f32; 3],
    /// World-space direction of the spawn transform's forward axis, projected to x.
    pub facing_right: bool,
}

/// `CameraPositioner` serialized fields (defaults are the class's field initializers).
#[derive(Clone, Debug, PartialEq)]
pub struct PositionerSettings {
    pub screen_edge_margin: f32,
    pub lowest_on_screen_y: f32,
    pub minimum_on_screen_width: f32,
    pub minimum_on_screen_height: f32,
    pub minimum_distance: f32,
    pub maximum_distance: f32,
    pub smooth_time_z: f32,
}

impl Default for PositionerSettings {
    fn default() -> Self {
        Self {
            screen_edge_margin: 25.0,
            lowest_on_screen_y: -1000.0,
            minimum_on_screen_width: 50.0,
            minimum_on_screen_height: 0.0,
            minimum_distance: 30.0,
            maximum_distance: 1000.0,
            smooth_time_z: 0.2,
        }
    }
}

#[derive(Clone, Debug)]
pub struct CameraSetup {
    pub position: [f32; 3],
    /// Vertical field of view in degrees.
    pub field_of_view: f32,
    pub near: f32,
    pub far: f32,
    pub background: [f32; 4],
    pub positioner: Option<PositionerSettings>,
}

#[derive(Clone, Debug)]
pub struct Level {
    pub name: String,
    pub colliders: Vec<Collider>,
    pub spawn_points: Vec<SpawnPoint>,
    pub camera: Option<CameraSetup>,
    /// World position of the scene's light, which sprite and line shadows are projected from
    /// (`SpriteShadowPositioner.GetSceneLight`; the original expects exactly one).
    pub light: Option<[f32; 3]>,
    /// The scene light in full (type, colour, range, and how it rides on the camera).
    pub light_setup: Option<LightSetup>,
    /// `RenderSettings` ambient light (`m_AmbientSkyColor * m_AmbientIntensity`; every level uses
    /// the flat ambient mode).
    pub ambient: [f32; 4],
    /// `AllowedWeapons.allowedWeapons` and `enemyAllowedWeapons`; `None` when the level has no
    /// such component (every weapon is allowed).
    pub allowed_weapons: Option<AllowedWeapons>,
    /// Enemies placed in the scene, in file-id order.
    pub enemies: Vec<EnemySpawn>,
    /// The level's `tk2dSprite` art (everything but the characters' own sprites).
    pub sprites: Vec<crate::sprites::SceneSprite>,
    /// The level's `tk2dTextMesh`es as they are once the level is loaded everywhere (see
    /// [`level_texts`]).
    pub texts: Vec<LevelText>,
    /// `HelicopterControl` vehicles.
    pub helicopters: Vec<crate::vehicles::HelicopterSpawn>,
    /// `EventSounds` start events: the music cue ("Level Music") and sounds played on `Start`.
    pub sounds: crate::audio::SceneSounds,
    /// GUIDs of the prefabs a `Prepooler` spawns and despawns when the level loads (their
    /// instances have run `Start` before gameplay, so their explosions are prepared).
    pub prepooled: Vec<String>,
    /// `RandomizeSpawnPoints` on "Player Spawn Points": campaign spawn points are shuffled too.
    pub randomize_spawn_points: bool,
    /// Built-in mesh renderers (Japan's sky plane).
    pub meshes: Vec<crate::scenery::SceneMesh>,
    /// `HorizontalScroller2D` bodies (Japan's clouds).
    pub scrollers: Vec<crate::scenery::Scroller>,
    /// Breakable objects (Japan's hanging lamps).
    pub props: Vec<crate::scenery::LevelProp>,
    /// The "Level Ender" set-ups that are active when the level loads.
    pub enders: Vec<LevelEnder>,
    /// `LevelTips.tips` (`None` without a `LevelTips` in the scene).
    pub tips: Option<Vec<LevelTip>>,
    /// The "Versus Game Ender" (versus levels).
    pub versus_ender: Option<VersusEnder>,
}

/// What `AllClientControlTypes` reports offline with the keyboard and mouse as the only input
/// (the sorted, comma-joined processor class names; `sticks_sim::devices::control_types_name`
/// builds it for other combinations).
pub const OFFLINE_CONTROL_TYPES: &str = "KeyboardAndMouseProcessor";

/// A level text: the text mesh itself (never a "Text Shadow" copy), its `TextShadower`
/// (`shadowFont` GUID, `shadowPlaneZ`) and whether it is an `ExplodableText` (split into
/// shootable letters on `OnLoadedLevelEverywhere`).
#[derive(Clone, Debug, PartialEq)]
pub struct LevelText {
    pub text: crate::fonts::SceneText,
    /// The object's layer: layer 5 ("UI") texts are seen by the UI camera, not the level's.
    pub layer: u8,
    pub shadower: Option<(Option<String>, f32)>,
    pub explodable: bool,
}

/// The texts that are active once `ControlTypeDependentActivator.ReceiveControlTypes` has run:
/// each active activator switches on its child named after the used control types (the
/// tutorial texts have one child per combination of control types).
pub fn level_texts(scene: &Scene, scripts: &ScriptIndex, control_types: &str) -> Vec<LevelText> {
    let class_of = |doc: &crate::yaml::Document| doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g));
    let has = |go: &GameObject<'_>, class_name: &str| {
        go.components().iter().any(|c| class_of(c) == Some(class_name) && c.body.bool_of("m_Enabled").unwrap_or(true))
    };
    let mut activated: Vec<FileId> = Vec::new();
    for go in scene.game_objects() {
        // An inactive activator never runs `Awake`, so it never subscribes.
        if go.active_in_hierarchy() && has(&go, "ControlTypeDependentActivator") {
            if let Some(child) = go.children().into_iter().find(|c| c.name() == control_types) {
                activated.push(child.id());
            }
        }
    }
    let active = |go: GameObject<'_>| {
        let mut current = Some(go);
        while let Some(o) = current {
            if !o.active_self() && !activated.contains(&o.id()) {
                return false;
            }
            current = o.parent();
        }
        true
    };
    let texts = crate::fonts::scene_texts(scene, scripts, true);
    let mut out = Vec::new();
    for (index, text) in texts.iter().enumerate() {
        if text.shadow_of.is_some() {
            continue;
        }
        let Some(go) = scene.game_object(text.object) else { continue };
        if !active(go) {
            continue;
        }
        let shadower = texts.iter().find_map(|t| match &t.shadow_of {
            Some((caster, font, plane)) if *caster == index => Some((font.clone(), *plane)),
            _ => None,
        });
        out.push(LevelText { text: text.clone(), layer: go.layer(), shadower, explodable: has(&go, "ExplodableText") });
    }
    out
}

/// One object on the way from the camera down to the light.
#[derive(Clone, Debug, PartialEq)]
pub struct LightLink {
    pub local_position: [f32; 3],
    /// `FixWorldZ.z` on this object: after it, the world z is pinned to this value (in
    /// `LateUpdate`, execution order 500).
    pub fix_world_z: Option<f32>,
}

/// The level's `Light`.
#[derive(Clone, Debug, PartialEq)]
pub struct LightSetup {
    /// `Light.m_Type`: 0 spot, 1 directional, 2 point.
    pub kind: i64,
    pub color: [f32; 4],
    pub intensity: f32,
    pub range: f32,
    /// World position when the scene loads.
    pub position: [f32; 3],
    /// When the light is (a descendant of) the camera: the objects from the camera's child down
    /// to the light. Every level parents it under `Camera/Fix Z`, so it follows the camera.
    /// TODO(UNCERTAIN): rotations and scales on that chain are assumed identity (they are in
    /// every scene shipped).
    pub camera_chain: Option<Vec<LightLink>>,
}

/// A `Targeter.Qualifier` component referenced by an enemy's `Targeter.qualifiers`.
#[derive(Clone, Debug, PartialEq)]
pub enum TargetQualifier {
    /// `MaximumDistanceTargetQualifier.maximumDistance`.
    MaximumDistance(f32),
    /// `LineOfSightTargetQualifier.obstructionLayers` (layer bit mask).
    LineOfSight(u32),
    /// `IsRootControlledTargetQualifier`.
    IsRootControlled,
    /// A qualifier class this reader does not know.
    Other(String),
}

/// `Targeter` settings.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TargeterSettings {
    pub qualifiers: Vec<TargetQualifier>,
    /// Number of `DistanceTargetSorter`s in `sorters` (the only sorter class).
    pub distance_sorters: usize,
}

/// `ProjectileLauncherModifier`: overrides of one of the character's launchers.
#[derive(Clone, Debug, PartialEq)]
pub struct LauncherModifier {
    /// Weapon pivot name (`Assault Rifle`, `Rocket Launcher`, ...).
    pub weapon: String,
    /// `firingRateTime`; negative keeps the launcher's own (and its fire mode).
    pub firing_rate_time: f32,
    /// GUID of the replacement projectile prefab.
    pub projectile: Option<String>,
}

/// An enemy placed in a level: a `Man` root with a child carrying `EnemyCharacterInput` and the
/// behaviours and modifiers that make it an enemy (`SetEnemyLayer`, `RemovePlayerOnlyParts`, ...).
#[derive(Clone, Debug, PartialEq)]
pub struct EnemySpawn {
    /// Name of the `Man` root (`Enemy`, `Enemy Still`, `Enemy Ducking`, ...).
    pub name: String,
    /// File id of the root object (what level scripts such as `DestroyParticularEnemyLevelEnder`
    /// refer to).
    pub object: FileId,
    /// An enabled `EnemyRegistrar` on the controller: the enemy announces itself on `Start`
    /// (`EnemyRegistrar.NewEnemy`), so the level enders count it.
    pub registered: bool,
    /// World position of the root (the character's feet).
    pub position: [f32; 3],
    /// Local position of the controller object under the root: the origin the targeter, the
    /// aim and the off-screen test measure from.
    pub controller_offset: [f32; 3],
    /// `SetStartingHealth.startingHealth`.
    pub starting_health: Option<f32>,
    /// `HealthDifficultyModifier` (applies to `SetStartingHealth`).
    pub health_scaling: Option<DifficultyScaling>,
    /// `SetEnemyWeapon.weapon`.
    pub selected_weapon: Option<String>,
    /// `ProjectileLauncherModifier`s in component order.
    pub launcher_modifiers: Vec<LauncherModifier>,
    /// `ProjectileLauncherFiringRateTimeDifficultyModifier`.
    pub firing_rate_scaling: Option<DifficultyScaling>,
    pub targeter: Option<TargeterSettings>,
    /// Enabled behaviours.
    pub fire_at_first_target: bool,
    pub move_to_first_target: bool,
    pub fire_and_move_if_necessary: bool,
    pub duck: bool,
    /// An enabled `Emerge`.
    pub emerge: Option<EmergeSpec>,
    /// `DeactivateOffScreen.radius`.
    pub deactivate_off_screen: Option<f32>,
    /// `CharacterColorSetter.bodyColor` (`Color32`) and `helmetColor` (`Color`).
    pub body_color: Option<[u8; 4]>,
    pub helmet_color: Option<[f32; 4]>,
}

/// `Emerge`: the enemy waits behind the scenery (z 0, layer 25, its firing subsumed) until a
/// player enters `trigger_object`'s `TriggeredByAnyPlayer`, walks `point_x` sideways aiming, then
/// takes its place (z -0.1, its own layer) and `post_emerge_wait_time` later starts fighting.
#[derive(Clone, Debug, PartialEq)]
pub struct EmergeSpec {
    pub point_x: f32,
    /// The object carrying the activation `TriggeredByAnyPlayer` (and its trigger collider).
    pub trigger_object: Option<FileId>,
    pub post_emerge_wait_time: f32,
    /// Classes of `unsubsumeOnCompletion` (subsumed until done).
    pub unsubsume_on_completion: Vec<String>,
    /// Classes of `enableOnCompletion` (disabled until done).
    pub enable_on_completion: Vec<String>,
}

/// The first object in `go`'s tree (itself first) carrying `class_name`.
fn first_in_children(go: &GameObject<'_>, scripts: &ScriptIndex, class_name: &str) -> Option<FileId> {
    if mono_fields(go, scripts, class_name).is_some() {
        return Some(go.id());
    }
    go.children().iter().find_map(|c| first_in_children(c, scripts, class_name))
}

pub(crate) fn scaling_of(body: &Value) -> DifficultyScaling {
    DifficultyScaling {
        amount: body.str_of("amount").unwrap_or("").to_owned(),
        affected_by_player_count: body.bool_of("affectedByPlayerCount").unwrap_or(true),
    }
}

fn script_name<'s>(doc: &crate::yaml::Document, scripts: &'s ScriptIndex) -> Option<&'s str> {
    doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g))
}

/// The enemy whose controller object is `controller` (it carries `EnemyCharacterInput`).
pub(crate) fn enemy_of(scene: &Scene, controller: &GameObject<'_>, scripts: &ScriptIndex) -> EnemySpawn {
    let mut root = *controller;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    // The offset is composed of local positions only: the root's own rotation is replaced by
    // `CharacterControl2D` (a rotation about y), and the authored offsets lie on the y axis.
    let mut offset = [0.0f32; 3];
    let mut current = Some(*controller);
    while let Some(go) = current {
        if go.id() == root.id() {
            break;
        }
        let p = go.transform().and_then(|t| t.body.get("m_LocalPosition")).and_then(Value::as_vec3).unwrap_or((0.0, 0.0, 0.0));
        offset = [offset[0] + p.0, offset[1] + p.1, offset[2] + p.2];
        current = go.parent();
    }
    let mut enemy = EnemySpawn {
        name: root.name().to_owned(),
        object: root.id(),
        registered: false,
        position: root.world_affine().t,
        controller_offset: offset,
        starting_health: None,
        health_scaling: None,
        selected_weapon: None,
        launcher_modifiers: Vec::new(),
        firing_rate_scaling: None,
        targeter: None,
        fire_at_first_target: false,
        move_to_first_target: false,
        fire_and_move_if_necessary: false,
        duck: false,
        emerge: None,
        deactivate_off_screen: None,
        body_color: None,
        helmet_color: None,
    };
    for component in controller.components() {
        if component.class_id != class::MONO_BEHAVIOUR {
            continue;
        }
        let b = &component.body;
        let enabled = b.bool_of("m_Enabled").unwrap_or(true);
        match script_name(component, scripts) {
            Some("SetStartingHealth") => enemy.starting_health = b.f32_of("startingHealth"),
            Some("HealthDifficultyModifier") => enemy.health_scaling = Some(scaling_of(b)),
            Some("SetEnemyWeapon") => enemy.selected_weapon = b.str_of("weapon").map(str::to_owned),
            Some("ProjectileLauncherModifier") => enemy.launcher_modifiers.push(LauncherModifier {
                weapon: b.str_of("name").unwrap_or("").to_owned(),
                firing_rate_time: b.f32_of("firingRateTime").unwrap_or(-1.0),
                projectile: b.get("projectilePrefab").and_then(Value::guid).map(str::to_owned),
            }),
            Some("ProjectileLauncherFiringRateTimeDifficultyModifier") => enemy.firing_rate_scaling = Some(scaling_of(b)),
            Some("FireAtFirstTarget") => enemy.fire_at_first_target = enabled,
            Some("MoveToFirstTarget") => enemy.move_to_first_target = enabled,
            Some("FireAndMoveIfNecessary") => enemy.fire_and_move_if_necessary = enabled,
            Some("Duck") => enemy.duck = enabled,
            Some("Emerge") if enabled => {
                // A component reference → the class of that component.
                let classes = |key: &str| -> Vec<String> {
                    b.get(key).map(Value::as_seq).unwrap_or(&[]).iter().filter_map(|r| scene.get(r.file_id()?)).filter_map(|d| script_name(d, scripts).map(str::to_owned)).collect()
                };
                // `activationTrigger` (a `TriggeredByAnyPlayer`), else the first one below the
                // controller (`GetComponentInChildren`).
                let trigger_object = b
                    .get("activationTrigger")
                    .and_then(Value::file_id)
                    .filter(|&id| id != 0)
                    .and_then(|id| scene.get(id))
                    .and_then(|d| d.body.get("m_GameObject").and_then(Value::file_id))
                    .or_else(|| first_in_children(controller, scripts, "TriggeredByAnyPlayer"));
                enemy.emerge = Some(EmergeSpec {
                    point_x: b.f32_of("emergePointX").unwrap_or(-3.0),
                    trigger_object,
                    post_emerge_wait_time: b.f32_of("postEmergeWaitTime").unwrap_or(0.4),
                    unsubsume_on_completion: classes("unsubsumeOnCompletion"),
                    enable_on_completion: classes("enableOnCompletion"),
                });
            }
            Some("EnemyRegistrar") => enemy.registered = enabled,
            Some("DeactivateOffScreen") if enabled => enemy.deactivate_off_screen = Some(b.f32_of("radius").unwrap_or(10.0)),
            Some("CharacterColorSetter") => {
                // `Color32` serializes as one little-endian `rgba` integer.
                enemy.body_color = b.get("bodyColor").and_then(|c| c.i64_of("rgba")).map(|rgba| (rgba as u32).to_le_bytes());
                enemy.helmet_color = b.get("helmetColor").map(|c| ["r", "g", "b", "a"].map(|k| c.f32_of(k).unwrap_or(1.0)));
            }
            Some("Targeter") => {
                let references = |key: &str| -> Vec<&crate::yaml::Document> {
                    b.get(key).map(Value::as_seq).unwrap_or(&[]).iter().filter_map(|r| scene.get(r.file_id()?)).collect()
                };
                let qualifiers = references("qualifiers")
                    .into_iter()
                    .map(|q| match script_name(q, scripts) {
                        Some("MaximumDistanceTargetQualifier") => TargetQualifier::MaximumDistance(q.body.f32_of("maximumDistance").unwrap_or(0.0)),
                        Some("LineOfSightTargetQualifier") => {
                            TargetQualifier::LineOfSight(q.body.get("obstructionLayers").and_then(|m| m.i64_of("m_Bits")).unwrap_or(0) as u32)
                        }
                        Some("IsRootControlledTargetQualifier") => TargetQualifier::IsRootControlled,
                        other => TargetQualifier::Other(other.unwrap_or("?").to_owned()),
                    })
                    .collect();
                let distance_sorters = references("sorters").into_iter().filter(|s| script_name(s, scripts) == Some("DistanceTargetSorter")).count();
                enemy.targeter = Some(TargeterSettings { qualifiers, distance_sorters });
            }
            _ => {}
        }
    }
    enemy
}

/// `AllowedWeapons`: which of the character's weapons a level keeps.
#[derive(Clone, Debug, PartialEq)]
pub struct AllowedWeapons {
    pub players: Vec<String>,
    /// Used for enemies when non-empty (`IsWeaponAllowed`).
    pub enemies: Vec<String>,
}

impl AllowedWeapons {
    /// `AllowedWeapons.IsWeaponAllowed`.
    pub fn allows(&self, weapon: &str, for_enemy: bool) -> bool {
        let list = if for_enemy && !self.enemies.is_empty() { &self.enemies } else { &self.players };
        list.iter().any(|w| w == weapon)
    }
}

fn behaviours_of(go: &GameObject<'_>, scripts: &ScriptIndex) -> Vec<String> {
    go.components()
        .into_iter()
        .filter(|c| c.class_id == class::MONO_BEHAVIOUR)
        .filter_map(|c| {
            let guid = c.body.get("m_Script")?.guid()?;
            scripts.class_name(guid).map(str::to_owned)
        })
        .collect()
}

fn mono_fields<'a>(go: &GameObject<'a>, scripts: &ScriptIndex, class_name: &str) -> Option<&'a Value> {
    go.components().into_iter().filter(|c| c.class_id == class::MONO_BEHAVIOUR).find_map(|c| {
        let guid = c.body.get("m_Script")?.guid()?;
        (scripts.class_name(guid)? == class_name).then_some(&c.body)
    })
}

fn xy(p: [f32; 3]) -> [f32; 2] {
    [p[0], p[1]]
}

/// Every collider of the scene, also those under inactive objects (for scenes such as the
/// Lobby, which switch parts on and off).
pub fn all_colliders(scene: &Scene, scripts: &ScriptIndex) -> Vec<Collider> {
    let mut out = Vec::new();
    for go in &scene.game_objects() {
        colliders_of(go, scripts, &mut out, true);
    }
    out
}

fn colliders_of(go: &GameObject<'_>, scripts: &ScriptIndex, out: &mut Vec<Collider>, include_inactive: bool) {
    if !include_inactive && !go.active_in_hierarchy() {
        return;
    }
    let shadow_plane_z = mono_fields(go, scripts, "PolygonColliderRendererShadower").filter(|b| b.bool_of("m_Enabled").unwrap_or(true)).map(|b| b.f32_of("shadowPlaneZ").unwrap_or(0.0));
    let world = go.world_affine();
    let behaviours = behaviours_of(go, scripts);
    let dynamic = go.has_rigidbody_2d_in_parents();
    let renderer = mono_fields(go, scripts, "PolygonColliderRenderer").filter(|b| b.bool_of("m_Enabled").unwrap_or(true));
    let push = |shape: Shape, trigger: bool, surface: Option<Surface>, enabled: bool, out: &mut Vec<Collider>| {
        out.push(Collider {
            object: go.id(),
            name: go.name().to_owned(),
            layer: go.layer(),
            tag: go.tag().to_owned(),
            trigger,
            dynamic,
            enabled,
            behaviours: behaviours.clone(),
            shape,
            surface,
        });
    };
    for comp in go.components() {
        let enabled = comp.body.bool_of("m_Enabled").unwrap_or(true);
        // Only a renderer-carrying polygon is of any use while disabled.
        if !enabled && !(comp.class_id == class::POLYGON_COLLIDER_2D && renderer.is_some()) {
            continue;
        }
        let trigger = comp.body.bool_of("m_IsTrigger").unwrap_or(false);
        let offset = comp.body.get("m_Offset").and_then(Value::as_vec2).unwrap_or((0.0, 0.0));
        match comp.class_id {
            class::POLYGON_COLLIDER_2D => {
                let paths = comp.body.get("m_Points").and_then(|p| p.get("m_Paths")).map(Value::as_seq).unwrap_or(&[]);
                for path in paths {
                    let pts: Vec<[f32; 2]> = path
                        .as_seq()
                        .iter()
                        .filter_map(Value::as_vec2)
                        .map(|(x, y)| xy(world.transform_point([x + offset.0, y + offset.1, 0.0])))
                        .collect();
                    if pts.len() >= 3 {
                        let surface = renderer.and_then(|r| {
                            let material = r.get("material")?.guid()?.to_owned();
                            let pair = |k: &str, default: f32| {
                                r.get(k).and_then(Value::as_vec2).map_or([default, default], |(x, y)| [x, y])
                            };
                            let local_path = path
                                .as_seq()
                                .iter()
                                .filter_map(Value::as_vec2)
                                .map(|(x, y)| [x + offset.0, y + offset.1])
                                .collect();
                            Some(Surface {
                                material,
                                tiling: pair("tiling", 1.0),
                                offset: pair("offset", 0.0),
                                is_shadow: r.bool_of("isShadow").unwrap_or(false),
                                local_path,
                                origin: world.t,
                                axis_x: xy(world.transform_vector([1.0, 0.0, 0.0])),
                                axis_y: xy(world.transform_vector([0.0, 1.0, 0.0])),
                                shadow_plane_z,
                                maximum_triangle_area: r.str_of("maximumTriangleArea").and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(600.0),
                                parallax: crate::sprites::parallax_of(*go, &|c: &crate::yaml::Document| script_name(c, scripts)),
                            })
                        });
                        push(Shape::Polygon(pts), trigger, surface, enabled, out);
                    }
                }
            }
            class::BOX_COLLIDER_2D => {
                let Some((sx, sy)) = comp.body.get("m_Size").and_then(Value::as_vec2) else { continue };
                let (hx, hy) = (sx * 0.5, sy * 0.5);
                let corners = [(-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)];
                let pts = corners
                    .iter()
                    .map(|(cx, cy)| xy(world.transform_point([cx + offset.0, cy + offset.1, 0.0])))
                    .collect();
                push(Shape::Polygon(pts), trigger, None, enabled, out);
            }
            class::CIRCLE_COLLIDER_2D => {
                let radius = comp.body.f32_of("m_Radius").unwrap_or(0.5);
                let s = world.lossy_scale();
                let center = xy(world.transform_point([offset.0, offset.1, 0.0]));
                push(Shape::Circle { center, radius: radius * s[0].abs().max(s[1].abs()) }, trigger, None, enabled, out);
            }
            class::EDGE_COLLIDER_2D => {
                let pts: Vec<[f32; 2]> = comp
                    .body
                    .get("m_Points")
                    .map(Value::as_seq)
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(Value::as_vec2)
                    .map(|(x, y)| xy(world.transform_point([x + offset.0, y + offset.1, 0.0])))
                    .collect();
                if pts.len() >= 2 {
                    push(Shape::Edge(pts), trigger, None, enabled, out);
                }
            }
            _ => {}
        }
    }
}

/// Extract the gameplay-relevant parts of a parsed level scene.
pub fn extract_level(name: &str, scene: &Scene, scripts: &ScriptIndex) -> Level {
    extract_level_with(name, scene, scripts, OFFLINE_CONTROL_TYPES)
}

/// [`extract_level`] for the control types in use (`AllClientControlTypes`, which picks the
/// `ControlTypeDependentActivator` children).
pub fn extract_level_with(name: &str, scene: &Scene, scripts: &ScriptIndex, control_types: &str) -> Level {
    let objects = scene.game_objects();
    let mut colliders = Vec::new();
    for go in &objects {
        colliders_of(go, scripts, &mut colliders, false);
    }

    // `PlayerSpawner.SpawnAll` addresses spawn points by child index under "/Player Spawn Points".
    let mut spawn_points = Vec::new();
    let mut randomize_spawn_points = false;
    if let Some(parent) = objects.iter().find(|g| g.name() == "Player Spawn Points" && g.parent().is_none()) {
        randomize_spawn_points = parent.components().into_iter().any(|c| script_name(c, scripts) == Some("RandomizeSpawnPoints"));
        for (index, child) in parent.children().into_iter().enumerate() {
            let world = child.world_affine();
            let forward = world.transform_vector([0.0, 0.0, 1.0]);
            spawn_points.push(SpawnPoint { index, position: world.t, facing_right: forward[0] >= 0.0 });
        }
    }

    let camera = objects.iter().find_map(|go| {
        let cam = go.component(class::CAMERA)?;
        if !go.active_in_hierarchy() || !cam.body.bool_of("m_Enabled").unwrap_or(true) {
            return None;
        }
        let bg = cam.body.get("m_BackGroundColor");
        let channel = |k: &str| bg.and_then(|c| c.f32_of(k)).unwrap_or(0.0);
        let positioner = mono_fields(go, scripts, "CameraPositioner").map(|f| {
            let d = PositionerSettings::default();
            PositionerSettings {
                screen_edge_margin: f.f32_of("screenEdgeMargin").unwrap_or(d.screen_edge_margin),
                lowest_on_screen_y: f.f32_of("lowestOnScreenY").unwrap_or(d.lowest_on_screen_y),
                minimum_on_screen_width: f.f32_of("minimumOnScreenWidth").unwrap_or(d.minimum_on_screen_width),
                minimum_on_screen_height: f.f32_of("minimumOnScreenHeight").unwrap_or(d.minimum_on_screen_height),
                minimum_distance: f.f32_of("minimumDistance").unwrap_or(d.minimum_distance),
                maximum_distance: f.f32_of("maximumDistance").unwrap_or(d.maximum_distance),
                smooth_time_z: f.f32_of("smoothTimeZ").unwrap_or(d.smooth_time_z),
            }
        });
        Some(CameraSetup {
            position: go.world_affine().t,
            field_of_view: cam.body.f32_of("field of view").unwrap_or(60.0),
            near: cam.body.f32_of("near clip plane").unwrap_or(0.3),
            far: cam.body.f32_of("far clip plane").unwrap_or(1000.0),
            background: [channel("r"), channel("g"), channel("b"), channel("a")],
            positioner,
        })
    });

    let light_object = objects
        .iter()
        .find(|go| go.active_in_hierarchy() && go.component(class::LIGHT).is_some_and(|l| l.body.bool_of("m_Enabled").unwrap_or(true)));
    let light = light_object.map(|go| go.world_affine().t);
    let light_setup = light_object.map(|go| {
        let l = &go.component(class::LIGHT).expect("found by its light").body;
        let c = l.get("m_Color");
        let mut chain = Vec::new();
        let mut current = Some(*go);
        let mut on_camera = false;
        while let Some(g) = current {
            if g.component(class::CAMERA).is_some() {
                on_camera = true;
                break;
            }
            chain.push(LightLink {
                local_position: g.transform().and_then(|t| t.body.get("m_LocalPosition")).and_then(Value::as_vec3).map_or([0.0; 3], |(x, y, z)| [x, y, z]),
                fix_world_z: mono_fields(&g, scripts, "FixWorldZ").and_then(|f| f.f32_of("z")),
            });
            current = g.parent();
        }
        chain.reverse();
        LightSetup {
            kind: l.i64_of("m_Type").unwrap_or(2),
            color: ["r", "g", "b", "a"].map(|k| c.and_then(|c| c.f32_of(k)).unwrap_or(1.0)),
            intensity: l.f32_of("m_Intensity").unwrap_or(1.0),
            range: l.f32_of("m_Range").unwrap_or(10.0),
            position: go.world_affine().t,
            camera_chain: on_camera.then_some(chain),
        }
    });
    let ambient = scene
        .documents()
        .find(|d| d.class_id == 104)
        .map(|r| {
            let c = r.body.get("m_AmbientSkyColor");
            let k = r.body.f32_of("m_AmbientIntensity").unwrap_or(1.0);
            let ch = |n: &str| c.and_then(|c| c.f32_of(n)).unwrap_or(0.2);
            [ch("r") * k, ch("g") * k, ch("b") * k, 1.0]
        })
        .unwrap_or([0.2, 0.2, 0.2, 1.0]);

    let allowed_weapons = objects.iter().find_map(|go| {
        let f = mono_fields(go, scripts, "AllowedWeapons")?;
        let names = |key: &str| f.get(key).map(|v| v.as_seq().iter().filter_map(|n| n.as_str().map(str::to_owned)).collect()).unwrap_or_default();
        Some(AllowedWeapons { players: names("allowedWeapons"), enemies: names("enemyAllowedWeapons") })
    });

    let enemies = objects
        .iter()
        .filter(|go| go.active_in_hierarchy() && behaviours_of(go, scripts).iter().any(|b| b == "EnemyCharacterInput"))
        .map(|go| enemy_of(scene, go, scripts))
        .collect();

    let sprites = crate::sprites::scene_sprites(scene, scripts);
    let texts = level_texts(scene, scripts, control_types);
    let helicopters = crate::vehicles::helicopters(scene, scripts);
    let sounds = crate::audio::scene_sounds(scene, scripts);
    let prepooled = scene
        .documents()
        .filter(|d| d.class_id == crate::scene::class::MONO_BEHAVIOUR)
        .filter(|d| d.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g)) == Some("Prepooler"))
        .filter_map(|d| d.body.get("prefab").and_then(Value::guid).map(str::to_owned))
        .collect();
    let enders = level_enders(scene, scripts);
    let tips = objects.iter().filter(|go| go.active_in_hierarchy()).find_map(|go| mono_fields(go, scripts, "LevelTips")).map(|b| {
        b.get("tips")
            .map(Value::as_seq)
            .unwrap_or(&[])
            .iter()
            .map(|t| LevelTip {
                applicable_control_types: t.get("applicableControlTypes").map(Value::as_seq).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
                message: t.str_of("message").unwrap_or("").to_owned(),
            })
            .collect()
    });
    let meshes = crate::scenery::scene_meshes(scene, scripts);
    let scrollers = crate::scenery::scrollers(scene, scripts);
    let props = crate::scenery::props(scene, scripts, &colliders, &sprites);
    Level {
        name: name.to_owned(),
        colliders,
        spawn_points,
        camera,
        light,
        light_setup,
        ambient,
        allowed_weapons,
        enemies,
        sprites,
        texts,
        helicopters,
        sounds,
        prepooled,
        randomize_spawn_points,
        meshes,
        scrollers,
        props,
        enders,
        tips,
        versus_ender: versus_ender(scene, scripts),
    }
}

/// One `LevelTips.LevelTip`.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelTip {
    /// Processor class names that must all be in use for the tip to apply (empty: always).
    pub applicable_control_types: Vec<String>,
    pub message: String,
}

/// What switches a level ender's "Ender" object on.
#[derive(Clone, Debug, PartialEq)]
pub enum EnderActivator {
    /// `ActivateSomethingOnTriggered` with its `TriggeredByAllPlayers` (the `trigger` field, or
    /// `GetComponentInChildren` at `Start`, which only finds active ones). `None`: no trigger
    /// was found, so the activator never fires (its `Update` throws every frame in the
    /// original).
    AllPlayersTrigger { trigger: Option<FileId> },
    /// `DestroyAllEnemiesLevelEnder`: every registered enemy exhausted; its `countdownLabel`.
    DestroyAllEnemies { countdown_label: Option<FileId> },
    /// `DestroyParticularEnemyLevelEnder`: the root object of the `enemy` health.
    DestroyParticularEnemy { enemy: Option<FileId> },
}

/// The "Ender" object a level ender activates and the end-of-level scripts on it.
#[derive(Clone, Debug, PartialEq)]
pub struct EndOfLevel {
    pub object: FileId,
    /// World position of the object (`EventSounds` plays there).
    pub position: [f32; 3],
    /// `EndOfLevelUIActivator`.
    pub ui: bool,
    /// `EndOfLevelCameraMover.minimumDistance`.
    pub camera_minimum_distance: Option<f32>,
    /// `EndOfLevelTimeSlower`.
    pub time_slower: bool,
    /// `DisableGameOvering`.
    pub disable_game_overing: bool,
    /// `LevelComplete`.
    pub level_complete: bool,
    /// `ExplodeSomethingOnStart.toExplode`.
    pub explode: Option<FileId>,
    /// `EventSounds` start sound groups (the "Applause").
    pub start_sounds: Vec<String>,
    /// `EndOfVersusGameUIActivator` (the Versus Ender's screen instead of "End of Level").
    pub versus_ui: bool,
    /// `EndOfVersusGameLoserDestroyer`.
    pub loser_destroyer: bool,
}

/// The "Versus Game Ender": `VersusGameEnder` and the object it activates when the frag limit
/// is reached.
#[derive(Clone, Debug, PartialEq)]
pub struct VersusEnder {
    pub object: FileId,
    pub ender: Option<EndOfLevel>,
}

/// The scripts of an "Ender" object.
fn end_of_level(scene: &Scene, scripts: &ScriptIndex, e: GameObject<'_>) -> EndOfLevel {
    let has = |n: &str| mono_fields(&e, scripts, n).is_some_and(|b| b.bool_of("m_Enabled").unwrap_or(true));
    let start_sounds = mono_fields(&e, scripts, "EventSounds")
        .and_then(|b| b.get("startSound").and_then(|s| s.get("SoundEvents")))
        .map(Value::as_seq)
        .unwrap_or(&[])
        .iter()
        .filter(|ev| ev.i64_of("currentSoundFunctionType").unwrap_or(0) == 0)
        .filter_map(|ev| ev.str_of("soundType").filter(|s| !s.is_empty() && *s != "[None]").map(str::to_owned))
        .collect();
    EndOfLevel {
        object: e.id(),
        position: e.world_affine().t,
        ui: has("EndOfLevelUIActivator"),
        camera_minimum_distance: mono_fields(&e, scripts, "EndOfLevelCameraMover").map(|b| b.f32_of("minimumDistance").unwrap_or(14.0)),
        time_slower: has("EndOfLevelTimeSlower"),
        disable_game_overing: has("DisableGameOvering"),
        level_complete: has("LevelComplete"),
        explode: mono_fields(&e, scripts, "ExplodeSomethingOnStart").and_then(|b| referenced_object(scene, b.get("toExplode"))).map(|t| t.id()),
        start_sounds,
        versus_ui: has("EndOfVersusGameUIActivator"),
        loser_destroyer: has("EndOfVersusGameLoserDestroyer"),
    }
}

/// The active, enabled `VersusGameEnder` of the scene and its `toActivate`.
pub fn versus_ender(scene: &Scene, scripts: &ScriptIndex) -> Option<VersusEnder> {
    scene.game_objects().into_iter().filter(|go| go.active_in_hierarchy()).find_map(|go| {
        let b = mono_fields(&go, scripts, "VersusGameEnder").filter(|b| b.bool_of("m_Enabled").unwrap_or(true))?;
        let ender = referenced_object(scene, b.get("toActivate")).map(|e| end_of_level(scene, scripts, e));
        Some(VersusEnder { object: go.id(), ender })
    })
}

/// The `Explodable` "Marker" (an exclamation mark drawn with `PolygonColliderOutlineLines`).
#[derive(Clone, Debug, PartialEq)]
pub struct EnderMarker {
    pub object: FileId,
    /// `activeInHierarchy` when the level loads.
    pub active: bool,
    /// World position of the object (its `LinesRenderer` draws relative to it).
    pub position: [f32; 3],
    /// The `PolygonCollider2D` paths in world space (each closed).
    pub outline: Vec<Vec<[f32; 3]>>,
    /// `PolygonColliderOutlineLines.color` (`Color32`), `lineThickness`, `shadowPlaneZ`.
    pub color: [u8; 4],
    pub line_thickness: f32,
    pub shadow_plane_z: f32,
    /// `ExplodeSpritesAndLines.fadeWaitTime`.
    pub fade_wait_time: f32,
}

/// A "Level Ender": `ActivateSomethingOnTriggered` (+ optionally an enemy-count ender) that
/// activates "Ender" (`LevelComplete` and friends); `GameOverUIActivator` and
/// `EndOfLevelEnemyDestroyer` ride on the same object.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelEnder {
    pub object: FileId,
    pub name: String,
    pub activators: Vec<EnderActivator>,
    pub ender: Option<EndOfLevel>,
    /// `GameOverUIActivator` (enabled).
    pub game_over_activator: bool,
    /// `EndOfLevelEnemyDestroyer`.
    pub enemy_destroyer: bool,
    pub markers: Vec<EnderMarker>,
}

/// The root object of a reference (`{fileID}` to a component or a game object).
fn referenced_object<'a>(scene: &'a Scene, value: Option<&Value>) -> Option<GameObject<'a>> {
    let id = value?.file_id().filter(|&id| id != 0)?;
    if let Some(go) = scene.game_object(id) {
        return Some(go);
    }
    let owner = scene.get(id)?.body.get("m_GameObject")?.file_id()?;
    scene.game_object(owner)
}

fn root_object(go: GameObject<'_>) -> GameObject<'_> {
    let mut root = go;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    root
}

/// `GetComponentInChildren<T>()`: the object itself, then its children depth first, active
/// objects only.
fn find_in_active_children<'a>(go: GameObject<'a>, scripts: &ScriptIndex, class_name: &str) -> Option<GameObject<'a>> {
    if !go.active_in_hierarchy() {
        return None;
    }
    if mono_fields(&go, scripts, class_name).is_some_and(|b| b.bool_of("m_Enabled").unwrap_or(true)) {
        return Some(go);
    }
    go.children().into_iter().find_map(|c| find_in_active_children(c, scripts, class_name))
}

/// Every active object with an `ActivateSomethingOnTriggered`, `DestroyAllEnemiesLevelEnder`
/// or `DestroyParticularEnemyLevelEnder`, with what it switches on.
pub fn level_enders(scene: &Scene, scripts: &ScriptIndex) -> Vec<LevelEnder> {
    let mut out = Vec::new();
    for go in scene.game_objects() {
        if !go.active_in_hierarchy() {
            continue;
        }
        let mut activators = Vec::new();
        let mut targets = Vec::new();
        for c in go.components() {
            if c.class_id != class::MONO_BEHAVIOUR || !c.body.bool_of("m_Enabled").unwrap_or(true) {
                continue;
            }
            let b = &c.body;
            let target = referenced_object(scene, b.get("toActivate"));
            match script_name(c, scripts) {
                Some("ActivateSomethingOnTriggered") => {
                    let trigger = referenced_object(scene, b.get("trigger")).or_else(|| find_in_active_children(go, scripts, "TriggeredByAllPlayers"));
                    activators.push(EnderActivator::AllPlayersTrigger { trigger: trigger.map(|t| t.id()) });
                }
                Some("DestroyAllEnemiesLevelEnder") => {
                    activators.push(EnderActivator::DestroyAllEnemies { countdown_label: referenced_object(scene, b.get("countdownLabel")).map(|t| t.id()) });
                }
                Some("DestroyParticularEnemyLevelEnder") => {
                    let enemy = referenced_object(scene, b.get("enemy")).map(|e| root_object(e).id());
                    activators.push(EnderActivator::DestroyParticularEnemy { enemy });
                }
                _ => continue,
            }
            targets.push(target);
        }
        if activators.is_empty() {
            continue;
        }
        // TODO(UNCERTAIN): every level's activators share one `toActivate` ("Ender"); a level
        // whose activators point at different objects would only get the first one's.
        let ender = targets.into_iter().flatten().next().map(|e| end_of_level(scene, scripts, e));
        let has = |n: &str| mono_fields(&go, scripts, n).is_some_and(|b| b.bool_of("m_Enabled").unwrap_or(true));
        let mut markers = Vec::new();
        let mut stack = go.children();
        while let Some(child) = stack.pop() {
            stack.extend(child.children());
            let Some(lines) = mono_fields(&child, scripts, "PolygonColliderOutlineLines") else { continue };
            let world = child.world_affine();
            let outline = child
                .component(class::POLYGON_COLLIDER_2D)
                .map(|p| {
                    let offset = p.body.get("m_Offset").and_then(Value::as_vec2).unwrap_or((0.0, 0.0));
                    p.body
                        .get("m_Points")
                        .and_then(|m| m.get("m_Paths"))
                        .map(Value::as_seq)
                        .unwrap_or(&[])
                        .iter()
                        .map(|path| path.as_seq().iter().filter_map(Value::as_vec2).map(|(x, y)| world.transform_point([x + offset.0, y + offset.1, 0.0])).collect())
                        .collect()
                })
                .unwrap_or_default();
            markers.push(EnderMarker {
                object: child.id(),
                active: child.active_in_hierarchy(),
                position: world.t,
                outline,
                color: lines.get("color").and_then(|c| c.i64_of("rgba")).map_or([255; 4], |rgba| (rgba as u32).to_le_bytes()),
                line_thickness: lines.f32_of("lineThickness").unwrap_or(0.1),
                shadow_plane_z: lines.f32_of("shadowPlaneZ").unwrap_or(0.0),
                fade_wait_time: mono_fields(&child, scripts, "ExplodeSpritesAndLines").and_then(|b| b.f32_of("fadeWaitTime")).unwrap_or(2.0),
            });
        }
        out.push(LevelEnder {
            object: go.id(),
            name: go.name().to_owned(),
            activators,
            ender,
            game_over_activator: has("GameOverUIActivator"),
            enemy_destroyer: has("EndOfLevelEnemyDestroyer"),
            markers,
        });
    }
    out
}

/// Load `Assets/Game/Levels/<folder>/<name>.unity` style paths relative to the exported project
/// root (the directory that contains `Assets/`).
pub fn load_level(export_root: &Path, scene_path: &str) -> std::io::Result<Level> {
    load_level_for(export_root, scene_path, None)
}

/// What `LevelLoader` knows when the level's `Awake`s run: `PlayerCount` and `Difficulty`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AwakeConditions {
    pub player_count: usize,
    pub difficulty: i32,
}

/// [`load_level`] with the objects that `MinimumPlayers`, `MinimumDifficulty` and
/// `ActivateOrDestroyAccordingToDifficultyLevel` destroy (or activate) in `Awake` already gone
/// (or switched on). `None` reads the scene as saved.
pub fn load_level_for(export_root: &Path, scene_path: &str, conditions: Option<AwakeConditions>) -> std::io::Result<Level> {
    load_level_with(export_root, scene_path, conditions, OFFLINE_CONTROL_TYPES)
}

/// [`load_level_for`] with the control types in use (see [`extract_level_with`]).
pub fn load_level_with(export_root: &Path, scene_path: &str, conditions: Option<AwakeConditions>, control_types: &str) -> std::io::Result<Level> {
    let scripts = ScriptIndex::scan(&export_root.join("Assets/Scripts"));
    let mut scene = Scene::load(&export_root.join(scene_path))?;
    if let Some(conditions) = conditions {
        apply_awake_gates(&mut scene, &scripts, conditions);
    }
    let name = Path::new(scene_path).file_stem().and_then(|s| s.to_str()).unwrap_or("level");
    Ok(extract_level_with(name, &scene, &scripts, control_types))
}

/// The `Awake` of every active object's level-set-up gates:
/// * `MinimumPlayers` (`minimumPlayers`, default 2): destroyed below that player count;
/// * `MinimumDifficulty` (`minimumDifficultyLevel`, default 0): destroyed below that difficulty;
/// * `ActivateOrDestroyAccordingToDifficultyLevel`: of `toActivate`, entry `Difficulty + 1` is
///   switched on and every other one destroyed.
///
/// A destroyed object is switched off here, which hides it (and its children) from every reader.
/// Returns the objects switched off.
pub fn apply_awake_gates(scene: &mut Scene, scripts: &ScriptIndex, conditions: AwakeConditions) -> Vec<FileId> {
    let mut off: Vec<FileId> = Vec::new();
    let mut on: Vec<FileId> = Vec::new();
    for go in scene.game_objects() {
        if !go.active_in_hierarchy() {
            continue;
        }
        if let Some(b) = mono_fields(&go, scripts, "MinimumPlayers") {
            if (conditions.player_count as i64) < b.i64_of("minimumPlayers").unwrap_or(2) {
                off.push(go.id());
            }
        }
        if let Some(b) = mono_fields(&go, scripts, "MinimumDifficulty") {
            if i64::from(conditions.difficulty) < b.i64_of("minimumDifficultyLevel").unwrap_or(0) {
                off.push(go.id());
            }
        }
        if let Some(b) = mono_fields(&go, scripts, "ActivateOrDestroyAccordingToDifficultyLevel") {
            let chosen = (conditions.difficulty + 1) as usize;
            for (i, target) in b.get("toActivate").map(Value::as_seq).unwrap_or(&[]).iter().enumerate() {
                let Some(id) = target.file_id().filter(|&id| id != 0) else { continue };
                if i == chosen { on.push(id) } else { off.push(id) }
            }
        }
    }
    for &id in &on {
        scene.set_active_self(id, true);
    }
    for &id in &off {
        scene.set_active_self(id, false);
    }
    off
}
