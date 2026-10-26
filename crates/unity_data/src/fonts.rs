//! 2D Toolkit fonts (`tk2dFontData` prefabs) and the `tk2dTextMesh`es placed in scenes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::assets::GuidIndex;
use crate::material::MaterialLibrary;
use crate::math::Affine;
use crate::scene::{FileId, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// `tk2dFontChar`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FontChar {
    pub p0: [f32; 2],
    pub p1: [f32; 2],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub flipped: bool,
    pub advance: f32,
}

/// `tk2dFontData`.
#[derive(Clone, Debug, Default)]
pub struct Font {
    pub name: String,
    pub version: i64,
    pub line_height: f32,
    pub texel_size: [f32; 2],
    pub use_dictionary: bool,
    /// `charDict` (when `use_dictionary`), keyed by character code.
    pub dict: HashMap<u32, FontChar>,
    /// `chars` (when not).
    pub chars: Vec<FontChar>,
    /// `(c0, c1, amount)`.
    pub kerning: Vec<(u32, u32, f32)>,
    pub material_guid: String,
    /// GUID of `spriteCollection` (fonts built into a sprite collection share its atlas).
    pub sprite_collection: Option<String>,
    /// The atlas the font draws with.
    pub texture: Option<PathBuf>,
}

impl Font {
    /// The character drawn for `c` (unknown characters draw character 0, as tk2d does).
    pub fn char(&self, c: u32) -> FontChar {
        if self.use_dictionary {
            self.dict.get(&c).or_else(|| self.dict.get(&0)).copied().unwrap_or_default()
        } else {
            let i = if (c as usize) < self.chars.len() { c as usize } else { 0 };
            self.chars.get(i).copied().unwrap_or_default()
        }
    }

    pub fn kerning(&self, c0: u32, c1: u32) -> Option<f32> {
        self.kerning.iter().find(|k| k.0 == c0 && k.1 == c1).map(|k| k.2)
    }
}

fn hex_u32s(text: &str) -> Vec<u32> {
    let text = text.trim();
    (0..text.len() / 8)
        .filter_map(|i| {
            let word = text.get(i * 8..i * 8 + 8)?;
            let b = (0..4).map(|k| u8::from_str_radix(&word[k * 2..k * 2 + 2], 16)).collect::<Result<Vec<u8>, _>>().ok()?;
            Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        })
        .collect()
}

fn font_char(v: &Value) -> FontChar {
    let v2 = |k: &str| v.get(k).and_then(Value::as_vec3).map_or([0.0; 2], |(x, y, _)| [x, y]);
    FontChar {
        p0: v2("p0"),
        p1: v2("p1"),
        uv0: v2("uv0"),
        uv1: v2("uv1"),
        flipped: v.bool_of("flipped").unwrap_or(false),
        advance: v.f32_of("advance").unwrap_or(0.0),
    }
}

/// Parse the `tk2dFontData` stored in a prefab.
pub fn parse_font(scene: &Scene, scripts: &ScriptIndex, materials: &MaterialLibrary) -> Option<Font> {
    let doc = scene.documents().find(|d| {
        d.class_id == class::MONO_BEHAVIOUR && d.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g)) == Some("tk2dFontData")
    })?;
    let b = &doc.body;
    let keys = hex_u32s(b.str_of("charDictKeys").unwrap_or(""));
    let values: Vec<FontChar> = b.get("charDictValues").map(Value::as_seq).unwrap_or(&[]).iter().map(font_char).collect();
    let dict = keys.into_iter().zip(values).collect();
    let chars = b.get("chars").map(Value::as_seq).unwrap_or(&[]).iter().map(font_char).collect();
    let kerning = b
        .get("kerning")
        .map(Value::as_seq)
        .unwrap_or(&[])
        .iter()
        .map(|k| (k.i64_of("c0").unwrap_or(0) as u32, k.i64_of("c1").unwrap_or(0) as u32, k.f32_of("amount").unwrap_or(0.0)))
        .collect();
    let material_guid = b.get("material").and_then(Value::guid).unwrap_or("").to_owned();
    Some(Font {
        name: String::new(),
        version: b.i64_of("version").unwrap_or(0),
        line_height: b.f32_of("lineHeight").unwrap_or(1.0),
        texel_size: b.get("texelSize").and_then(Value::as_vec2).map_or([0.0; 2], |(x, y)| [x, y]),
        use_dictionary: b.bool_of("useDictionary").unwrap_or(false),
        dict,
        chars,
        kerning,
        texture: materials.load(&material_guid).and_then(|m| m.main_texture),
        sprite_collection: b.get("spriteCollection").and_then(Value::guid).map(str::to_owned),
        material_guid,
    })
}

/// Load fonts by prefab GUID, each once.
pub struct FontLibrary {
    prefabs: GuidIndex,
    scripts: ScriptIndex,
    materials: MaterialLibrary,
    pub fonts: HashMap<String, Font>,
}

impl FontLibrary {
    pub fn new(assets_dir: &Path) -> FontLibrary {
        FontLibrary {
            prefabs: GuidIndex::scan(&assets_dir.join("GameObject"), "prefab"),
            scripts: ScriptIndex::scan(&assets_dir.join("Scripts")),
            materials: MaterialLibrary::scan(assets_dir),
            fonts: HashMap::new(),
        }
    }

    pub fn get(&mut self, guid: &str) -> Option<&Font> {
        if !self.fonts.contains_key(guid) {
            let path = self.prefabs.path(guid)?;
            let scene = Scene::load(path).ok()?;
            let mut font = parse_font(&scene, &self.scripts, &self.materials)?;
            font.name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_owned();
            // `tk2dFontData.Init` with `needMaterialInstance`: the collection's instance of the
            // same material, i.e. the collection's texture for it.
            if let Some(collection) = font.sprite_collection.as_ref().and_then(|g| self.prefabs.path(g)) {
                if let Some(c) = Scene::load(collection).ok().and_then(|s| crate::sprites::parse_collection(&s, &self.scripts)) {
                    if let Some(t) = crate::sprites::collection_texture(&c, &self.materials, &font.material_guid) {
                        font.texture = Some(t);
                    }
                }
            }
            self.fonts.insert(guid.to_owned(), font);
        }
        self.fonts.get(guid)
    }
}

/// `tk2dTextMeshData` plus where the text mesh sits.
#[derive(Clone, Debug, PartialEq)]
pub struct SceneText {
    pub object: FileId,
    pub name: String,
    /// GUID of the font prefab.
    pub font: String,
    pub text: String,
    pub color: [f32; 4],
    pub color2: [f32; 4],
    pub use_gradient: bool,
    /// `TextAnchor`: 0 upper-left ... 8 lower-right (row = anchor / 3, column = anchor % 3).
    pub anchor: i64,
    pub scale: [f32; 3],
    pub kerning: bool,
    pub max_chars: usize,
    pub inline_styling: bool,
    pub formatting: bool,
    pub word_wrap_width: i64,
    pub spacing: f32,
    pub line_spacing: f32,
    pub world: Affine,
    pub world_rotation: [f32; 4],
    /// For the `"Text Shadow"` child of a `TextShadower`: the index of the text it copies, the
    /// shadower's `shadowFont` (GUID, if any) and its `shadowPlaneZ`.
    pub shadow_of: Option<(usize, Option<String>, f32)>,
}

/// Every enabled `tk2dTextMesh` of the scene (optionally also under inactive objects), with the
/// `"Text Shadow"` children linked to the text they copy. A `TextShadower` without a serialized
/// shadow child gets one (as its `Awake` creates it).
pub fn scene_texts(scene: &Scene, scripts: &ScriptIndex, include_inactive: bool) -> Vec<SceneText> {
    let class_of = |doc: &crate::yaml::Document| doc.body.get("m_Script").and_then(Value::guid).and_then(|g| scripts.class_name(g));
    let mut texts: Vec<SceneText> = Vec::new();
    let mut index_of: HashMap<FileId, usize> = HashMap::new();
    let mut shadows: Vec<(FileId, FileId)> = Vec::new();
    let mut shadowers: Vec<(FileId, Option<String>, f32)> = Vec::new();
    for go in scene.game_objects() {
        if !include_inactive && !go.active_in_hierarchy() {
            continue;
        }
        let components = go.components();
        if let Some(shadower) = components.iter().find(|c| class_of(c) == Some("TextShadower")) {
            let font = shadower.body.get("shadowFont").and_then(Value::guid).map(str::to_owned);
            shadowers.push((go.id(), font, shadower.body.f32_of("shadowPlaneZ").unwrap_or(0.0)));
        }
        let renderer_on = components.iter().find(|c| c.class_id == class::MESH_RENDERER).is_none_or(|r| r.body.bool_of("m_Enabled").unwrap_or(true));
        let Some(mesh) = components.iter().find(|c| class_of(c) == Some("tk2dTextMesh")) else { continue };
        if !renderer_on || !mesh.body.bool_of("m_Enabled").unwrap_or(true) {
            continue;
        }
        let Some(d) = mesh.body.get("data") else { continue };
        let color = |k: &str| {
            let c = d.get(k);
            ["r", "g", "b", "a"].map(|n| c.and_then(|c| c.f32_of(n)).unwrap_or(1.0))
        };
        index_of.insert(go.id(), texts.len());
        if components.iter().any(|c| class_of(c) == Some("TextShadowPositioner")) {
            if let Some(parent) = go.parent() {
                shadows.push((go.id(), parent.id()));
            }
        }
        texts.push(SceneText {
            object: go.id(),
            name: go.name().to_owned(),
            font: d.get("font").and_then(Value::guid).unwrap_or("").to_owned(),
            text: d.str_of("text").unwrap_or("").to_owned(),
            color: color("color"),
            color2: color("color2"),
            use_gradient: d.bool_of("useGradient").unwrap_or(false),
            anchor: d.i64_of("anchor").unwrap_or(0),
            scale: d.get("scale").and_then(Value::as_vec3).map_or([1.0; 3], |(x, y, z)| [x, y, z]),
            kerning: d.bool_of("kerning").unwrap_or(false),
            max_chars: d.i64_of("maxChars").unwrap_or(16).max(0) as usize,
            inline_styling: d.bool_of("inlineStyling").unwrap_or(false),
            formatting: d.bool_of("formatting").unwrap_or(false),
            word_wrap_width: d.i64_of("wordWrapWidth").unwrap_or(0),
            spacing: d.f32_of("spacing").unwrap_or(0.0),
            line_spacing: d.f32_of("lineSpacing").unwrap_or(0.0),
            world: go.world_affine(),
            world_rotation: crate::sprites::world_rotation_of(go),
            shadow_of: None,
        });
    }
    for (shadow, caster) in shadows {
        if let (Some(&s), Some(&c)) = (index_of.get(&shadow), index_of.get(&caster)) {
            let (font, plane) = shadowers.iter().find(|s| s.0 == caster).map_or((None, 0.0), |s| (s.1.clone(), s.2));
            texts[s].shadow_of = Some((c, font, plane));
        }
    }
    // Shadowers without a serialized shadow child.
    let linked: Vec<usize> = texts.iter().filter_map(|t| t.shadow_of.as_ref().map(|s| s.0)).collect();
    for (caster_object, font, plane) in shadowers {
        let Some(&c) = index_of.get(&caster_object) else { continue };
        if linked.contains(&c) {
            continue;
        }
        let mut shadow = texts[c].clone();
        shadow.name = "Text Shadow".to_owned();
        shadow.shadow_of = Some((c, font, plane));
        texts.push(shadow);
    }
    texts
}
