//! Scene/prefab documents: GameObjects, Transform hierarchies and their components.

use std::collections::HashMap;
use std::path::Path;

use crate::math::Affine;
use crate::yaml::{self, Document, Value};

pub type FileId = i64;

/// Unity class ids used by the loaders.
pub mod class {
    pub const GAME_OBJECT: u32 = 1;
    pub const TRANSFORM: u32 = 4;
    pub const CAMERA: u32 = 20;
    pub const LIGHT: u32 = 108;
    pub const RIGIDBODY_2D: u32 = 50;
    pub const CIRCLE_COLLIDER_2D: u32 = 58;
    pub const POLYGON_COLLIDER_2D: u32 = 60;
    pub const BOX_COLLIDER_2D: u32 = 61;
    pub const EDGE_COLLIDER_2D: u32 = 68;
    pub const MESH_RENDERER: u32 = 23;
    pub const BOX_COLLIDER: u32 = 65;
    pub const MONO_BEHAVIOUR: u32 = 114;
    pub const AUDIO_SOURCE: u32 = 82;
}

/// A parsed `.unity` or `.prefab` file.
pub struct Scene {
    docs: HashMap<FileId, Document>,
    /// GameObject file id → its Transform document id.
    transform_of: HashMap<FileId, FileId>,
}

impl Scene {
    pub fn load(path: &Path) -> std::io::Result<Scene> {
        let text = std::fs::read_to_string(path)?;
        Ok(Scene::parse(&text))
    }

    pub fn parse(text: &str) -> Scene {
        let docs = yaml::index_by_file_id(yaml::parse_documents(text));
        let mut transform_of = HashMap::new();
        for doc in docs.values() {
            if doc.class_id == class::TRANSFORM {
                if let Some(go) = doc.body.get("m_GameObject").and_then(Value::file_id) {
                    transform_of.insert(go, doc.file_id);
                }
            }
        }
        Scene { docs, transform_of }
    }

    pub fn get(&self, id: FileId) -> Option<&Document> {
        self.docs.get(&id)
    }

    pub fn documents(&self) -> impl Iterator<Item = &Document> {
        self.docs.values()
    }

    pub fn game_object(&self, id: FileId) -> Option<GameObject<'_>> {
        let doc = self.docs.get(&id)?;
        (doc.class_id == class::GAME_OBJECT).then_some(GameObject { scene: self, doc })
    }

    /// All GameObjects, sorted by file id for deterministic iteration.
    pub fn game_objects(&self) -> Vec<GameObject<'_>> {
        let mut out: Vec<_> = self
            .docs
            .values()
            .filter(|d| d.class_id == class::GAME_OBJECT)
            .map(|doc| GameObject { scene: self, doc })
            .collect();
        out.sort_by_key(|g| g.doc.file_id);
        out
    }

    fn transform_doc(&self, go: FileId) -> Option<&Document> {
        self.docs.get(self.transform_of.get(&go)?)
    }
}

#[derive(Clone, Copy)]
pub struct GameObject<'a> {
    scene: &'a Scene,
    pub doc: &'a Document,
}

impl<'a> GameObject<'a> {
    pub fn id(&self) -> FileId {
        self.doc.file_id
    }

    pub fn name(&self) -> &'a str {
        self.doc.body.str_of("m_Name").unwrap_or("")
    }

    pub fn layer(&self) -> u8 {
        self.doc.body.i64_of("m_Layer").unwrap_or(0) as u8
    }

    pub fn tag(&self) -> &'a str {
        self.doc.body.str_of("m_TagString").unwrap_or("Untagged")
    }

    /// `GameObject.activeSelf`.
    pub fn active_self(&self) -> bool {
        self.doc.body.bool_of("m_IsActive").unwrap_or(true)
    }

    /// `GameObject.activeInHierarchy`.
    pub fn active_in_hierarchy(&self) -> bool {
        self.active_self() && self.parent().is_none_or(|p| p.active_in_hierarchy())
    }

    pub fn components(&self) -> Vec<&'a Document> {
        self.doc
            .body
            .get("m_Component")
            .map(|list| {
                list.as_seq()
                    .iter()
                    .filter_map(|entry| {
                        let (_, reference) = entry.as_map().first()?;
                        self.scene.get(reference.file_id()?)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn component(&self, class_id: u32) -> Option<&'a Document> {
        self.components().into_iter().find(|c| c.class_id == class_id)
    }

    pub fn transform(&self) -> Option<&'a Document> {
        self.scene.transform_doc(self.id())
    }

    pub fn parent(&self) -> Option<GameObject<'a>> {
        let father = self.transform()?.body.get("m_Father")?.file_id()?;
        let parent_transform = self.scene.get(father)?;
        let go = parent_transform.body.get("m_GameObject")?.file_id()?;
        self.scene.game_object(go)
    }

    /// Children in `m_Children` order.
    pub fn children(&self) -> Vec<GameObject<'a>> {
        let Some(t) = self.transform() else { return Vec::new() };
        t.body
            .get("m_Children")
            .map(|c| {
                c.as_seq()
                    .iter()
                    .filter_map(|r| {
                        let child_t = self.scene.get(r.file_id()?)?;
                        self.scene.game_object(child_t.body.get("m_GameObject")?.file_id()?)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn has_rigidbody_2d_in_parents(&self) -> bool {
        self.component(class::RIGIDBODY_2D).is_some() || self.parent().is_some_and(|p| p.has_rigidbody_2d_in_parents())
    }

    /// Local transform (`T * R * S`).
    pub fn local_affine(&self) -> Affine {
        let Some(t) = self.transform() else { return Affine::IDENTITY };
        let pos = t.body.get("m_LocalPosition").and_then(Value::as_vec3).unwrap_or((0.0, 0.0, 0.0));
        let rot = t.body.get("m_LocalRotation").and_then(Value::as_quat).unwrap_or((0.0, 0.0, 0.0, 1.0));
        let scale = t.body.get("m_LocalScale").and_then(Value::as_vec3).unwrap_or((1.0, 1.0, 1.0));
        Affine::from_trs([pos.0, pos.1, pos.2], rot, [scale.0, scale.1, scale.2])
    }

    /// `Transform.localToWorldMatrix`.
    pub fn world_affine(&self) -> Affine {
        match self.parent() {
            Some(p) => p.world_affine().mul(&self.local_affine()),
            None => self.local_affine(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCENE: &str = "\
%YAML 1.1
--- !u!1 &1
GameObject:
  m_Component:
  - 4: {fileID: 2}
  m_Layer: 8
  m_Name: Parent
  m_TagString: Untagged
  m_IsActive: 1
--- !u!4 &2
Transform:
  m_GameObject: {fileID: 1}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 10, y: 0, z: 0}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children:
  - {fileID: 4}
  m_Father: {fileID: 0}
--- !u!1 &3
GameObject:
  m_Component:
  - 4: {fileID: 4}
  m_Layer: 0
  m_Name: Child
  m_TagString: Untagged
  m_IsActive: 0
--- !u!4 &4
Transform:
  m_GameObject: {fileID: 3}
  m_LocalRotation: {x: 0, y: 0, z: 0, w: 1}
  m_LocalPosition: {x: 1, y: 2, z: 3}
  m_LocalScale: {x: 1, y: 1, z: 1}
  m_Children: []
  m_Father: {fileID: 2}
";

    #[test]
    fn resolves_hierarchy_world_position_and_activity() {
        let scene = Scene::parse(SCENE);
        let child = scene.game_object(3).unwrap();
        assert_eq!(child.name(), "Child");
        assert_eq!(child.parent().unwrap().name(), "Parent");
        assert_eq!(child.world_affine().t, [11.0, 2.0, 3.0]);
        assert!(!child.active_in_hierarchy());
        let parent = scene.game_object(1).unwrap();
        assert!(parent.active_in_hierarchy());
        assert_eq!(parent.layer(), 8);
        assert_eq!(parent.children().len(), 1);
    }
}
