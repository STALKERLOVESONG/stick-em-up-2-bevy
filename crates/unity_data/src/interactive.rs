//! Scenes whose parts are switched on and off while they run (the Lobby): the object tree with
//! every object's `activeSelf`, every script instance with its serialized fields (references
//! resolvable to objects), and the renderers under inactive objects too.

use std::collections::HashMap;
use std::path::Path;

use crate::fonts::{SceneText, scene_texts};
use crate::level::{Collider, Level, all_colliders, extract_level};
use crate::math::Affine;
use crate::scene::{FileId, Scene, class};
use crate::scripts::ScriptIndex;
use crate::sprites::{SceneSlicedSprite, SceneSprite, scene_sliced_sprites, scene_sprites_with};
use crate::yaml::Value;

#[derive(Clone, Debug)]
pub struct ObjectNode {
    pub id: FileId,
    pub name: String,
    pub parent: Option<usize>,
    /// In `m_Children` order.
    pub children: Vec<usize>,
    /// `GameObject.activeSelf` as serialized.
    pub active_self: bool,
    pub local_position: [f32; 3],
    /// `m_LocalRotation` (x, y, z, w) and `m_LocalScale`.
    pub local_rotation: [f32; 4],
    pub local_scale: [f32; 3],
    /// `Transform.localToWorldMatrix` as serialized.
    pub world: Affine,
}

/// One `MonoBehaviour`.
#[derive(Clone, Debug)]
pub struct ScriptInstance {
    pub object: usize,
    pub class: String,
    pub enabled: bool,
    pub fields: Value,
}

/// A 3D `BoxCollider` (what `OnMouse*` events and `tk2dUI` hit), in its object's local space.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxCollider {
    pub object: usize,
    pub center: [f32; 3],
    pub size: [f32; 3],
    pub enabled: bool,
}

pub struct InteractiveScene {
    pub objects: Vec<ObjectNode>,
    pub scripts: Vec<ScriptInstance>,
    pub sprites: Vec<SceneSprite>,
    /// `tk2dSlicedSprite`s under any object.
    pub sliced: Vec<SceneSlicedSprite>,
    pub texts: Vec<SceneText>,
    /// Colliders (with their `PolygonColliderRenderer` surfaces) under any object.
    pub colliders: Vec<Collider>,
    pub boxes: Vec<BoxCollider>,
    /// `MeshRenderer.m_SortingLayerID` and `m_SortingOrder` per object.
    pub sorting: HashMap<usize, (i64, i64)>,
    /// Camera, light and the rest of what the level reader extracts (active parts only).
    pub level: Level,
    by_id: HashMap<FileId, usize>,
    /// Component file id → index of its object.
    component_object: HashMap<FileId, usize>,
}

impl InteractiveScene {
    pub fn object_of(&self, id: FileId) -> Option<usize> {
        self.by_id.get(&id).copied()
    }

    /// The object a reference field points at (`{fileID: n}` to a GameObject or a component).
    pub fn reference(&self, value: Option<&Value>) -> Option<usize> {
        let id = value?.file_id()?;
        if id == 0 {
            return None;
        }
        self.by_id.get(&id).or_else(|| self.component_object.get(&id)).copied()
    }

    /// `script.fields[field]` as an object.
    pub fn field_object(&self, script: &ScriptInstance, field: &str) -> Option<usize> {
        self.reference(script.fields.get(field))
    }

    /// Objects of a reference list field.
    pub fn field_objects(&self, script: &ScriptInstance, field: &str) -> Vec<Option<usize>> {
        script.fields.get(field).map(Value::as_seq).unwrap_or(&[]).iter().map(|v| self.reference(Some(v))).collect()
    }

    pub fn scripts_of<'a>(&'a self, object: usize) -> impl Iterator<Item = &'a ScriptInstance> + 'a {
        self.scripts.iter().filter(move |s| s.object == object)
    }

    pub fn script(&self, object: usize, class_name: &str) -> Option<&ScriptInstance> {
        self.scripts.iter().find(|s| s.object == object && s.class == class_name)
    }

    pub fn scripts_named<'a>(&'a self, class_name: &'a str) -> impl Iterator<Item = &'a ScriptInstance> + 'a {
        self.scripts.iter().filter(move |s| s.class == class_name)
    }

    /// `Transform.Find`: a direct child by name.
    pub fn child(&self, object: usize, name: &str) -> Option<usize> {
        self.objects[object].children.iter().copied().find(|&c| self.objects[c].name == name)
    }

    /// A root object by name.
    pub fn root(&self, name: &str) -> Option<usize> {
        self.objects.iter().position(|o| o.parent.is_none() && o.name == name)
    }

    /// `activeInHierarchy` under the given `activeSelf` flags.
    pub fn active_in_hierarchy(&self, object: usize, active_self: &[bool]) -> bool {
        let mut current = Some(object);
        while let Some(i) = current {
            if !active_self[i] {
                return false;
            }
            current = self.objects[i].parent;
        }
        true
    }
}

/// Parse a scene (relative to the export root) as an [`InteractiveScene`].
pub fn load_interactive_scene(export_root: &Path, scene_path: &str) -> std::io::Result<InteractiveScene> {
    let scripts_index = ScriptIndex::scan(&export_root.join("Assets/Scripts"));
    let scene = Scene::load(&export_root.join(scene_path))?;
    let name = Path::new(scene_path).file_stem().and_then(|s| s.to_str()).unwrap_or("scene");
    let level = extract_level(name, &scene, &scripts_index);

    let gos = scene.game_objects();
    let by_id: HashMap<FileId, usize> = gos.iter().enumerate().map(|(i, g)| (g.id(), i)).collect();
    let mut objects: Vec<ObjectNode> = gos
        .iter()
        .map(|g| ObjectNode {
            id: g.id(),
            name: g.name().to_owned(),
            parent: g.parent().and_then(|p| by_id.get(&p.id()).copied()),
            children: g.children().iter().filter_map(|c| by_id.get(&c.id()).copied()).collect(),
            active_self: g.active_self(),
            local_position: g.transform().and_then(|t| t.body.get("m_LocalPosition")).and_then(Value::as_vec3).map_or([0.0; 3], |(x, y, z)| [x, y, z]),
            local_rotation: g.transform().and_then(|t| t.body.get("m_LocalRotation")).and_then(Value::as_quat).map_or([0.0, 0.0, 0.0, 1.0], |(x, y, z, w)| [x, y, z, w]),
            local_scale: g.transform().and_then(|t| t.body.get("m_LocalScale")).and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
            world: g.world_affine(),
        })
        .collect();
    objects.shrink_to_fit();

    let mut component_object = HashMap::new();
    let mut scripts = Vec::new();
    let mut boxes = Vec::new();
    let mut sorting = HashMap::new();
    for (index, g) in gos.iter().enumerate() {
        for c in g.components() {
            component_object.insert(c.file_id, index);
            match c.class_id {
                class::MONO_BEHAVIOUR => {
                    let Some(class_name) = c.body.get("m_Script").and_then(Value::guid).and_then(|guid| scripts_index.class_name(guid)) else { continue };
                    scripts.push(ScriptInstance {
                        object: index,
                        class: class_name.to_owned(),
                        enabled: c.body.bool_of("m_Enabled").unwrap_or(true),
                        fields: c.body.clone(),
                    });
                }
                class::MESH_RENDERER => {
                    sorting.insert(index, (c.body.i64_of("m_SortingLayerID").unwrap_or(0), c.body.i64_of("m_SortingOrder").unwrap_or(0)));
                }
                class::BOX_COLLIDER => {
                    let v3 = |k: &str, d: f32| c.body.get(k).and_then(Value::as_vec3).map_or([d; 3], |(x, y, z)| [x, y, z]);
                    boxes.push(BoxCollider { object: index, center: v3("m_Center", 0.0), size: v3("m_Size", 1.0), enabled: c.body.bool_of("m_Enabled").unwrap_or(true) });
                }
                _ => {}
            }
        }
    }
    Ok(InteractiveScene {
        sprites: scene_sprites_with(&scene, &scripts_index, true),
        sliced: scene_sliced_sprites(&scene, &scripts_index),
        texts: scene_texts(&scene, &scripts_index, true),
        colliders: all_colliders(&scene, &scripts_index),
        objects,
        scripts,
        boxes,
        sorting,
        level,
        by_id,
        component_object,
    })
}
