//! Materials: just enough to find the texture and tint a surface should be drawn with.

use std::path::{Path, PathBuf};

use crate::assets::GuidIndex;
use crate::yaml::{self, Value};

/// What the game needs from a Unity `.mat` asset.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// `_MainTex` asset path, if the material has one.
    pub main_texture: Option<PathBuf>,
    pub tiling: [f32; 2],
    pub offset: [f32; 2],
    /// `_Color` (white when absent).
    pub color: [f32; 4],
}

/// GUID lookups for materials and 2D textures of an exported project's `Assets/` directory.
pub struct MaterialLibrary {
    materials: GuidIndex,
    textures: GuidIndex,
    text_assets: GuidIndex,
}

impl MaterialLibrary {
    pub fn scan(assets_dir: &Path) -> MaterialLibrary {
        MaterialLibrary {
            materials: GuidIndex::scan(&assets_dir.join("Material"), "mat"),
            textures: GuidIndex::scan(&assets_dir.join("Texture2D"), "png"),
            text_assets: GuidIndex::scan(&assets_dir.join("TextAsset"), "bytes"),
        }
    }

    /// A `TextAsset` file by GUID (`*.bytes`).
    pub fn text_asset(&self, guid: &str) -> Option<PathBuf> {
        self.text_assets.path(guid).map(Path::to_path_buf)
    }

    /// A texture file by GUID.
    pub fn texture(&self, guid: &str) -> Option<PathBuf> {
        self.textures.path(guid).map(Path::to_path_buf)
    }

    pub fn load(&self, material_guid: &str) -> Option<Material> {
        let text = std::fs::read_to_string(self.materials.path(material_guid)?).ok()?;
        parse_material(&text, &self.textures)
    }
}

fn parse_material(text: &str, textures: &GuidIndex) -> Option<Material> {
    let docs = yaml::parse_documents(text);
    let doc = docs.iter().find(|d| d.type_name == "Material")?;
    let saved = doc.body.get("m_SavedProperties")?;
    let mut main_texture = None;
    let (mut tiling, mut offset) = ([1.0, 1.0], [0.0, 0.0]);
    // `m_TexEnvs.data` is a list of single-entry maps in the export; the yaml reader keeps them
    // as repeated `data` keys.
    if let Some(Value::Map(entries)) = saved.get("m_TexEnvs") {
        for (_, entry) in entries.iter().filter(|(k, _)| k == "data") {
            if entry.get("first").and_then(|f| f.str_of("name")) == Some("_MainTex") {
                let second = entry.get("second")?;
                main_texture = second.get("m_Texture").and_then(|t| t.guid()).and_then(|g| textures.path(g)).map(Path::to_path_buf);
                if let Some((x, y)) = second.get("m_Scale").and_then(Value::as_vec2) {
                    tiling = [x, y];
                }
                if let Some((x, y)) = second.get("m_Offset").and_then(Value::as_vec2) {
                    offset = [x, y];
                }
            }
        }
    }
    let mut color = [1.0; 4];
    if let Some(Value::Map(entries)) = saved.get("m_Colors") {
        for (_, entry) in entries.iter().filter(|(k, _)| k == "data") {
            if entry.get("first").and_then(|f| f.str_of("name")) == Some("_Color") {
                if let Some(c) = entry.get("second") {
                    color = ["r", "g", "b", "a"].map(|k| c.f32_of(k).unwrap_or(1.0));
                }
            }
        }
    }
    Some(Material { main_texture, tiling, offset, color })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_texture_guid_and_color() {
        let dir = std::env::temp_dir().join(format!("matlib_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Texture2D")).unwrap();
        std::fs::write(dir.join("Texture2D/Ground.png"), b"").unwrap();
        std::fs::write(dir.join("Texture2D/Ground.png.meta"), "fileFormatVersion: 2\nguid: 7ec4db48f3f5956489566cbada461656\n").unwrap();
        let textures = GuidIndex::scan(&dir.join("Texture2D"), "png");
        let text = "%YAML 1.1\n--- !u!21 &2100000\nMaterial:\n  m_Name: Cardboard\n  m_SavedProperties:\n    serializedVersion: 2\n    m_TexEnvs:\n      data:\n        first:\n          name: _MainTex\n        second:\n          m_Texture: {fileID: 2800000, guid: 7ec4db48f3f5956489566cbada461656, type: 3}\n          m_Scale: {x: 2, y: 3}\n          m_Offset: {x: 0, y: 0}\n    m_Floats: {}\n    m_Colors:\n      data:\n        first:\n          name: _Color\n        second: {r: 1, g: 0.5, b: 1, a: 1}\n";
        let m = parse_material(text, &textures).unwrap();
        assert_eq!(m.main_texture.as_deref(), Some(dir.join("Texture2D/Ground.png").as_path()));
        assert_eq!(m.tiling, [2.0, 3.0]);
        assert_eq!(m.color, [1.0, 0.5, 1.0, 1.0]);
        std::fs::remove_dir_all(dir).ok();
    }
}
