//! Level scenery that is not a `tk2dSprite` or a `PolygonColliderRenderer`:
//!
//! * built-in mesh renderers ([`SceneMesh`]: Japan's sky is a `MeshRenderer` with Unity's
//!   "Plane" mesh and a "Color Polygon" material, under a `FakeParallax`);
//! * `HorizontalScroller2D` bodies ([`Scroller`]: Japan's clouds drift and wrap around);
//! * breakable props ([`LevelProp`]: an object with `Health`, `ExplodeOnHealthExhausted` and an
//!   `Explodable`, such as Japan's hanging lamps).

use crate::level::Collider;
use crate::math::Affine;
use crate::projectile::DifficultyScaling;
use crate::scene::{FileId, GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::sprites::{Parallax, SceneSprite};
use crate::yaml::Value;

/// Unity's built-in meshes (`{fileID: n, guid: 0000000000000000e000000000000000}`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BuiltinMesh {
    /// 10 × 10 in local x/z, 11 × 11 vertices, normal +y.
    Plane,
    /// 1 × 1 in local x/y, normal -z.
    Quad,
}

/// GUID of Unity's built-in extra resources (the primitive meshes).
const BUILTIN_EXTRA: &str = "0000000000000000e000000000000000";

impl BuiltinMesh {
    pub fn of_file_id(id: i64) -> Option<BuiltinMesh> {
        match id {
            10209 => Some(BuiltinMesh::Plane),
            10210 => Some(BuiltinMesh::Quad),
            _ => None,
        }
    }
}

/// An enabled `MeshRenderer` drawing a built-in mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneMesh {
    pub object: FileId,
    pub name: String,
    pub mesh: BuiltinMesh,
    /// GUID of the first material.
    pub material: String,
    pub world: Affine,
    /// A `FakeParallax` on the object or an ancestor.
    pub parallax: Option<Parallax>,
    /// `m_SortingLayerID`, `m_SortingOrder`.
    pub sorting: (i64, i64),
}

/// `HorizontalScroller2D` on a kinematic `Rigidbody2D`: `OnEnable` gives the body `velocity`
/// along x; every `FixedUpdate` wraps it by `right - left` once it leaves `[left, right]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Scroller {
    pub object: FileId,
    pub name: String,
    pub velocity: f32,
    pub left: f32,
    pub right: f32,
    /// World x of the body as saved.
    pub x: f32,
}

/// `Wave` (Pirates' water): its sprite slides along `velocity.x` and bobs, wrapping every sprite
/// size.
#[derive(Clone, Debug, PartialEq)]
pub struct WaveSpec {
    /// The object with the `Wave` and its `tk2dSprite`.
    pub object: FileId,
    pub velocity: [f32; 2],
    pub oscillation_amplitude: f32,
    /// The parent's world scale (the local movement in world units).
    pub parent_scale: [f32; 2],
}

/// Every active, enabled `Wave`.
pub fn waves(scene: &Scene, scripts: &ScriptIndex) -> Vec<WaveSpec> {
    scene
        .game_objects()
        .into_iter()
        .filter(|go| go.active_in_hierarchy())
        .filter_map(|go| {
            let b = &mono(&go, scripts, "Wave")?.body;
            if !b.bool_of("m_Enabled").unwrap_or(true) {
                return None;
            }
            let scale = go.parent().map_or([1.0; 3], |p| p.world_affine().lossy_scale());
            Some(WaveSpec {
                object: go.id(),
                velocity: b.get("velocity").and_then(Value::as_vec2).map_or([-1.0, 0.16], |(x, y)| [x, y]),
                oscillation_amplitude: b.f32_of("oscillationAmplitude").unwrap_or(0.7),
                parent_scale: [scale[0], scale[1]],
            })
        })
        .collect()
}

/// `Whale` (Pirates): a kinematic body that waits `start_wait_time`, then forever waits
/// `down_time`, rises `up_y_offset` in `transition_time` (`Mathf.SmoothStep`), waits `up_time`
/// and sinks back. Its polygon collider is in [`crate::level::Level::colliders`] (object
/// [`WhaleSpec::object`], a dynamic collider), its sprites follow it.
#[derive(Clone, Debug, PartialEq)]
pub struct WhaleSpec {
    pub object: FileId,
    pub up_time: f32,
    pub down_time: f32,
    pub transition_time: f32,
    pub up_y_offset: f32,
    pub start_wait_time: f32,
}

/// Every active, enabled `Whale`.
pub fn whales(scene: &Scene, scripts: &ScriptIndex) -> Vec<WhaleSpec> {
    scene
        .game_objects()
        .into_iter()
        .filter(|go| go.active_in_hierarchy())
        .filter_map(|go| {
            let b = &mono(&go, scripts, "Whale")?.body;
            if !b.bool_of("m_Enabled").unwrap_or(true) {
                return None;
            }
            Some(WhaleSpec {
                object: go.id(),
                up_time: b.f32_of("upTime").unwrap_or(4.0),
                down_time: b.f32_of("downTime").unwrap_or(4.0),
                transition_time: b.f32_of("transitionTime").unwrap_or(1.0),
                up_y_offset: b.f32_of("upYOffset").unwrap_or(14.0),
                start_wait_time: b.f32_of("startWaitTime").unwrap_or(0.0),
            })
        })
        .collect()
}

/// `StarShower` (Moon): stars fall on a random `Targetable` whose x is in `[start_x, end_x]`,
/// at a rate per second rising from `start_rate` to `peak_rate` over `[start_x, peak_start_x]`,
/// holding to `peak_end_x` and falling to `end_rate` at `end_x`; ×`vehicle_rate_multiplier` (rate
/// and spread) on a car; × the number of targets. Each star (`prefab`) starts `launch_offset_y`
/// above the target, ahead by where `launch_velocity` and gravity bring it down to the target's
/// height, spread ±`random_range / 2` in x, spinning between the two angular velocities.
#[derive(Clone, Debug, PartialEq)]
pub struct StarShowerSpec {
    pub prefab: String,
    pub start_x: f32,
    pub peak_start_x: f32,
    pub peak_end_x: f32,
    pub end_x: f32,
    pub start_rate: f32,
    pub peak_rate: f32,
    pub end_rate: f32,
    pub vehicle_rate_multiplier: f32,
    pub launch_angular_velocity_min: f32,
    pub launch_angular_velocity_max: f32,
    pub launch_velocity: [f32; 2],
    pub launch_offset_y: f32,
    pub random_range: f32,
}

/// Every active, enabled `StarShower`.
pub fn star_showers(scene: &Scene, scripts: &ScriptIndex) -> Vec<StarShowerSpec> {
    scene
        .game_objects()
        .into_iter()
        .filter(|go| go.active_in_hierarchy())
        .filter_map(|go| {
            let b = &mono(&go, scripts, "StarShower")?.body;
            if !b.bool_of("m_Enabled").unwrap_or(true) {
                return None;
            }
            let f = |k: &str, d: f32| b.f32_of(k).unwrap_or(d);
            Some(StarShowerSpec {
                prefab: b.get("prefab").and_then(Value::guid)?.to_owned(),
                start_x: f("startX", 0.0),
                peak_start_x: f("peakStartX", 10.0),
                peak_end_x: f("peakEndX", 20.0),
                end_x: f("endX", 30.0),
                start_rate: f("startRate", 0.5),
                peak_rate: f("peakRate", 8.0),
                end_rate: f("endRate", 0.5),
                vehicle_rate_multiplier: f("vehicleRateMultiplier", 1.5),
                launch_angular_velocity_min: f("launchAngularVelocityMin", -229.18312),
                launch_angular_velocity_max: f("launchAngularVelocityMax", -57.29578),
                launch_velocity: b.get("launchVelocity").and_then(Value::as_vec2).map_or([-2.0, -1.0], |(x, y)| [x, y]),
                launch_offset_y: f("launchOffsetY", 20.0),
                random_range: f("randomRange", 60.0),
            })
        })
        .collect()
}

/// A breakable level object: `Health` → `ExplodeOnHealthExhausted` → `Explodable.Explode` →
/// `ExplodeSpritesAndLines` (its sprites fly apart), `PlaySoundOnExplode`, and
/// `DestroyOnExplosionFinished`.
#[derive(Clone, Debug, PartialEq)]
pub struct LevelProp {
    pub object: FileId,
    pub name: String,
    pub position: [f32; 3],
    /// `Health.startingHealth` (or `SetStartingHealth`).
    pub starting_health: f32,
    /// `HealthDifficultyModifier`.
    pub health_scaling: Option<DifficultyScaling>,
    /// Indices into `Level::colliders` of the colliders in the object's tree.
    pub colliders: Vec<usize>,
    /// Indices into `Level::sprites` of the sprites (and their shadow copies) in its tree.
    pub sprites: Vec<usize>,
    /// `PlaySoundOnExplode.sound`.
    pub sound: Option<String>,
    /// `ExplodeSpritesAndLines.fadeWaitTime` (`None`: no `ExplodeSpritesAndLines`; the object
    /// is only switched off).
    pub fade_wait_time: Option<f32>,
    pub destroy_on_finished: bool,
    /// `DestroyAllForGoal`: destroying every such prop alone gives "<level> all destroyed".
    pub destroy_all_for_goal: bool,
    /// `DamagingDoesNotAwardScore`: hits on it credit nobody, and it is left out of
    /// `MaximumDamageCalculator`.
    pub no_score: bool,
    /// `SetLayerInCampaignMode.layer`.
    pub campaign_layer: Option<u8>,
}

fn script_name<'s>(doc: &crate::yaml::Document, scripts: &'s ScriptIndex) -> Option<&'s str> {
    doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g))
}

fn mono<'a>(go: &GameObject<'a>, scripts: &ScriptIndex, class_name: &str) -> Option<&'a crate::yaml::Document> {
    go.components().into_iter().filter(|c| c.class_id == class::MONO_BEHAVIOUR).find(|c| script_name(c, scripts) == Some(class_name))
}

fn subtree_ids(go: &GameObject<'_>, out: &mut Vec<FileId>) {
    out.push(go.id());
    for child in go.children() {
        subtree_ids(&child, out);
    }
}

fn has_character_root(go: &GameObject<'_>, scripts: &ScriptIndex) -> bool {
    let mut root = *go;
    while let Some(p) = root.parent() {
        root = p;
    }
    mono(&root, scripts, "CharacterControl2D").is_some()
}

/// Every active, enabled `MeshRenderer` with a built-in mesh (sprites and text meshes have their
/// own generated meshes and are not included).
pub fn scene_meshes(scene: &Scene, scripts: &ScriptIndex) -> Vec<SceneMesh> {
    let mut out = Vec::new();
    for go in scene.game_objects() {
        if !go.active_in_hierarchy() {
            continue;
        }
        let Some(renderer) = go.component(class::MESH_RENDERER) else { continue };
        if !renderer.body.bool_of("m_Enabled").unwrap_or(true) {
            continue;
        }
        let Some(filter) = go.component(33) else { continue };
        let Some(mesh_ref) = filter.body.get("m_Mesh") else { continue };
        if mesh_ref.guid() != Some(BUILTIN_EXTRA) {
            continue;
        }
        let Some(mesh) = mesh_ref.file_id().and_then(BuiltinMesh::of_file_id) else { continue };
        let Some(material) = renderer.body.get("m_Materials").and_then(|m| m.as_seq().first()).and_then(Value::guid) else { continue };
        let class_of = |c: &crate::yaml::Document| script_name(c, scripts);
        out.push(SceneMesh {
            object: go.id(),
            name: go.name().to_owned(),
            mesh,
            material: material.to_owned(),
            world: go.world_affine(),
            parallax: crate::sprites::parallax_of(go, &class_of),
            sorting: (renderer.body.i64_of("m_SortingLayerID").unwrap_or(0), renderer.body.i64_of("m_SortingOrder").unwrap_or(0)),
        });
    }
    out
}

/// Every active, enabled `HorizontalScroller2D`.
pub fn scrollers(scene: &Scene, scripts: &ScriptIndex) -> Vec<Scroller> {
    scene
        .game_objects()
        .into_iter()
        .filter(|go| go.active_in_hierarchy())
        .filter_map(|go| {
            let b = &mono(&go, scripts, "HorizontalScroller2D")?.body;
            if !b.bool_of("m_Enabled").unwrap_or(true) {
                return None;
            }
            Some(Scroller {
                object: go.id(),
                name: go.name().to_owned(),
                velocity: b.f32_of("velocity").unwrap_or(0.0),
                left: b.f32_of("left").unwrap_or(0.0),
                right: b.f32_of("right").unwrap_or(0.0),
                x: go.world_affine().t[0],
            })
        })
        .collect()
}

/// Every active breakable object outside the characters.
pub fn props(scene: &Scene, scripts: &ScriptIndex, colliders: &[Collider], sprites: &[SceneSprite]) -> Vec<LevelProp> {
    let mut out = Vec::new();
    for go in scene.game_objects() {
        if !go.active_in_hierarchy() || has_character_root(&go, scripts) {
            continue;
        }
        let (Some(health), Some(_), Some(_)) = (mono(&go, scripts, "Health"), mono(&go, scripts, "ExplodeOnHealthExhausted"), mono(&go, scripts, "Explodable")) else { continue };
        let mut starting_health = health.body.f32_of("startingHealth").unwrap_or(100.0);
        if let Some(s) = mono(&go, scripts, "SetStartingHealth") {
            starting_health = s.body.f32_of("startingHealth").unwrap_or(starting_health);
        }
        let mut ids = Vec::new();
        subtree_ids(&go, &mut ids);
        out.push(LevelProp {
            object: go.id(),
            name: go.name().to_owned(),
            position: go.world_affine().t,
            starting_health,
            health_scaling: mono(&go, scripts, "HealthDifficultyModifier").map(|m| crate::level::scaling_of(&m.body)),
            colliders: colliders.iter().enumerate().filter(|(_, c)| ids.contains(&c.object)).map(|(i, _)| i).collect(),
            sprites: sprites.iter().enumerate().filter(|(_, s)| ids.contains(&s.object)).map(|(i, _)| i).collect(),
            sound: mono(&go, scripts, "PlaySoundOnExplode").and_then(|m| m.body.str_of("sound")).map(str::to_owned),
            fade_wait_time: mono(&go, scripts, "ExplodeSpritesAndLines").map(|m| m.body.f32_of("fadeWaitTime").unwrap_or(2.0)),
            destroy_on_finished: mono(&go, scripts, "DestroyOnExplosionFinished").is_some(),
            destroy_all_for_goal: mono(&go, scripts, "DestroyAllForGoal").is_some(),
            no_score: mono(&go, scripts, "DamagingDoesNotAwardScore").is_some(),
            campaign_layer: mono(&go, scripts, "SetLayerInCampaignMode").and_then(|m| m.body.i64_of("layer")).map(|l| l as u8),
        });
    }
    out
}
