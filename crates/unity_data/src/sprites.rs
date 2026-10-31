//! 2D Toolkit sprite collections (`tk2dSpriteCollectionData`): named sprites with their quad
//! geometry and atlas UVs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::assets::GuidIndex;
use crate::material::MaterialLibrary;
use crate::scripts::ScriptIndex;
use crate::scene::{Scene, class};
use crate::yaml::Value;

/// One `tk2dSpriteDefinition`.
#[derive(Clone, Debug, PartialEq)]
pub struct SpriteDef {
    pub name: String,
    /// Vertex positions in sprite units, z = 0.
    pub positions: Vec<[f32; 3]>,
    /// Per-vertex normals: `(0, 0, -1)` on the front quad; double-sided definitions add a back
    /// quad with `(0, 0, 1)`.
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub material_guid: String,
    /// `texelSize` (world size of one atlas texel at scale 1).
    pub texel_size: [f32; 2],
    /// `untrimmedBoundsData`: centre and size.
    pub untrimmed_bounds: [[f32; 3]; 2],
    /// `boundsData` (the trimmed geometry): centre and size.
    pub bounds: [[f32; 3]; 2],
    /// `flipped` (`FlipMode`: 0 None, 1 Tk2d, 2 TPackerCW).
    pub flipped: i64,
}

#[derive(Clone, Debug, Default)]
pub struct SpriteCollection {
    pub name: String,
    pub sprites: Vec<SpriteDef>,
    /// `materials[i]` → `textures[i]` (GUIDs): the atlas each material instance draws with.
    pub material_textures: Vec<(String, String)>,
    /// `pngTextures` (TextAsset GUIDs holding PNG files) and `materialPngTextureId`: collections
    /// built to load their atlases from PNG bytes at runtime.
    pub png_textures: Vec<String>,
    pub material_png_ids: Vec<u32>,
    /// `materials` (GUIDs), in order.
    pub materials: Vec<String>,
    /// `Utility.GetPixelsPerMeter`: `halfTargetHeight * invOrthoSize`.
    pub pixels_per_meter: f32,
}

impl SpriteCollection {
    /// `GetSpriteIdByName`.
    pub fn id_by_name(&self, name: &str) -> Option<usize> {
        self.sprites.iter().position(|s| s.name == name)
    }

    pub fn sprite(&self, name: &str) -> Option<&SpriteDef> {
        self.sprites.iter().find(|s| s.name == name)
    }
}

fn hex_u32s(text: &str) -> Vec<u32> {
    let text = text.trim();
    (0..text.len() / 8)
        .filter_map(|i| {
            let word = &text[i * 8..i * 8 + 8];
            let b = (0..4).map(|k| u8::from_str_radix(&word[k * 2..k * 2 + 2], 16)).collect::<Result<Vec<u8>, _>>().ok()?;
            Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        })
        .collect()
}

/// Parse the collection stored in a prefab, if it holds one.
pub fn parse_collection(scene: &Scene, scripts: &ScriptIndex) -> Option<SpriteCollection> {
    for doc in scene.documents() {
        if doc.class_id != class::MONO_BEHAVIOUR {
            continue;
        }
        let Some(guid) = doc.body.get("m_Script").and_then(Value::guid) else { continue };
        if scripts.class_name(guid) != Some("tk2dSpriteCollectionData") {
            continue;
        }
        let mut sprites = Vec::new();
        for def in doc.body.get("spriteDefinitions")?.as_seq() {
            let positions = def.get("positions")?.as_seq().iter().filter_map(Value::as_vec3).map(|(x, y, z)| [x, y, z]).collect();
            let uvs = def.get("uvs")?.as_seq().iter().filter_map(Value::as_vec2).map(|(x, y)| [x, y]).collect();
            let normals = def.get("normals").map(Value::as_seq).unwrap_or(&[]).iter().filter_map(Value::as_vec3).map(|(x, y, z)| [x, y, z]).collect();
            let trimmed: Vec<[f32; 3]> = def.get("boundsData").map(Value::as_seq).unwrap_or(&[]).iter().filter_map(Value::as_vec3).map(|(x, y, z)| [x, y, z]).collect();
            let untrimmed: Vec<[f32; 3]> = def.get("untrimmedBoundsData").map(Value::as_seq).unwrap_or(&[]).iter().filter_map(Value::as_vec3).map(|(x, y, z)| [x, y, z]).collect();
            sprites.push(SpriteDef {
                name: def.str_of("name")?.to_owned(),
                positions,
                normals,
                uvs,
                indices: hex_u32s(def.str_of("indices").unwrap_or("")),
                material_guid: def.get("material")?.guid().unwrap_or("").to_owned(),
                texel_size: def.get("texelSize").and_then(Value::as_vec2).map_or([0.0, 0.0], |(x, y)| [x, y]),
                untrimmed_bounds: [untrimmed.first().copied().unwrap_or([0.0; 3]), untrimmed.get(1).copied().unwrap_or([0.0; 3])],
                bounds: [trimmed.first().copied().unwrap_or([0.0; 3]), trimmed.get(1).copied().unwrap_or([0.0; 3])],
                flipped: def.i64_of("flipped").unwrap_or(0),
            });
        }
        let guids = |key: &str| -> Vec<String> {
            doc.body.get(key).map(Value::as_seq).unwrap_or(&[]).iter().map(|v| v.guid().unwrap_or("").to_owned()).collect()
        };
        let materials = guids("materials");
        let material_textures = materials.iter().cloned().zip(guids("textures")).collect();
        return Some(SpriteCollection {
            name: doc.body.str_of("spriteCollectionName").unwrap_or("").to_owned(),
            sprites,
            material_textures,
            png_textures: guids("pngTextures"),
            material_png_ids: hex_u32s(doc.body.str_of("materialPngTextureId").unwrap_or("")),
            materials,
            pixels_per_meter: doc.body.f32_of("halfTargetHeight").unwrap_or(0.0) * doc.body.f32_of("invOrthoSize").unwrap_or(0.0),
        });
    }
    None
}

/// The atlas a collection material draws with: the collection's own `textures[i]` for
/// `materials[i]` (what tk2d puts into the material instance), else the `.mat` file's `_MainTex`.
///
/// Collections with `pngTextures` give the `.png.bytes` TextAsset instead (the PNG file the
/// collection decodes with `Texture2D.LoadImage`).
pub fn collection_texture(collection: &SpriteCollection, materials: &MaterialLibrary, material_guid: &str) -> Option<PathBuf> {
    if !collection.png_textures.is_empty() {
        let index = collection.materials.iter().position(|m| m == material_guid)?;
        let png = collection.material_png_ids.get(index).copied().unwrap_or(0) as usize;
        if let Some(path) = collection.png_textures.get(png).and_then(|g| materials.text_asset(g)) {
            return Some(path);
        }
    }
    collection
        .material_textures
        .iter()
        .find(|(m, _)| m == material_guid)
        .and_then(|(_, t)| materials.texture(t))
        .or_else(|| materials.load(material_guid).and_then(|m| m.main_texture))
}

/// Collection GUIDs referenced by `tk2dSprite` components of a scene text
/// (`collection: {fileID: .., guid: <guid>, type: 2}`), in first-use order.
pub fn collection_guids_in(scene_text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for line in scene_text.lines() {
        let Some(rest) = line.trim_start().strip_prefix("collection: {") else { continue };
        let Some(start) = rest.find("guid: ") else { continue };
        let guid: String = rest[start + 6..].chars().take_while(|c| c.is_ascii_hexdigit()).collect();
        if !guid.is_empty() && !out.contains(&guid) {
            out.push(guid);
        }
    }
    out
}

/// A sprite collection together with the atlas texture its `Line`, `Helmet`, ... sprites use.
pub struct SpriteLibrary {
    pub collection: SpriteCollection,
    /// Atlas file per material GUID.
    textures: HashMap<String, PathBuf>,
}

impl SpriteLibrary {
    /// Atlas image of a sprite (`_MainTex` of its material).
    pub fn texture_of(&self, sprite: &SpriteDef) -> Option<&Path> {
        self.textures.get(&sprite.material_guid).map(PathBuf::as_path)
    }

    /// The collection stored in the prefab with GUID `guid` (`tk2dSprite.collection`).
    pub fn load(assets_dir: &Path, guid: &str) -> std::io::Result<SpriteLibrary> {
        let prefabs = GuidIndex::scan(&assets_dir.join("GameObject"), "prefab");
        let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
        let materials = MaterialLibrary::scan(assets_dir);
        let path = prefabs.path(guid).ok_or_else(|| std::io::Error::other(format!("no sprite collection prefab {guid}")))?;
        let collection = parse_collection(&Scene::load(path)?, &scripts).ok_or_else(|| std::io::Error::other(format!("{}: not a sprite collection", path.display())))?;
        Ok(SpriteLibrary::with_textures(collection, &materials))
    }

    fn with_textures(collection: SpriteCollection, materials: &MaterialLibrary) -> SpriteLibrary {
        let mut textures = HashMap::new();
        for sprite in &collection.sprites {
            if textures.contains_key(&sprite.material_guid) {
                continue;
            }
            if let Some(path) = collection_texture(&collection, materials, &sprite.material_guid) {
                textures.insert(sprite.material_guid.clone(), path);
            }
        }
        SpriteLibrary { collection, textures }
    }
}

/// A `tk2dSprite` placed in a scene, with everything needed to build its mesh.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSprite {
    pub object: crate::scene::FileId,
    /// Name of the object (`"Sprite Shadow"` for shadow copies).
    pub name: String,
    /// GUID of the collection prefab.
    pub collection: String,
    pub sprite_id: usize,
    /// `tk2dSprite._color`.
    pub color: [f32; 4],
    /// `tk2dSprite._scale`.
    pub scale: [f32; 3],
    /// `Transform.localToWorldMatrix` of the object.
    pub world: crate::math::Affine,
    /// World rotation `(x, y, z, w)` of the object (`Transform.rotation`).
    pub world_rotation: [f32; 4],
    /// For the `"Sprite Shadow"` child that `SpriteShadower` uses: the index (in the returned
    /// list) of the sprite casting it and `SpriteShadowPositioner.shadowPlaneZ`. Its transform
    /// and scale are recomputed from the scene light every frame.
    pub shadow_of: Option<(usize, f32)>,
    /// `tk2dTiledSprite`: the sprite repeated over `dimensions` (in texels).
    pub tiled: Option<TiledLayout>,
    /// A `FakeParallax` on the object or an ancestor, which moves it with the camera.
    pub parallax: Option<Parallax>,
    /// `ScaleSpriteWithCameraDistance.factor`.
    pub scale_with_camera: Option<f32>,
    /// `tk2dClippedSprite`: `_clipBottomLeft` and `_clipTopRight` (fractions of the untrimmed
    /// bounds).
    pub clipped: Option<[[f32; 2]; 2]>,
    /// The nearest object (itself or an ancestor) with a `Rigidbody2D`: the sprite moves with
    /// that body.
    pub rigid_root: Option<crate::scene::FileId>,
}

/// `tk2dTiledSprite._dimensions` / `_anchor`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TiledLayout {
    pub dimensions: [f32; 2],
    pub anchor: i64,
}

/// A `FakeParallax` component, on `object`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parallax {
    pub object: crate::scene::FileId,
    pub real_position: [f32; 2],
    pub fake_z: f32,
    pub x_only: bool,
    /// The parallax object's serialized local position and its parent's world transform: its
    /// `Update` rewrites the local position, which moves the sprite by the parent's image of the
    /// change.
    pub local_position: [f32; 3],
    pub parent_world: crate::math::Affine,
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

pub(crate) fn world_rotation_of(go: crate::scene::GameObject<'_>) -> [f32; 4] {
    let local = go.transform().and_then(|t| t.body.get("m_LocalRotation")).and_then(Value::as_quat).map_or([0.0, 0.0, 0.0, 1.0], |(x, y, z, w)| [x, y, z, w]);
    match go.parent() {
        Some(parent) => quat_mul(world_rotation_of(parent), local),
        None => local,
    }
}

/// The nearest enabled `FakeParallax` on the object or an ancestor.
pub(crate) fn parallax_of<'s>(go: crate::scene::GameObject<'_>, class_of: &dyn Fn(&crate::yaml::Document) -> Option<&'s str>) -> Option<Parallax> {
    let mut current = Some(go);
    while let Some(o) = current {
        if let Some(p) = o.components().iter().find(|c| class_of(c) == Some("FakeParallax") && c.body.bool_of("m_Enabled").unwrap_or(true)) {
            let b = &p.body;
            let local = o.transform().and_then(|t| t.body.get("m_LocalPosition")).and_then(Value::as_vec3).map_or([0.0; 3], |(x, y, z)| [x, y, z]);
            return Some(Parallax {
                object: o.id(),
                real_position: b.get("realPosition").and_then(Value::as_vec2).map_or([0.0, 0.0], |(x, y)| [x, y]),
                fake_z: b.f32_of("fakeZ").unwrap_or(0.0),
                x_only: b.bool_of("xOnly").unwrap_or(true),
                local_position: local,
                parent_world: o.parent().map_or(crate::math::Affine::IDENTITY, |p| p.world_affine()),
            });
        }
        current = o.parent();
    }
    None
}

fn root_of(go: crate::scene::GameObject<'_>) -> crate::scene::GameObject<'_> {
    let mut root = go;
    while let Some(parent) = root.parent() {
        root = parent;
    }
    root
}

/// Every `tk2dSprite` the scene draws as part of the level: active, enabled, with an enabled
/// `MeshRenderer`. Sprites inside characters (`CharacterControl2D` roots: players' and enemies'
/// helmets, weapons and their shadows) are left out: the figure renderer poses and draws those.
///
/// Every `SpriteShadower` without a serialized `"Sprite Shadow"` child gets one (as `OnEnable`
/// creates it), with `shadowPlaneZ` 0.
pub fn scene_sprites(scene: &Scene, scripts: &ScriptIndex) -> Vec<SceneSprite> {
    scene_sprites_with(scene, scripts, false)
}

/// [`scene_sprites`], optionally also under inactive objects (`object` says where each one sits,
/// so the caller can follow activity).
pub fn scene_sprites_with(scene: &Scene, scripts: &ScriptIndex, include_inactive: bool) -> Vec<SceneSprite> {
    let class_of = |doc: &crate::yaml::Document| doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g));
    let mut sprites: Vec<SceneSprite> = Vec::new();
    let mut index_of_object: HashMap<crate::scene::FileId, usize> = HashMap::new();
    let mut shadow_children: Vec<(crate::scene::FileId, crate::scene::FileId, f32)> = Vec::new();
    let mut shadowers: Vec<crate::scene::FileId> = Vec::new();
    for go in scene.game_objects() {
        if !include_inactive && !go.active_in_hierarchy() {
            continue;
        }
        let components = go.components();
        let renderer_on = components.iter().find(|c| c.class_id == class::MESH_RENDERER).is_some_and(|r| r.body.bool_of("m_Enabled").unwrap_or(true));
        let Some(sprite) = components.iter().find(|c| matches!(class_of(c), Some("tk2dSprite" | "tk2dTiledSprite" | "tk2dClippedSprite"))) else { continue };
        if !renderer_on || !sprite.body.bool_of("m_Enabled").unwrap_or(true) {
            continue;
        }
        if root_of(go).components().iter().any(|c| class_of(c) == Some("CharacterControl2D")) {
            continue;
        }
        let b = &sprite.body;
        let Some(collection) = b.get("collection").and_then(Value::guid) else { continue };
        let c = b.get("_color");
        let index = sprites.len();
        sprites.push(SceneSprite {
            object: go.id(),
            name: go.name().to_owned(),
            collection: collection.to_owned(),
            sprite_id: b.i64_of("_spriteId").unwrap_or(0).max(0) as usize,
            color: ["r", "g", "b", "a"].map(|k| c.and_then(|c| c.f32_of(k)).unwrap_or(1.0)),
            scale: b.get("_scale").and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
            world: go.world_affine(),
            world_rotation: world_rotation_of(go),
            shadow_of: None,
            tiled: (class_of(sprite) == Some("tk2dTiledSprite")).then(|| TiledLayout {
                dimensions: b.get("_dimensions").and_then(Value::as_vec2).map_or([50.0, 50.0], |(x, y)| [x, y]),
                anchor: b.i64_of("_anchor").unwrap_or(0),
            }),
            parallax: parallax_of(go, &class_of),
            scale_with_camera: components
                .iter()
                .find(|c| class_of(c) == Some("ScaleSpriteWithCameraDistance") && c.body.bool_of("m_Enabled").unwrap_or(true))
                .map(|c| c.body.f32_of("factor").unwrap_or(1.0)),
            rigid_root: {
                let mut current = Some(go);
                let mut found = None;
                while let Some(o) = current {
                    if o.component(class::RIGIDBODY_2D).is_some() {
                        found = Some(o.id());
                        break;
                    }
                    current = o.parent();
                }
                found
            },
            clipped: (class_of(sprite) == Some("tk2dClippedSprite")).then(|| {
                let v2 = |k: &str, d: [f32; 2]| b.get(k).and_then(Value::as_vec2).map_or(d, |(x, y)| [x, y]);
                [v2("_clipBottomLeft", [0.0, 0.0]), v2("_clipTopRight", [1.0, 1.0])]
            }),
        });
        index_of_object.insert(go.id(), index);
        if let Some(positioner) = components.iter().find(|c| class_of(c) == Some("SpriteShadowPositioner")) {
            if let Some(parent) = go.parent() {
                shadow_children.push((go.id(), parent.id(), positioner.body.f32_of("shadowPlaneZ").unwrap_or(0.0)));
            }
        }
        if components.iter().any(|c| class_of(c) == Some("SpriteShadower")) && !go.children().iter().any(|c| c.name() == "Sprite Shadow") {
            shadowers.push(go.id());
        }
    }
    for (shadow, caster, plane) in shadow_children {
        if let (Some(&s), Some(&c)) = (index_of_object.get(&shadow), index_of_object.get(&caster)) {
            sprites[s].shadow_of = Some((c, plane));
        }
    }
    for caster in shadowers {
        let c = index_of_object[&caster];
        let mut shadow = sprites[c].clone();
        shadow.name = "Sprite Shadow".to_owned();
        shadow.shadow_of = Some((c, 0.0));
        sprites.push(shadow);
    }
    sprites
}

/// A `tk2dSlicedSprite` (nine-sliced box: the menus' panels and button backgrounds).
#[derive(Clone, Debug, PartialEq)]
pub struct SceneSlicedSprite {
    pub object: crate::scene::FileId,
    pub collection: String,
    pub sprite_id: usize,
    /// `_color`.
    pub color: [f32; 4],
    /// `_scale`.
    pub scale: [f32; 3],
    /// `_dimensions` in texels.
    pub dimensions: [f32; 2],
    /// `_anchor` (`tk2dBaseSprite.Anchor`: 0 lower-left ... 8 upper-right).
    pub anchor: i64,
    /// `borderLeft`, `borderBottom`, `borderRight`, `borderTop` (fractions of the sprite).
    pub borders: [f32; 4],
    /// `_borderOnly`: the centre cell is left out.
    pub border_only: bool,
}

/// Every enabled `tk2dSlicedSprite` with an enabled `MeshRenderer`, also under inactive objects
/// (the menus are switched on and off).
pub fn scene_sliced_sprites(scene: &Scene, scripts: &ScriptIndex) -> Vec<SceneSlicedSprite> {
    let class_of = |doc: &crate::yaml::Document| doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g));
    let mut out = Vec::new();
    for go in scene.game_objects() {
        let components = go.components();
        let renderer_on = components.iter().find(|c| c.class_id == class::MESH_RENDERER).is_some_and(|r| r.body.bool_of("m_Enabled").unwrap_or(true));
        let Some(sprite) = components.iter().find(|c| class_of(c) == Some("tk2dSlicedSprite")) else { continue };
        if !renderer_on || !sprite.body.bool_of("m_Enabled").unwrap_or(true) {
            continue;
        }
        let b = &sprite.body;
        let Some(collection) = b.get("collection").and_then(Value::guid) else { continue };
        let c = b.get("_color");
        out.push(SceneSlicedSprite {
            object: go.id(),
            collection: collection.to_owned(),
            sprite_id: b.i64_of("_spriteId").unwrap_or(0).max(0) as usize,
            color: ["r", "g", "b", "a"].map(|k| c.and_then(|c| c.f32_of(k)).unwrap_or(1.0)),
            scale: b.get("_scale").and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
            dimensions: b.get("_dimensions").and_then(Value::as_vec2).map_or([50.0, 50.0], |(x, y)| [x, y]),
            anchor: b.i64_of("_anchor").unwrap_or(0),
            borders: ["borderLeft", "borderBottom", "borderRight", "borderTop"].map(|k| b.f32_of(k).unwrap_or(0.2)),
            border_only: b.bool_of("_borderOnly").unwrap_or(false),
        });
    }
    out
}

/// Find, among the collections the scene's sprites use, the first that contains every
/// requested sprite — which is how `SpriteFinder` resolves names (first `tk2dSprite` whose
/// collection has the sprite).
pub fn load_collection_with_sprites(assets_dir: &Path, scene_path: &Path, names: &[&str]) -> std::io::Result<SpriteLibrary> {
    let scene_text = std::fs::read_to_string(scene_path)?;
    let prefabs = GuidIndex::scan(&assets_dir.join("GameObject"), "prefab");
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    let materials = MaterialLibrary::scan(assets_dir);
    for guid in collection_guids_in(&scene_text) {
        let Some(path) = prefabs.path(&guid) else { continue };
        let scene = Scene::load(path)?;
        let Some(collection) = parse_collection(&scene, &scripts) else { continue };
        if names.iter().all(|n| collection.id_by_name(n).is_some()) {
            return Ok(SpriteLibrary::with_textures(collection, &materials));
        }
    }
    Err(std::io::Error::other(format!("no sprite collection in the scene contains {names:?}")))
}
