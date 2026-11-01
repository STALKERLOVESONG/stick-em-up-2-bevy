//! Animation clips (curves) and Animator controllers.
//!
//! Only what the character controller observably depends on is modeled: humanoid root-motion
//! curves, animator-parameter curves, and the state machine's states, blend trees and transitions.

use std::collections::HashMap;
use std::path::Path;

use crate::assets::GuidIndex;
use crate::yaml::{self, Value};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub value: f32,
    pub in_slope: f32,
    pub out_slope: f32,
}

/// A Hermite `AnimationCurve`, clamped outside its key range.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Curve {
    pub keys: Vec<Keyframe>,
}

impl Curve {
    pub fn evaluate(&self, t: f32) -> f32 {
        let keys = &self.keys;
        let (Some(first), Some(last)) = (keys.first(), keys.last()) else { return 0.0 };
        if t <= first.time {
            return first.value;
        }
        if t >= last.time {
            return last.value;
        }
        let idx = keys.partition_point(|k| k.time <= t) - 1;
        let (k0, k1) = (keys[idx], keys[idx + 1]);
        let dt = k1.time - k0.time;
        if k0.out_slope.is_infinite() || k1.in_slope.is_infinite() {
            // Stepped tangent.
            return k0.value;
        }
        let s = (t - k0.time) / dt;
        let (s2, s3) = (s * s, s * s * s);
        let m0 = k0.out_slope * dt;
        let m1 = k1.in_slope * dt;
        (2.0 * s3 - 3.0 * s2 + 1.0) * k0.value + (s3 - 2.0 * s2 + s) * m0 + (-2.0 * s3 + 3.0 * s2) * k1.value + (s3 - s2) * m1
    }

    pub fn first_value(&self) -> f32 {
        self.keys.first().map_or(0.0, |k| k.value)
    }

    pub fn last_value(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.value)
    }
}

fn parse_curve(v: &Value) -> Curve {
    let keys = v
        .get("m_Curve")
        .map(Value::as_seq)
        .unwrap_or(&[])
        .iter()
        .filter_map(|k| {
            Some(Keyframe {
                time: k.f32_of("time")?,
                value: k.f32_of("value")?,
                in_slope: k.f32_of("inSlope").unwrap_or(0.0),
                out_slope: k.f32_of("outSlope").unwrap_or(0.0),
            })
        })
        .collect();
    Curve { keys }
}

/// Humanoid root translation curves (`RootT.x/y/z`), in normalized (avatar-scaled) units.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RootMotion {
    pub x: Curve,
    pub y: Curve,
    pub z: Curve,
}

/// Root-transform handling of a humanoid clip (`AnimationClipSettings`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClipSettings {
    /// `m_LoopBlendOrientation`: root rotation stays in the pose ("Bake Into Pose").
    pub bake_orientation: bool,
    /// `m_LoopBlendPositionY`.
    pub bake_position_y: bool,
    /// `m_LoopBlendPositionXZ`.
    pub bake_position_xz: bool,
    /// `m_OrientationOffsetY`, degrees.
    pub orientation_offset_y: f32,
    pub mirror: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Clip {
    pub name: String,
    /// `m_StopTime - m_StartTime`.
    pub length: f32,
    pub looping: bool,
    pub root_motion: Option<RootMotion>,
    /// Float curves keyed by their `attribute` string (animator-parameter curves appear as
    /// `typetree_0x<crc32>_...`).
    pub float_curves: HashMap<String, Curve>,
    /// Humanoid curves by attribute name: muscles (`"Spine Front-Back"`, ...) and
    /// `RootQ.x/y/z/w`. Root translation is in [`Clip::root_motion`].
    pub humanoid_curves: HashMap<String, Curve>,
    pub settings: ClipSettings,
}

/// CRC-32 (IEEE), which Unity uses for `Animator.StringToHash` bindings in clips.
pub fn crc32(text: &str) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in text.as_bytes() {
        crc ^= byte as u32;
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 != 0 { 0xEDB8_8320 } else { 0 };
        }
    }
    !crc
}

impl Clip {
    /// The curve that drives animator parameter `name`, if this clip animates it.
    pub fn parameter_curve(&self, name: &str) -> Option<&Curve> {
        let tag = format!("typetree_0x{:08X}", crc32(name));
        self.float_curves.iter().find(|(attr, _)| attr.starts_with(&tag)).map(|(_, c)| c)
    }
}

pub fn parse_clip(text: &str) -> Option<Clip> {
    let docs = yaml::parse_documents(text);
    let doc = docs.iter().find(|d| d.type_name == "AnimationClip")?;
    let body = &doc.body;
    let settings = body.get("m_AnimationClipSettings")?;
    let start = settings.f32_of("m_StartTime").unwrap_or(0.0);
    let stop = settings.f32_of("m_StopTime").unwrap_or(0.0);
    let mut float_curves = HashMap::new();
    let mut humanoid_curves = HashMap::new();
    let (mut rx, mut ry, mut rz) = (None, None, None);
    for entry in body.get("m_FloatCurves").map(Value::as_seq).unwrap_or(&[]) {
        let attribute = entry.str_of("attribute").unwrap_or("");
        let curve = parse_curve(entry.get("curve").unwrap_or(&Value::Null));
        match attribute {
            "RootT.x" => rx = Some(curve),
            "RootT.y" => ry = Some(curve),
            "RootT.z" => rz = Some(curve),
            a if a.starts_with("typetree_") => {
                float_curves.insert(a.to_owned(), curve);
            }
            a if !a.is_empty() => {
                humanoid_curves.insert(a.to_owned(), curve);
            }
            _ => {}
        }
    }
    let root_motion = match (rx, ry, rz) {
        (Some(x), Some(y), Some(z)) => Some(RootMotion { x, y, z }),
        _ => None,
    };
    Some(Clip {
        name: body.str_of("m_Name").unwrap_or("").to_owned(),
        length: stop - start,
        looping: settings.bool_of("m_LoopTime").unwrap_or(false),
        root_motion,
        float_curves,
        humanoid_curves,
        settings: ClipSettings {
            bake_orientation: settings.bool_of("m_LoopBlendOrientation").unwrap_or(false),
            bake_position_y: settings.bool_of("m_LoopBlendPositionY").unwrap_or(false),
            bake_position_xz: settings.bool_of("m_LoopBlendPositionXZ").unwrap_or(false),
            orientation_offset_y: settings.f32_of("m_OrientationOffsetY").unwrap_or(0.0),
            mirror: settings.bool_of("m_Mirror").unwrap_or(false),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Generic transform clips (the weapons' firing animations)
// ---------------------------------------------------------------------------------------------

/// A non-humanoid clip that moves and turns transforms below the animated object: the weapons'
/// firing animations ("Sword Swing", "Shuriken Throw", "Spear Throw"), played by the weapon
/// pivot's own `Animator` when its `ProjectileLauncher` fires (`firingAnimator.SetTrigger`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TransformClip {
    pub name: String,
    /// `m_StopTime - m_StartTime`.
    pub length: f32,
    /// `m_PositionCurves`: the path below the animated object and x, y, z.
    pub positions: Vec<(String, [Curve; 3])>,
    /// `m_RotationCurves`: the path and the quaternion's x, y, z, w.
    pub rotations: Vec<(String, [Curve; 4])>,
}

fn parse_vector_curves<const N: usize>(entries: &[Value]) -> Vec<(String, [Curve; N])> {
    const AXES: [&str; 4] = ["x", "y", "z", "w"];
    entries
        .iter()
        .map(|entry| {
            let path = entry.str_of("path").unwrap_or("").to_owned();
            let keys = entry.get("curve").and_then(|c| c.get("m_Curve")).map(Value::as_seq).unwrap_or(&[]);
            let curves = std::array::from_fn(|axis| Curve {
                keys: keys
                    .iter()
                    .filter_map(|k| {
                        let pick = |name: &str| k.get(name).and_then(|v| v.f32_of(AXES[axis]));
                        Some(Keyframe { time: k.f32_of("time")?, value: pick("value")?, in_slope: pick("inSlope").unwrap_or(0.0), out_slope: pick("outSlope").unwrap_or(0.0) })
                    })
                    .collect(),
            });
            (path, curves)
        })
        .collect()
}

/// Read a [`TransformClip`] (`.anim` text).
pub fn parse_transform_clip(text: &str) -> Option<TransformClip> {
    let docs = yaml::parse_documents(text);
    let body = &docs.iter().find(|d| d.type_name == "AnimationClip")?.body;
    let settings = body.get("m_AnimationClipSettings");
    let start = settings.and_then(|s| s.f32_of("m_StartTime")).unwrap_or(0.0);
    let stop = settings.and_then(|s| s.f32_of("m_StopTime")).unwrap_or(0.0);
    Some(TransformClip {
        name: body.str_of("m_Name").unwrap_or("").to_owned(),
        length: stop - start,
        positions: parse_vector_curves::<3>(body.get("m_PositionCurves").map(Value::as_seq).unwrap_or(&[])),
        rotations: parse_vector_curves::<4>(body.get("m_RotationCurves").map(Value::as_seq).unwrap_or(&[])),
    })
}

/// The firing animation of the rig weapon `weapon` ("Sword" → "Sword Swing.anim", "Shuriken" /
/// "Spear" → "… Throw.anim").
///
/// TODO(UNCERTAIN): the weapon pivots' animator controllers were not exported; each clip is
/// matched to its weapon by name, and played once from its start on every shot (the controllers'
/// transitions and their timing are not known).
pub fn load_weapon_action(assets_dir: &Path, weapon: &str) -> Option<TransformClip> {
    ["Swing", "Throw"].iter().find_map(|verb| {
        let text = std::fs::read_to_string(assets_dir.join("AnimationClip").join(format!("{weapon} {verb}.anim"))).ok()?;
        parse_transform_clip(&text)
    })
}

// ---------------------------------------------------------------------------------------------
// Animator controller
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConditionMode {
    If,
    IfNot,
    Greater,
    Less,
    Equals,
    NotEqual,
}

#[derive(Clone, Debug)]
pub struct Condition {
    pub mode: ConditionMode,
    pub parameter: String,
    pub threshold: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interruption {
    None,
    Source,
    Destination,
    SourceThenDestination,
    DestinationThenSource,
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub destination: usize,
    pub conditions: Vec<Condition>,
    /// Seconds when `fixed_duration`, otherwise a fraction of the source state's duration.
    pub duration: f32,
    pub fixed_duration: bool,
    /// Normalized start time in the destination state.
    pub offset: f32,
    pub has_exit_time: bool,
    pub exit_time: f32,
    pub interruption: Interruption,
}

#[derive(Clone, Debug)]
pub struct BlendChild {
    pub clip_guid: String,
    pub threshold: f32,
    pub time_scale: f32,
}

#[derive(Clone, Debug)]
pub enum Motion {
    None,
    Clip(String),
    /// 1D blend tree.
    Blend { parameter: String, children: Vec<BlendChild> },
}

#[derive(Clone, Debug)]
pub struct State {
    pub name: String,
    pub speed: f32,
    pub tag: String,
    pub motion: Motion,
    pub transitions: Vec<Transition>,
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub name: String,
    pub default: f32,
}

#[derive(Clone, Debug)]
pub struct Controller {
    pub parameters: Vec<Parameter>,
    pub states: Vec<State>,
    pub default_state: usize,
}

impl Controller {
    pub fn state_index(&self, name: &str) -> Option<usize> {
        self.states.iter().position(|s| s.name == name)
    }
}

fn parse_motion(file_id_or_guid: &Value, docs: &HashMap<i64, yaml::Document>) -> Motion {
    if let Some(guid) = file_id_or_guid.guid() {
        return Motion::Clip(guid.to_owned());
    }
    let Some(id) = file_id_or_guid.file_id() else { return Motion::None };
    let Some(doc) = docs.get(&id) else { return Motion::None };
    if doc.type_name != "BlendTree" {
        return Motion::None;
    }
    let children = doc
        .body
        .get("m_Childs")
        .map(Value::as_seq)
        .unwrap_or(&[])
        .iter()
        .filter_map(|c| {
            Some(BlendChild {
                clip_guid: c.get("m_Motion")?.guid()?.to_owned(),
                threshold: c.f32_of("m_Threshold")?,
                time_scale: c.f32_of("m_TimeScale").unwrap_or(1.0),
            })
        })
        .collect();
    Motion::Blend { parameter: doc.body.str_of("m_BlendParameter").unwrap_or("").to_owned(), children }
}

pub fn parse_controller(text: &str) -> Option<Controller> {
    let docs = yaml::index_by_file_id(yaml::parse_documents(text));
    let root = docs.values().find(|d| d.type_name == "AnimatorController")?;
    let parameters = root
        .body
        .get("m_AnimatorParameters")
        .map(Value::as_seq)
        .unwrap_or(&[])
        .iter()
        .map(|p| Parameter {
            name: p.str_of("m_Name").unwrap_or("").to_owned(),
            // Bool/trigger defaults live in `m_DefaultBool`; floats in `m_DefaultFloat`.
            default: p.f32_of("m_DefaultFloat").unwrap_or(0.0),
        })
        .collect();

    // State file ids in a stable order (file-id order of the serialized states).
    let mut state_ids: Vec<i64> = docs.values().filter(|d| d.type_name == "AnimatorState").map(|d| d.file_id).collect();
    state_ids.sort_unstable();
    let index_of = |id: i64| state_ids.iter().position(|&s| s == id);

    let mut states = Vec::new();
    for &id in &state_ids {
        let doc = &docs[&id];
        let mut transitions = Vec::new();
        for t in doc.body.get("m_Transitions").map(Value::as_seq).unwrap_or(&[]) {
            let Some(tdoc) = t.file_id().and_then(|i| docs.get(&i)) else { continue };
            let b = &tdoc.body;
            let Some(destination) = b.get("m_DstState").and_then(Value::file_id).and_then(index_of) else { continue };
            let conditions = b
                .get("m_Conditions")
                .map(Value::as_seq)
                .unwrap_or(&[])
                .iter()
                .filter_map(|c| {
                    let mode = match c.i64_of("m_ConditionMode")? {
                        1 => ConditionMode::If,
                        2 => ConditionMode::IfNot,
                        3 => ConditionMode::Greater,
                        4 => ConditionMode::Less,
                        6 => ConditionMode::Equals,
                        7 => ConditionMode::NotEqual,
                        _ => return None,
                    };
                    Some(Condition {
                        mode,
                        parameter: c.str_of("m_ConditionEvent")?.to_owned(),
                        threshold: c.f32_of("m_EventTreshold").unwrap_or(0.0),
                    })
                })
                .collect();
            transitions.push(Transition {
                destination,
                conditions,
                duration: b.f32_of("m_TransitionDuration").unwrap_or(0.0),
                fixed_duration: b.bool_of("m_HasFixedDuration").unwrap_or(true),
                offset: b.f32_of("m_TransitionOffset").unwrap_or(0.0),
                has_exit_time: b.bool_of("m_HasExitTime").unwrap_or(false),
                exit_time: b.f32_of("m_ExitTime").unwrap_or(0.0),
                interruption: match b.i64_of("m_InterruptionSource").unwrap_or(0) {
                    1 => Interruption::Source,
                    2 => Interruption::Destination,
                    3 => Interruption::SourceThenDestination,
                    4 => Interruption::DestinationThenSource,
                    _ => Interruption::None,
                },
            });
        }
        states.push(State {
            name: doc.body.str_of("m_Name").unwrap_or("").to_owned(),
            speed: doc.body.f32_of("m_Speed").unwrap_or(1.0),
            tag: doc.body.str_of("m_Tag").unwrap_or("").to_owned(),
            motion: doc.body.get("m_Motion").map(|m| parse_motion(m, &docs)).unwrap_or(Motion::None),
            transitions,
        });
    }

    let default_state = docs
        .values()
        .find(|d| d.type_name == "AnimatorStateMachine")
        .and_then(|d| d.body.get("m_DefaultState")?.file_id())
        .and_then(index_of)
        .unwrap_or(0);
    Some(Controller { parameters, states, default_state })
}

/// Everything the animator model needs for the player character, loaded from disk.
pub struct CharacterAnimationData {
    pub controller: Controller,
    pub clips: HashMap<String, Clip>,
    /// Humanoid avatar scale (`m_Human.m_Scale`), which scales root-motion translation.
    pub human_scale: f32,
}

/// Find `m_Scale` under the avatar's `m_Human` block.
pub fn parse_avatar_scale(text: &str) -> Option<f32> {
    let docs = yaml::parse_documents(text);
    let avatar = docs.iter().find(|d| d.type_name == "Avatar")?;
    avatar.body.get("m_Avatar")?.get("m_Human")?.get("data")?.get("m_Scale")?.as_f32()
}

/// Load the `Character` animator controller, every clip it references, and the `ManAvatar` scale
/// from the exported project (`Assets/` directory).
pub fn load_character_animation(assets_dir: &Path) -> std::io::Result<CharacterAnimationData> {
    let controller_text = std::fs::read_to_string(assets_dir.join("AnimatorController/Character.controller"))?;
    let controller = parse_controller(&controller_text).ok_or_else(|| std::io::Error::other("bad controller"))?;
    let index = GuidIndex::scan(&assets_dir.join("AnimationClip"), "anim");
    let mut clips = HashMap::new();
    for state in &controller.states {
        let guids: Vec<&str> = match &state.motion {
            Motion::Clip(g) => vec![g.as_str()],
            Motion::Blend { children, .. } => children.iter().map(|c| c.clip_guid.as_str()).collect(),
            Motion::None => vec![],
        };
        for guid in guids {
            if clips.contains_key(guid) {
                continue;
            }
            let Some(path) = index.path(guid) else { continue };
            // A missing clip file only matters if the state is used; leave it out of the map.
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            if let Some(clip) = parse_clip(&text) {
                clips.insert(guid.to_owned(), clip);
            }
        }
    }
    let avatar_text = std::fs::read_to_string(assets_dir.join("Avatar/ManAvatar.asset"))?;
    let human_scale = parse_avatar_scale(&avatar_text).ok_or_else(|| std::io::Error::other("avatar scale missing"))?;
    Ok(CharacterAnimationData { controller, clips, human_scale })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc32_matches_unity_parameter_hash() {
        assert_eq!(crc32("Collider Height Fraction"), 0x8308_C4DE);
    }

    #[test]
    fn hermite_curve_interpolates_and_clamps() {
        let c = Curve {
            keys: vec![
                Keyframe { time: 0.0, value: 0.0, in_slope: 0.0, out_slope: 2.0 },
                Keyframe { time: 1.0, value: 1.0, in_slope: 0.0, out_slope: 0.0 },
            ],
        };
        assert_eq!(c.evaluate(-1.0), 0.0);
        assert_eq!(c.evaluate(2.0), 1.0);
        let mid = c.evaluate(0.5);
        assert!(mid > 0.5 && mid < 1.0, "{mid}");
    }
}
