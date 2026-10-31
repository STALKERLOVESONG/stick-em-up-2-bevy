//! `ProjectSettings/*.asset` readers (Physics2D, Time).

use crate::yaml;

/// `Physics2DSettings.asset`.
#[derive(Clone, Debug, PartialEq)]
pub struct Physics2dSettings {
    pub gravity: (f32, f32),
    pub velocity_iterations: u32,
    pub position_iterations: u32,
    pub raycasts_hit_triggers: bool,
    /// `m_LayerCollisionMatrix`: word `a` has bit `b` set when layers `a` and `b` collide.
    pub layer_collision_matrix: [u32; 32],
    /// The Box2D tuning Unity exposes (`b2_velocityThreshold`, `b2_maxLinearCorrection`, ...).
    pub box2d: Box2dTuning,
}

/// `Physics2DSettings` values that replace Box2D's compile-time constants.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Box2dTuning {
    pub velocity_threshold: f32,
    pub max_linear_correction: f32,
    /// Degrees.
    pub max_angular_correction: f32,
    pub max_translation_speed: f32,
    /// Degrees per second.
    pub max_rotation_speed: f32,
    pub min_penetration_for_penalty: f32,
    pub baumgarte_scale: f32,
    pub baumgarte_time_of_impact_scale: f32,
    pub time_to_sleep: f32,
    pub linear_sleep_tolerance: f32,
    /// Degrees per second.
    pub angular_sleep_tolerance: f32,
}

impl Default for Box2dTuning {
    /// Unity 5's defaults (what this project also uses).
    fn default() -> Self {
        Box2dTuning {
            velocity_threshold: 1.0,
            max_linear_correction: 0.2,
            max_angular_correction: 8.0,
            max_translation_speed: 100.0,
            max_rotation_speed: 360.0,
            min_penetration_for_penalty: 0.01,
            baumgarte_scale: 0.2,
            baumgarte_time_of_impact_scale: 0.75,
            time_to_sleep: 0.5,
            linear_sleep_tolerance: 0.01,
            angular_sleep_tolerance: 2.0,
        }
    }
}

impl Physics2dSettings {
    pub fn layers_collide(&self, a: u8, b: u8) -> bool {
        self.layer_collision_matrix[(a & 31) as usize] >> (b & 31) & 1 != 0
    }
}

pub fn parse_physics2d(text: &str) -> Option<Physics2dSettings> {
    let docs = yaml::parse_documents(text);
    let body = &docs.iter().find(|d| d.type_name == "Physics2DSettings")?.body;
    let hex = body.str_of("m_LayerCollisionMatrix")?;
    if hex.len() != 32 * 8 {
        return None;
    }
    let mut matrix = [0u32; 32];
    for (i, word) in matrix.iter_mut().enumerate() {
        // Each word is stored as little-endian bytes in hex.
        let mut bytes = [0u8; 4];
        for (b, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 8 + b * 2..i * 8 + b * 2 + 2], 16).ok()?;
        }
        *word = u32::from_le_bytes(bytes);
    }
    Some(Physics2dSettings {
        gravity: body.get("m_Gravity")?.as_vec2()?,
        velocity_iterations: body.i64_of("m_VelocityIterations")? as u32,
        position_iterations: body.i64_of("m_PositionIterations")? as u32,
        raycasts_hit_triggers: body.bool_of("m_RaycastsHitTriggers").unwrap_or(true),
        layer_collision_matrix: matrix,
        box2d: {
            let d = Box2dTuning::default();
            let f = |k: &str, v: f32| body.f32_of(k).unwrap_or(v);
            Box2dTuning {
                velocity_threshold: f("m_VelocityThreshold", d.velocity_threshold),
                max_linear_correction: f("m_MaxLinearCorrection", d.max_linear_correction),
                max_angular_correction: f("m_MaxAngularCorrection", d.max_angular_correction),
                max_translation_speed: f("m_MaxTranslationSpeed", d.max_translation_speed),
                max_rotation_speed: f("m_MaxRotationSpeed", d.max_rotation_speed),
                min_penetration_for_penalty: f("m_MinPenetrationForPenalty", d.min_penetration_for_penalty),
                baumgarte_scale: f("m_BaumgarteScale", d.baumgarte_scale),
                baumgarte_time_of_impact_scale: f("m_BaumgarteTimeOfImpactScale", d.baumgarte_time_of_impact_scale),
                time_to_sleep: f("m_TimeToSleep", d.time_to_sleep),
                linear_sleep_tolerance: f("m_LinearSleepTolerance", d.linear_sleep_tolerance),
                angular_sleep_tolerance: f("m_AngularSleepTolerance", d.angular_sleep_tolerance),
            }
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASSET: &str = "\
%YAML 1.1
--- !u!19 &1
Physics2DSettings:
  m_Gravity: {x: 0, y: -16.076}
  m_VelocityIterations: 8
  m_PositionIterations: 3
  m_RaycastsHitTriggers: 1
  m_LayerCollisionMatrix: ffffffefffffffefffffffefffffffffffffffefffffffefffffffffffffffffffefeeacffdfddacffbfbbacff7f77acffeee0e5ffddd0a5ffbbb0a5ff7770a5ff0ee0e5ff0dd0a5ff0bb0a5ff0770a5ffeefea4ffddfda4ffbbfba4ff77f7a4fff00f84ff0000a4ffffffebff0f00acc8000080ffffffaeff100184ffffffff
";

    #[test]
    fn decodes_gravity_and_layer_matrix() {
        let s = parse_physics2d(ASSET).unwrap();
        assert_eq!(s.gravity, (0.0, -16.076));
        // Players (layer 8) collide with the Default layer (ground) and with other players.
        assert!(s.layers_collide(8, 0));
        assert!(s.layers_collide(0, 8));
        assert!(s.layers_collide(8, 9));
        // ...but not with their own team's projectiles (layer 12).
        assert!(!s.layers_collide(8, 12));
        assert!(!s.layers_collide(12, 8));
    }
}

/// `TagManager.asset` `m_SortingLayers`: the layers' `uniqueID`s (as the `int` a renderer's
/// `m_SortingLayerID` stores them) in draw order.
pub fn parse_sorting_layers(text: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut inside = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("m_SortingLayers:") {
            inside = true;
            continue;
        }
        if inside {
            if let Some(id) = trimmed.strip_prefix("uniqueID:") {
                if let Ok(v) = id.trim().parse::<i64>() {
                    // Serialized unsigned; renderers keep the same bits as a signed 32-bit int.
                    out.push(v as u32 as i32 as i64);
                }
            } else if trimmed.starts_with("m_") {
                // The next field of the tag manager.
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod sorting_tests {
    #[test]
    fn sorting_layers_in_order() {
        let text = "TagManager:\n  m_SortingLayers:\n  - name: In-Game\n    uniqueID: 4258750573\n    locked: 0\n  - name: Default\n    uniqueID: 0\n    locked: 0\n  m_Other: 1\n";
        let layers = super::parse_sorting_layers(text);
        assert_eq!(layers, vec![-36216723, 0]);
        assert_eq!(super::sorting_rank(&layers, 0), 1);
        assert_eq!(super::sorting_rank(&layers, -36216723), 0);
        assert_eq!(super::sorting_rank(&layers, 12345), 1, "unknown: Default");
    }
}

/// The draw-order rank of a renderer's `m_SortingLayerID` (unknown ids rank as "Default").
pub fn sorting_rank(layers: &[i64], id: i64) -> i32 {
    layers.iter().position(|&l| l == id).or_else(|| layers.iter().position(|&l| l == 0)).unwrap_or(0) as i32
}
