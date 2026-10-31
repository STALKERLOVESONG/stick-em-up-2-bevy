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
            campaign_layer: mono(&go, scripts, "SetLayerInCampaignMode").and_then(|m| m.body.i64_of("layer")).map(|l| l as u8),
        });
    }
    out
}
