//! Humanoid poses: muscle values → bone transforms of the `Man` rig.
//!
//! Mecanim stores a humanoid clip as a root transform plus one "muscle" value per degree of
//! freedom; the engine turns that into bone rotations through the avatar's per-bone axes. This
//! module reproduces that pipeline for the player rig:
//!
//! * [`ClipPose`] samples a clip and applies its root-transform settings ("bake into pose" /
//!   extracted root motion),
//! * [`HumanPose::blend`] mixes poses the way the state machine cross-fades and blend trees do,
//! * [`CharacterRigModel::solve`] converts a pose into the world transform of every rig bone in
//!   the character's local space.
//!
//! What is recovered from data: bone axes (pre/post rotation, sign, limits), the rest pose, bone
//! masses, the hierarchy. What is reconstructed from knowledge of Mecanim and checked against the
//! data: the angle-from-muscle rule (`v ≥ 0 ? v·max : −v·min`), the `ZYRoll` composition and the
//! centre-of-mass placement of the body; those are marked `TODO(UNCERTAIN)` below.

use unity_compat::{Quat, Vec3};
use unity_data::anim::{Clip, ClipSettings, Curve};
use unity_data::humanoid::HumanAvatar;
use unity_data::rig::Rig;

/// `UnityEngine.HumanBodyBones` indices of the bones that carry muscles.
pub mod bone {
    pub const HIPS: usize = 0;
    pub const LEFT_UPPER_LEG: usize = 1;
    pub const RIGHT_UPPER_LEG: usize = 2;
    pub const LEFT_LOWER_LEG: usize = 3;
    pub const RIGHT_LOWER_LEG: usize = 4;
    pub const LEFT_FOOT: usize = 5;
    pub const RIGHT_FOOT: usize = 6;
    pub const SPINE: usize = 7;
    pub const CHEST: usize = 8;
    pub const NECK: usize = 9;
    pub const HEAD: usize = 10;
    pub const LEFT_SHOULDER: usize = 11;
    pub const RIGHT_SHOULDER: usize = 12;
    pub const LEFT_UPPER_ARM: usize = 13;
    pub const RIGHT_UPPER_ARM: usize = 14;
    pub const LEFT_LOWER_ARM: usize = 15;
    pub const RIGHT_LOWER_ARM: usize = 16;
    pub const LEFT_HAND: usize = 17;
    pub const RIGHT_HAND: usize = 18;
    /// Number of bones with mass and colliders in the avatar.
    pub const COUNT: usize = 19;
}

/// One muscle: the clip attribute that drives it and the bone axis it rotates.
///
/// Axis 0 is the twist about the bone, axes 1 and 2 are the two swings (the avatar's limits
/// confirm this: twist ranges are ±90° on arms and legs, the unused axes have zero range).
#[derive(Clone, Copy, Debug)]
pub struct Muscle {
    pub name: &'static str,
    pub bone: usize,
    pub axis: usize,
}

const fn m(name: &'static str, bone: usize, axis: usize) -> Muscle {
    Muscle { name, bone, axis }
}

pub const MUSCLE_COUNT: usize = 44;

/// Every muscle the `Man` avatar has axes for (fingers, eyes and jaw have no bones).
pub const MUSCLES: [Muscle; MUSCLE_COUNT] = [
    m("Spine Front-Back", bone::SPINE, 2),
    m("Spine Left-Right", bone::SPINE, 1),
    m("Spine Twist Left-Right", bone::SPINE, 0),
    m("Chest Front-Back", bone::CHEST, 2),
    m("Chest Left-Right", bone::CHEST, 1),
    m("Chest Twist Left-Right", bone::CHEST, 0),
    m("Neck Nod Down-Up", bone::NECK, 2),
    m("Neck Tilt Left-Right", bone::NECK, 1),
    m("Neck Turn Left-Right", bone::NECK, 0),
    m("Head Nod Down-Up", bone::HEAD, 2),
    m("Head Tilt Left-Right", bone::HEAD, 1),
    m("Head Turn Left-Right", bone::HEAD, 0),
    m("Left Upper Leg Front-Back", bone::LEFT_UPPER_LEG, 2),
    m("Left Upper Leg In-Out", bone::LEFT_UPPER_LEG, 1),
    m("Left Upper Leg Twist In-Out", bone::LEFT_UPPER_LEG, 0),
    m("Left Lower Leg Stretch", bone::LEFT_LOWER_LEG, 2),
    m("Left Lower Leg Twist In-Out", bone::LEFT_LOWER_LEG, 0),
    m("Left Foot Up-Down", bone::LEFT_FOOT, 2),
    m("Left Foot Twist In-Out", bone::LEFT_FOOT, 1),
    m("Right Upper Leg Front-Back", bone::RIGHT_UPPER_LEG, 2),
    m("Right Upper Leg In-Out", bone::RIGHT_UPPER_LEG, 1),
    m("Right Upper Leg Twist In-Out", bone::RIGHT_UPPER_LEG, 0),
    m("Right Lower Leg Stretch", bone::RIGHT_LOWER_LEG, 2),
    m("Right Lower Leg Twist In-Out", bone::RIGHT_LOWER_LEG, 0),
    m("Right Foot Up-Down", bone::RIGHT_FOOT, 2),
    m("Right Foot Twist In-Out", bone::RIGHT_FOOT, 1),
    m("Left Shoulder Down-Up", bone::LEFT_SHOULDER, 2),
    m("Left Shoulder Front-Back", bone::LEFT_SHOULDER, 1),
    m("Left Arm Down-Up", bone::LEFT_UPPER_ARM, 2),
    m("Left Arm Front-Back", bone::LEFT_UPPER_ARM, 1),
    m("Left Arm Twist In-Out", bone::LEFT_UPPER_ARM, 0),
    m("Left Forearm Stretch", bone::LEFT_LOWER_ARM, 2),
    m("Left Forearm Twist In-Out", bone::LEFT_LOWER_ARM, 0),
    m("Left Hand Down-Up", bone::LEFT_HAND, 2),
    m("Left Hand In-Out", bone::LEFT_HAND, 1),
    m("Right Shoulder Down-Up", bone::RIGHT_SHOULDER, 2),
    m("Right Shoulder Front-Back", bone::RIGHT_SHOULDER, 1),
    m("Right Arm Down-Up", bone::RIGHT_UPPER_ARM, 2),
    m("Right Arm Front-Back", bone::RIGHT_UPPER_ARM, 1),
    m("Right Arm Twist In-Out", bone::RIGHT_UPPER_ARM, 0),
    m("Right Forearm Stretch", bone::RIGHT_LOWER_ARM, 2),
    m("Right Forearm Twist In-Out", bone::RIGHT_LOWER_ARM, 0),
    m("Right Hand Down-Up", bone::RIGHT_HAND, 2),
    m("Right Hand In-Out", bone::RIGHT_HAND, 1),
];

/// Root transform and muscle values of a humanoid at one instant.
///
/// `root_t` is in avatar-normalized units (multiply by the avatar scale for metres) and places
/// the body's centre of mass; `root_q` orients the body.
#[derive(Clone, Debug, PartialEq)]
pub struct HumanPose {
    pub root_t: Vec3,
    pub root_q: Quat,
    pub muscles: [f32; MUSCLE_COUNT],
}

impl Default for HumanPose {
    fn default() -> HumanPose {
        HumanPose { root_t: Vec3::ZERO, root_q: Quat::IDENTITY, muscles: [0.0; MUSCLE_COUNT] }
    }
}

impl HumanPose {
    /// Weighted mix of poses (weights are normalized; zero weights are skipped).
    pub fn blend(poses: &[(&HumanPose, f32)]) -> HumanPose {
        let total: f32 = poses.iter().map(|(_, w)| *w).filter(|w| *w > 0.0).sum();
        let Some(&(first, _)) = poses.iter().find(|(_, w)| *w > 0.0) else { return HumanPose::default() };
        if total <= 0.0 {
            return first.clone();
        }
        let mut out = HumanPose { root_t: Vec3::ZERO, root_q: Quat::new(0.0, 0.0, 0.0, 0.0), muscles: [0.0; MUSCLE_COUNT] };
        for &(pose, weight) in poses.iter().filter(|(_, w)| *w > 0.0) {
            let w = weight / total;
            out.root_t = out.root_t + pose.root_t * w;
            for (o, v) in out.muscles.iter_mut().zip(pose.muscles.iter()) {
                *o += v * w;
            }
            // Accumulate quaternions on the hemisphere of the first pose (normalized lerp).
            let q = if first.root_q.dot(pose.root_q) < 0.0 {
                Quat::new(-pose.root_q.x, -pose.root_q.y, -pose.root_q.z, -pose.root_q.w)
            } else {
                pose.root_q
            };
            out.root_q = Quat::new(out.root_q.x + q.x * w, out.root_q.y + q.y * w, out.root_q.z + q.z * w, out.root_q.w + q.w * w);
        }
        out.root_q = out.root_q.normalized();
        out
    }
}

/// A clip's humanoid curves, compiled for fast sampling.
#[derive(Clone, Debug)]
pub struct ClipPose {
    muscles: Vec<Option<Curve>>,
    root_t: [Option<Curve>; 3],
    root_q: [Option<Curve>; 4],
    length: f32,
    looping: bool,
    settings: ClipSettings,
}

impl ClipPose {
    pub fn compile(clip: &Clip) -> ClipPose {
        let find = |name: &str| clip.humanoid_curves.get(name).cloned();
        let root_t = match &clip.root_motion {
            Some(rm) => [Some(rm.x.clone()), Some(rm.y.clone()), Some(rm.z.clone())],
            None => [None, None, None],
        };
        ClipPose {
            muscles: MUSCLES.iter().map(|mu| find(mu.name)).collect(),
            root_t,
            root_q: [find("RootQ.x"), find("RootQ.y"), find("RootQ.z"), find("RootQ.w")],
            length: clip.length,
            looping: clip.looping,
            settings: clip.settings,
        }
    }

    /// The pose at `normalized` time (unbounded for loops).
    ///
    /// Root transform handling follows the clip settings: components that are not "baked into
    /// the pose" are root motion (consumed by the controller through the animator's
    /// `delta_position`), so the pose keeps their value at the start of the clip.
    /// TODO(UNCERTAIN): Unity's reference for "Based Upon: Original" is assumed to be the clip
    /// start; the orientation offset is assumed to add to the clip's heading.
    pub fn sample(&self, normalized: f32) -> HumanPose {
        let n = if self.looping { normalized - normalized.floor() } else { normalized.clamp(0.0, 1.0) };
        let time = n * self.length;
        let at = |curve: &Option<Curve>, t: f32, default: f32| curve.as_ref().map_or(default, |c| c.evaluate(t));
        let mut pose = HumanPose::default();
        for (value, curve) in pose.muscles.iter_mut().zip(self.muscles.iter()) {
            *value = at(curve, time, 0.0);
        }
        let t_now = Vec3::new(at(&self.root_t[0], time, 0.0), at(&self.root_t[1], time, 0.0), at(&self.root_t[2], time, 0.0));
        let t_start = Vec3::new(at(&self.root_t[0], 0.0, 0.0), at(&self.root_t[1], 0.0, 0.0), at(&self.root_t[2], 0.0, 0.0));
        pose.root_t = Vec3::new(
            if self.settings.bake_position_xz { t_now.x } else { t_start.x },
            if self.settings.bake_position_y { t_now.y } else { t_start.y },
            if self.settings.bake_position_xz { t_now.z } else { t_start.z },
        );
        let q = Quat::new(at(&self.root_q[0], time, 0.0), at(&self.root_q[1], time, 0.0), at(&self.root_q[2], time, 0.0), at(&self.root_q[3], time, 1.0)).normalized();
        // Swing-twist split about the vertical axis: `q = heading * rest`.
        let heading = Quat::new(0.0, q.y, 0.0, q.w).normalized();
        pose.root_q = if self.settings.bake_orientation {
            Quat::euler_y_degrees(self.settings.orientation_offset_y) * q
        } else {
            heading.conjugate() * q
        };
        pose
    }
}

/// Immutable solver data: the humanoid avatar bound to the rig of `Man.prefab`.
#[derive(Clone, Debug)]
pub struct CharacterRigModel {
    avatar: HumanAvatar,
    rig: Rig,
    /// Human skeleton node → rig bone.
    node_bone: Vec<Option<usize>>,
    /// Rest transform of the hips in the body (centre-of-mass) frame.
    hips_in_root: (Vec3, Quat),
    /// Centre of mass of the rest pose in the body frame, subtracted so that the rest pose
    /// reproduces the avatar's `m_RootX` exactly.
    rest_com: Vec3,
    /// Muscle → (human node, axis).
    muscle_slots: Vec<(usize, usize)>,
}

/// World transform (in the character's local space) of every rig bone.
#[derive(Clone, Debug)]
pub struct RigPose {
    pub world: Vec<(Vec3, Quat)>,
}

impl RigPose {
    pub fn position(&self, bone: usize) -> Vec3 {
        self.world[bone].0
    }

    pub fn rotation(&self, bone: usize) -> Quat {
        self.world[bone].1
    }
}

fn axis_rotation(angles: [f32; 3]) -> Quat {
    // `ZYRoll`: swing about z, then y, then roll (twist) about the bone's x axis.
    // TODO(UNCERTAIN): the composition order of the three axis rotations is recalled from
    // Mecanim's `ZYRoll` helper, not recovered from data. Twist (x) is invisible to the
    // stick-figure lines because every child lies on the bone axis.
    Quat::about_z(angles[2]) * Quat::about_y(angles[1]) * Quat::about_x(angles[0])
}

impl CharacterRigModel {
    pub fn new(avatar: HumanAvatar, rig: Rig) -> Option<CharacterRigModel> {
        let node_bone = avatar.bind_to_rig(&rig);
        let hips_node = avatar.bone_node.get(bone::HIPS).copied().flatten()?;
        let world = rest_world(&avatar);
        let root_q = Quat::from_array(avatar.root_q);
        let inv_root = root_q.conjugate();
        let root_t = vec3(avatar.root_t);
        let hips_in_root = (inv_root * (world[hips_node].0 - root_t), inv_root * world[hips_node].1);
        let muscle_slots = MUSCLES.iter().map(|mu| Some((avatar.bone_node.get(mu.bone).copied().flatten()?, mu.axis))).collect::<Option<Vec<_>>>()?;
        let mut model = CharacterRigModel { avatar, rig, node_bone, hips_in_root, rest_com: Vec3::ZERO, muscle_slots };
        let rest_rotations: Vec<Quat> = model.avatar.nodes.iter().map(|n| Quat::from_array(n.rest_q)).collect();
        model.rest_com = model.centre_of_mass(&rest_rotations).0;
        Some(model)
    }

    pub fn rig(&self) -> &Rig {
        &self.rig
    }

    pub fn avatar(&self) -> &HumanAvatar {
        &self.avatar
    }

    /// The rig bone a `HumanBodyBones` index (see [`bone`]) drives.
    pub fn bone_of(&self, human_bone: usize) -> Option<usize> {
        let node = self.avatar.bone_node.get(human_bone).copied().flatten()?;
        self.node_bone.get(node).copied().flatten()
    }

    /// Bone rotation of every human skeleton node for the given muscle values.
    fn local_rotations(&self, muscles: &[f32; MUSCLE_COUNT]) -> Vec<Quat> {
        let nodes = &self.avatar.nodes;
        let mut angles = vec![[0.0f32; 3]; nodes.len()];
        for (&(node, axis), &v) in self.muscle_slots.iter().zip(muscles.iter()) {
            let Some(axes) = nodes[node].axes.map(|a| &self.avatar.axes[a]) else { continue };
            // TODO(UNCERTAIN): the muscle → angle rule (`v·max` above zero, `−v·min` below) is the
            // standard Mecanim mapping; the avatar's limits are consistent with it.
            let angle = if v >= 0.0 { v * axes.max[axis] } else { -v * axes.min[axis] };
            angles[node][axis] = angle * axes.sgn[axis];
        }
        nodes
            .iter()
            .enumerate()
            .map(|(i, node)| match node.axes {
                Some(a) => {
                    let axes = &self.avatar.axes[a];
                    Quat::from_array(axes.pre) * axis_rotation(angles[i]) * Quat::from_array(axes.post).conjugate()
                }
                None => Quat::from_array(node.rest_q),
            })
            .collect()
    }

    /// Human skeleton nodes in the body frame (hips at their rest transform) and the pose's
    /// centre of mass, relative to the rest pose's.
    fn centre_of_mass(&self, rotations: &[Quat]) -> (Vec3, Vec<(Vec3, Quat)>) {
        let nodes = &self.avatar.nodes;
        let mut world = vec![(Vec3::ZERO, Quat::IDENTITY); nodes.len()];
        for (i, node) in nodes.iter().enumerate() {
            let Some(parent) = node.parent else { continue };
            world[i] = if parent == 0 {
                self.hips_in_root
            } else {
                let (pp, pq) = world[parent];
                (pp + pq * vec3(node.rest_t), pq * rotations[i])
            };
        }
        // TODO(UNCERTAIN): bone masses sit on the bone's joint. The avatar's collider centres
        // (half a bone away) put the rest centre of mass 6–15 cm from `m_RootX`, the joints only
        // 8 mm, so joints are used and the rest value is subtracted ([`Self::rest_com`]).
        let mut sum = Vec3::ZERO;
        let mut mass = 0.0;
        for (bone, node) in self.avatar.bone_node.iter().enumerate().take(bone::COUNT) {
            let (Some(node), Some(&m)) = (node, self.avatar.bone_mass.get(bone)) else { continue };
            sum = sum + world[*node].0 * m;
            mass += m;
        }
        let com = if mass > 0.0 { sum * (1.0 / mass) } else { Vec3::ZERO };
        (com - self.rest_com, world)
    }

    /// World transforms of every rig bone in the character's local space (the space of the
    /// object that carries the `Animator`, before the facing rotation is applied).
    pub fn solve(&self, pose: &HumanPose) -> RigPose {
        let rotations = self.local_rotations(&pose.muscles);
        self.solve_with(pose.root_t, pose.root_q, &rotations)
    }

    /// [`Self::solve`] for explicit human-skeleton node rotations.
    fn solve_with(&self, root_t: Vec3, root_q: Quat, rotations: &[Quat]) -> RigPose {
        let (com, _) = self.centre_of_mass(rotations);
        let scale = self.avatar.scale;
        let body_t = root_t * scale;
        // Every point `p` of the body frame lands at `body_t + root_q * (p - com)`.
        let hips_world = (body_t + root_q * (self.hips_in_root.0 - com), root_q * self.hips_in_root.1);

        let bones = &self.rig.bones;
        let mut local_rotation: Vec<Quat> = bones.iter().map(|b| Quat::from_array(b.rotation)).collect();
        for (node, bone) in self.node_bone.iter().enumerate() {
            if let Some(bone) = bone {
                if self.avatar.nodes[node].axes.is_some() {
                    local_rotation[*bone] = rotations[node];
                }
            }
        }
        let hips_bone = self.node_bone[self.avatar.bone_node[bone::HIPS].unwrap_or(1)];
        let mut world = vec![(Vec3::ZERO, Quat::IDENTITY); bones.len()];
        for (i, b) in bones.iter().enumerate() {
            let Some(parent) = b.parent else { continue };
            world[i] = if Some(i) == hips_bone {
                hips_world
            } else {
                let (pp, pq) = world[parent];
                (pp + pq * vec3(b.position), pq * local_rotation[i])
            };
        }
        RigPose { world }
    }
}

fn vec3(v: [f32; 3]) -> Vec3 {
    Vec3::new(v[0], v[1], v[2])
}

/// Rest-pose world transforms of the human skeleton (node 0 is the world root).
pub(crate) fn rest_world(avatar: &HumanAvatar) -> Vec<(Vec3, Quat)> {
    let mut world = vec![(Vec3::ZERO, Quat::IDENTITY); avatar.nodes.len()];
    for (i, node) in avatar.nodes.iter().enumerate() {
        match node.parent {
            None => world[i] = (vec3(node.rest_t), Quat::from_array(node.rest_q)),
            Some(p) => {
                let (pp, pq) = world[p];
                world[i] = (pp + pq * vec3(node.rest_t), pq * Quat::from_array(node.rest_q));
            }
        }
    }
    world
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use unity_data::humanoid::parse_avatar;
    use unity_data::rig::load_man_rig;

    fn assets() -> Option<PathBuf> {
        let root = std::env::var_os("STICK_EXPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
        let assets = root.join("Assets");
        assets.join("Game").is_dir().then_some(assets)
    }

    fn model() -> Option<CharacterRigModel> {
        let assets = assets()?;
        let rig = load_man_rig(&assets).unwrap();
        let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap()).unwrap();
        Some(CharacterRigModel::new(avatar, rig).unwrap())
    }

    fn clip_pose(assets: &std::path::Path, name: &str) -> ClipPose {
        let text = std::fs::read_to_string(assets.join(format!("AnimationClip/{name}.anim"))).unwrap();
        ClipPose::compile(&unity_data::anim::parse_clip(&text).unwrap())
    }

    fn close(a: Vec3, b: Vec3, tol: f32) -> bool {
        (a - b).magnitude() <= tol
    }

    #[test]
    fn the_muscle_table_matches_the_avatars_axes() {
        let Some(model) = model() else { return };
        let avatar = model.avatar();
        for mu in MUSCLES {
            let node = avatar.bone_node[mu.bone].unwrap_or_else(|| panic!("{} has no bone", mu.name));
            let axes = &avatar.axes[avatar.nodes[node].axes.unwrap_or_else(|| panic!("{} has no axes", mu.name))];
            // A muscle always drives an axis with a real range.
            assert!(axes.max[mu.axis] - axes.min[mu.axis] > 0.1, "{} drives a dead axis", mu.name);
        }
    }

    #[test]
    fn rest_rotations_reproduce_the_prefab_pose_and_the_avatars_centre_of_mass() {
        let Some(model) = model() else { return };
        let avatar = model.avatar();
        let rig = model.rig();
        let rest: Vec<Quat> = avatar.nodes.iter().map(|n| Quat::from_array(n.rest_q)).collect();
        let (com, _) = model.centre_of_mass(&rest);
        assert!(com.magnitude() < 1e-5, "calibrated rest centre of mass is zero: {com:?}");
        // The uncalibrated rest centre of mass is within a centimetre of the avatar's own root.
        assert!(model.rest_com.magnitude() < 0.015, "{:?}", model.rest_com);

        // Body exactly at m_RootX with rest rotations: every bone must sit where the prefab's
        // transform hierarchy puts it (validates the node ↔ bone mapping and the composition).
        let solved = model.solve_with(vec3(avatar.root_t) * (1.0 / avatar.scale), Quat::from_array(avatar.root_q), &rest);
        let mut prefab: Vec<(Vec3, Quat)> = vec![(Vec3::ZERO, Quat::IDENTITY); rig.bones.len()];
        for (i, b) in rig.bones.iter().enumerate() {
            if let Some(p) = b.parent {
                let (pp, pq) = prefab[p];
                prefab[i] = (pp + pq * vec3(b.position), pq * Quat::from_array(b.rotation));
            }
        }
        for (i, b) in rig.bones.iter().enumerate() {
            assert!(close(solved.position(i), prefab[i].0, 2e-3), "{}: solved {:?} vs prefab {:?}", b.path, solved.position(i), prefab[i].0);
        }
    }

    #[test]
    fn human_skeleton_rest_pose_matches_the_prefab_hierarchy() {
        let Some(model) = model() else { return };
        let avatar = model.avatar();
        let rig = model.rig();
        let mut prefab: Vec<(Vec3, Quat)> = vec![(Vec3::ZERO, Quat::IDENTITY); rig.bones.len()];
        for (i, b) in rig.bones.iter().enumerate() {
            if let Some(p) = b.parent {
                let (pp, pq) = prefab[p];
                prefab[i] = (pp + pq * vec3(b.position), pq * Quat::from_array(b.rotation));
            }
        }
        let human = rest_world(avatar);
        for (node, bone) in model.node_bone.iter().enumerate().skip(1) {
            let bone = bone.unwrap();
            assert!(close(prefab[bone].0, human[node].0, 2e-3), "{}: prefab {:?} vs human skeleton {:?}", rig.bones[bone].path, prefab[bone].0, human[node].0);
        }
    }

    #[test]
    fn running_pose_leans_forward_and_has_feet_near_the_ground_plane() {
        let Some(model) = model() else { return };
        let assets = assets().unwrap();
        let run = clip_pose(&assets, "Run");
        let rig = model.rig();
        let hips = rig.bone_by_path("Armature/Hips").unwrap();
        let head = rig.bone_by_path("Armature/Hips/Waist/Spine1/Spine2/Neck/Head").unwrap();
        let left = rig.bone_by_path("Armature/Hips/LeftHip/LeftKnee/LeftAnkle").unwrap();
        let right = rig.bone_by_path("Armature/Hips/RightHip/RightKnee/RightAnkle").unwrap();
        let mut lowest_overall = f32::MAX;
        for step in 0..32 {
            let pose = run.sample(step as f32 / 32.0);
            let solved = model.solve(&pose);
            let (h, hd) = (solved.position(hips), solved.position(head));
            // Upright torso, head well above the hips, a little ahead (+z is the facing direction).
            assert!(hd.y - h.y > 0.9, "step {step}: head {hd:?} hips {h:?}");
            assert!(hd.z - h.z > -0.1, "step {step}: not leaning backwards");
            let feet = solved.position(left).y.min(solved.position(right).y);
            lowest_overall = lowest_overall.min(feet);
            // The collider bottom is the origin; ankles stay within a few tenths of it.
            assert!((0.0..0.6).contains(&feet), "step {step}: lowest ankle {feet}");
        }
        assert!(lowest_overall < 0.35);
    }

    #[test]
    fn blending_two_poses_halfway_averages_muscles_and_keeps_a_unit_root_rotation() {
        let mut a = HumanPose::default();
        let mut b = HumanPose::default();
        a.muscles[0] = 1.0;
        b.muscles[0] = -1.0;
        b.root_q = Quat::about_y(1.0);
        b.root_t = Vec3::new(2.0, 0.0, 0.0);
        let mix = HumanPose::blend(&[(&a, 0.5), (&b, 0.5)]);
        assert!(mix.muscles[0].abs() < 1e-6);
        assert!((mix.root_t.x - 1.0).abs() < 1e-6);
        assert!((mix.root_q.dot(mix.root_q) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn falling_pose_keeps_start_height_because_y_is_not_baked() {
        let Some(_) = model() else { return };
        let assets = assets().unwrap();
        let falling = clip_pose(&assets, "Falling");
        let a = falling.sample(0.0);
        let b = falling.sample(0.5);
        assert_eq!(a.root_t.y, b.root_t.y);
        assert_eq!(a.root_t.x, b.root_t.x);
    }
}
