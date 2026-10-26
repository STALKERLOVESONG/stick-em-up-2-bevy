//! The character rig of `Man.prefab`: the bone hierarchy and the visual components on it.
//!
//! Only visual and aiming data is read here (stick-figure lines, flat sprites, colours, weapon
//! pivots); gameplay components (health, launchers) are read by their own loaders.

use std::collections::HashMap;
use std::path::Path;

use crate::scene::{FileId, GameObject, Scene, class};
use crate::scripts::ScriptIndex;
use crate::yaml::Value;

/// `LinesToChildren`: draws a line from this bone to each unskipped child.
#[derive(Clone, Debug, PartialEq)]
pub struct LinesSpec {
    pub z: f32,
    pub dont_rotate_z: bool,
    pub width: f32,
    /// `Color32` as `[r, g, b, a]`.
    pub color: [u8; 4],
    pub shadow_plane_z: f32,
    /// Children (bone indices) that get no line.
    pub skip_children: Vec<usize>,
}

/// `FlatSprite`: a camera-facing sprite that follows a bone.
#[derive(Clone, Debug, PartialEq)]
pub struct FlatSpriteSpec {
    pub sprite_name: String,
    pub color: [f32; 4],
    pub scale: [f32; 3],
    pub root_relative_z: f32,
    pub shadow_plane_z: f32,
}

#[derive(Clone, Debug)]
pub struct RigBone {
    pub name: String,
    pub parent: Option<usize>,
    /// Children in `m_Children` order (the order `LinesToChildren` iterates).
    pub children: Vec<usize>,
    pub position: [f32; 3],
    /// `(x, y, z, w)`.
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
    /// `GameObject.activeSelf` in the prefab.
    pub active_self: bool,
    pub lines: Option<LinesSpec>,
    pub flat_sprite: Option<FlatSpriteSpec>,
    /// Hierarchy path below the root (`Armature/Hips/Waist`); empty for the root.
    pub path: String,
}

/// `CharacterColor`: which parts take the player's colours, and the prefab defaults.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorSpec {
    pub body_color: [u8; 4],
    pub face_color: [u8; 4],
    pub helmet_color: [f32; 4],
    /// Bones whose `LinesToChildren` is body-coloured.
    pub body: Vec<usize>,
    /// Bone whose `LinesToChildren` is the face (head) line.
    pub face: usize,
    /// Bone carrying the helmet `FlatSprite`.
    pub helmet: usize,
}

/// One weapon of the `Weapon Pivots` hierarchy: the bone that is switched on and aimed, its
/// hand targets, and its `ProjectileLauncher`.
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponSpec {
    /// Child name under `Weapon Pivots` (`"Assault Rifle"`); `SetWeapon` finds it by name.
    pub name: String,
    /// The weapon's own bone; `LookAt` aims it.
    pub bone: usize,
    pub left_hand: Option<usize>,
    pub right_hand: Option<usize>,
    /// Bone of the `ProjectileLauncher` (projectiles spawn at its position).
    pub launch_point: Option<usize>,
    /// `ProjectileLauncher.pivot`: the bone the aim direction is measured from.
    pub aim_pivot: Option<usize>,
    pub firing_rate_time: f32,
    pub fully_automatic: bool,
    /// GUID of the projectile prefab.
    pub projectile: String,
    pub firing_sound: String,
    /// Bones with a `FlatSprite` inside this weapon (its visible parts).
    pub sprite_bones: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Rig {
    pub bones: Vec<RigBone>,
    pub root: usize,
    pub colors: ColorSpec,
    /// Weapons in `Weapon Pivots` child order (the order `weaponNames` is built in).
    pub weapons: Vec<WeaponSpec>,
    by_path: HashMap<String, usize>,
}

impl Rig {
    pub fn bone_by_path(&self, path: &str) -> Option<usize> {
        self.by_path.get(path).copied()
    }
}

fn unity_color32(v: &Value) -> [u8; 4] {
    // Serialized as one little-endian `rgba` integer: r in the low byte.
    let packed = v.get("rgba").and_then(Value::as_i64).unwrap_or(0xFFFF_FFFF) as u32;
    [packed as u8, (packed >> 8) as u8, (packed >> 16) as u8, (packed >> 24) as u8]
}

fn vec3(v: Option<&Value>, default: [f32; 3]) -> [f32; 3] {
    v.and_then(Value::as_vec3).map_or(default, |(x, y, z)| [x, y, z])
}

/// Parse `Man.prefab`.
pub fn parse_rig(scene: &Scene, scripts: &ScriptIndex) -> Option<Rig> {
    let objects = scene.game_objects();
    let root_go = objects.iter().find(|g| g.parent().is_none())?;

    // Depth-first so that parents precede children.
    let mut order: Vec<(GameObject, Option<usize>)> = Vec::new();
    fn visit<'a>(go: GameObject<'a>, parent: Option<usize>, out: &mut Vec<(GameObject<'a>, Option<usize>)>) {
        let index = out.len();
        out.push((go, parent));
        for child in go.children() {
            visit(child, Some(index), out);
        }
    }
    visit(*root_go, None, &mut order);

    let index_of_go: HashMap<FileId, usize> = order.iter().enumerate().map(|(i, (g, _))| (g.id(), i)).collect();
    let transform_to_bone = |transform_id: FileId| -> Option<usize> {
        let go = scene.get(transform_id)?.body.get("m_GameObject")?.file_id()?;
        index_of_go.get(&go).copied()
    };

    let mut bones: Vec<RigBone> = Vec::new();
    let mut lines_component_bone: HashMap<FileId, usize> = HashMap::new();
    let mut flat_component_bone: HashMap<FileId, usize> = HashMap::new();
    let mut color_doc = None;
    let mut launchers: Vec<(usize, &crate::yaml::Document)> = Vec::new();
    for (index, (go, parent)) in order.iter().enumerate() {
        let transform = go.transform()?;
        let mut lines = None;
        let mut flat_sprite = None;
        for component in go.components() {
            if component.class_id != class::MONO_BEHAVIOUR {
                continue;
            }
            let Some(guid) = component.body.get("m_Script").and_then(Value::guid) else { continue };
            match scripts.class_name(guid) {
                Some("LinesToChildren") => {
                    let b = &component.body;
                    let skip = b
                        .get("skipLinesForChildren")
                        .map(Value::as_seq)
                        .unwrap_or(&[])
                        .iter()
                        .filter_map(|r| transform_to_bone(r.file_id()?))
                        .collect();
                    lines = Some(LinesSpec {
                        z: b.f32_of("z").unwrap_or(0.0),
                        dont_rotate_z: b.bool_of("dontRotateZ").unwrap_or(false),
                        width: b.f32_of("width").unwrap_or(1.0),
                        color: b.get("color").map_or([255; 4], unity_color32),
                        shadow_plane_z: b.f32_of("shadowPlaneZ").unwrap_or(0.0),
                        skip_children: skip,
                    });
                    lines_component_bone.insert(component.file_id, index);
                }
                Some("FlatSprite") => {
                    let b = &component.body;
                    let c = b.get("color");
                    flat_sprite = Some(FlatSpriteSpec {
                        sprite_name: b.str_of("spriteName").unwrap_or("").to_owned(),
                        color: ["r", "g", "b", "a"].map(|k| c.and_then(|c| c.f32_of(k)).unwrap_or(1.0)),
                        scale: vec3(b.get("scale"), [1.0; 3]),
                        root_relative_z: b.f32_of("rootRelativeZ").unwrap_or(0.0),
                        shadow_plane_z: b.f32_of("shadowPlaneZ").unwrap_or(0.0),
                    });
                    flat_component_bone.insert(component.file_id, index);
                }
                Some("CharacterColor") => color_doc = Some(component),
                Some("ProjectileLauncher") => launchers.push((index, component)),
                _ => {}
            }
        }
        let rotation = transform.body.get("m_LocalRotation").and_then(Value::as_quat).map_or([0.0, 0.0, 0.0, 1.0], |(x, y, z, w)| [x, y, z, w]);
        bones.push(RigBone {
            name: go.name().to_owned(),
            parent: *parent,
            children: Vec::new(),
            position: vec3(transform.body.get("m_LocalPosition"), [0.0; 3]),
            rotation,
            scale: vec3(transform.body.get("m_LocalScale"), [1.0; 3]),
            active_self: go.active_self(),
            lines,
            flat_sprite,
            path: String::new(),
        });
    }
    for i in 1..bones.len() {
        let parent = bones[i].parent?;
        bones[parent].children.push(i);
    }
    let mut by_path = HashMap::new();
    for i in 1..bones.len() {
        let parent_path = bones[bones[i].parent?].path.clone();
        let path = if parent_path.is_empty() { bones[i].name.clone() } else { format!("{parent_path}/{}", bones[i].name) };
        bones[i].path = path.clone();
        by_path.insert(path, i);
    }

    let cc = &color_doc?.body;
    let color4 = |key: &str| cc.get(key).map_or([255; 4], unity_color32);
    let helmet = cc.get("helmetColor");
    let colors = ColorSpec {
        body_color: color4("bodyColor"),
        face_color: color4("faceColor"),
        helmet_color: ["r", "g", "b", "a"].map(|k| helmet.and_then(|c| c.f32_of(k)).unwrap_or(1.0)),
        body: cc
            .get("body")
            .map(Value::as_seq)
            .unwrap_or(&[])
            .iter()
            .filter_map(|r| lines_component_bone.get(&r.file_id()?).copied())
            .collect(),
        face: *cc.get("face").and_then(Value::file_id).and_then(|id| lines_component_bone.get(&id))?,
        helmet: *cc.get("helmet").and_then(Value::file_id).and_then(|id| flat_component_bone.get(&id))?,
    };
    let weapons = parse_weapons(&bones, &by_path, &launchers, &transform_to_bone);
    Some(Rig { bones, root: 0, colors, weapons, by_path })
}

fn parse_weapons(
    bones: &[RigBone],
    by_path: &HashMap<String, usize>,
    launchers: &[(usize, &crate::yaml::Document)],
    transform_to_bone: &dyn Fn(FileId) -> Option<usize>,
) -> Vec<WeaponSpec> {
    let Some(pivots) = by_path.iter().find(|(path, _)| path.ends_with("Weapon Pivots")).map(|(_, &i)| i) else {
        return Vec::new();
    };
    let within = |bone: usize, ancestor: usize| {
        let mut current = Some(bone);
        while let Some(i) = current {
            if i == ancestor {
                return true;
            }
            current = bones[i].parent;
        }
        false
    };
    bones[pivots]
        .children
        .iter()
        .map(|&weapon| {
            let child = |name: &str| bones[weapon].children.iter().copied().find(|&c| bones[c].name == name);
            let launcher = launchers.iter().find(|(bone, _)| within(*bone, weapon));
            let body = launcher.map(|(_, doc)| &doc.body);
            WeaponSpec {
                name: bones[weapon].name.clone(),
                bone: weapon,
                left_hand: child("Left Hand"),
                right_hand: child("Right Hand"),
                launch_point: launcher.map(|(bone, _)| *bone),
                aim_pivot: body.and_then(|b| b.get("pivot")).and_then(Value::file_id).and_then(transform_to_bone),
                firing_rate_time: body.and_then(|b| b.f32_of("firingRateTime")).unwrap_or(0.0),
                fully_automatic: body.and_then(|b| b.bool_of("fullyAutomatic")).unwrap_or(true),
                projectile: body.and_then(|b| b.get("prefab")).and_then(Value::guid).unwrap_or("").to_owned(),
                firing_sound: body.and_then(|b| b.str_of("firingSound")).unwrap_or("").to_owned(),
                sprite_bones: (0..bones.len()).filter(|&i| bones[i].flat_sprite.is_some() && within(i, weapon)).collect(),
            }
        })
        .collect()
}

/// Load `GameObject/Man.prefab` from the exported `Assets/` directory.
pub fn load_man_rig(assets_dir: &Path) -> std::io::Result<Rig> {
    let scene = Scene::load(&assets_dir.join("GameObject/Man.prefab"))?;
    let scripts = ScriptIndex::scan(&assets_dir.join("Scripts"));
    parse_rig(&scene, &scripts).ok_or_else(|| std::io::Error::other("Man.prefab: rig not recognised"))
}
