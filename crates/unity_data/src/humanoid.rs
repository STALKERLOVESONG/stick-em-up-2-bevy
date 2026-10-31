//! The humanoid `Avatar` asset: the muscle axes and rest pose of the human skeleton.


use crate::yaml::{self, Value};

/// Rotation axes of one human bone (`math::Axes`): the muscle angles are applied between
/// `pre` and `post`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Axes {
    pub pre: [f32; 4],
    pub post: [f32; 4],
    pub sgn: [f32; 3],
    pub min: [f32; 3],
    pub max: [f32; 3],
    pub length: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HumanNode {
    pub parent: Option<usize>,
    pub axes: Option<usize>,
    /// Rest ("T-pose") local translation and rotation `(x, y, z, w)` in the human skeleton.
    pub rest_t: [f32; 3],
    pub rest_q: [f32; 4],
}

/// Everything needed to turn muscle values into bone rotations.
#[derive(Clone, Debug)]
pub struct HumanAvatar {
    /// `m_Human.m_Scale`: root translation curves are in units of this scale.
    pub scale: f32,
    /// `m_Human.m_ArmStretch`: how much longer (as a fraction) IK may make an arm to reach a goal.
    pub arm_stretch: f32,
    /// Center-of-mass transform of the rest pose (`m_RootX`).
    pub root_t: [f32; 3],
    pub root_q: [f32; 4],
    /// The human skeleton; node 0 is its root and node 1 the hips.
    pub nodes: Vec<HumanNode>,
    pub axes: Vec<Axes>,
    /// `HumanBodyBones` index → human skeleton node.
    pub bone_node: Vec<Option<usize>>,
    /// `HumanBodyBones` index → mass fraction.
    pub bone_mass: Vec<f32>,
    /// `HumanBodyBones` index → centre of the bone's collider (`m_ColliderArray[..].m_X.t`),
    /// in the bone's own axes. The bone's centre of mass sits there.
    pub bone_collider_offset: Vec<[f32; 3]>,
    /// Human skeleton node → node of the avatar's own (model) skeleton.
    pub node_avatar: Vec<usize>,
    /// Avatar skeleton: parent index per node and the CRC-32 of the bone's name.
    pub avatar_parent: Vec<Option<usize>>,
    pub avatar_name_hash: Vec<u32>,
}

impl HumanAvatar {
    /// Bind the avatar's skeleton to a model hierarchy by walking down from the root and
    /// matching each node's name hash against the model bone's children. Returns the rig bone of
    /// every human skeleton node (`None` for the human root).
    pub fn bind_to_rig(&self, rig: &crate::rig::Rig) -> Vec<Option<usize>> {
        let mut bone_of_avatar: Vec<Option<usize>> = vec![None; self.avatar_parent.len()];
        for i in 0..self.avatar_parent.len() {
            bone_of_avatar[i] = match self.avatar_parent[i] {
                None => Some(rig.root),
                Some(p) => bone_of_avatar[p].and_then(|pb| {
                    rig.bones[pb].children.iter().copied().find(|&c| crate::anim::crc32(&rig.bones[c].name) == self.avatar_name_hash[i])
                }),
            };
        }
        self.node_avatar.iter().enumerate().map(|(h, &a)| if h == 0 { None } else { bone_of_avatar.get(a).copied().flatten() }).collect()
    }
}

fn hex_u32s(text: &str) -> Vec<u32> {
    let text = text.trim();
    (0..text.len() / 8)
        .filter_map(|i| {
            let word = &text[i * 8..i * 8 + 8];
            let bytes = (0..4).map(|b| u8::from_str_radix(&word[b * 2..b * 2 + 2], 16)).collect::<Result<Vec<u8>, _>>().ok()?;
            Some(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
        })
        .collect()
}

fn quat_of(v: Option<&Value>) -> Option<[f32; 4]> {
    let (x, y, z, w) = v?.as_quat()?;
    Some([x, y, z, w])
}

fn vec3_of(v: Option<&Value>) -> Option<[f32; 3]> {
    let (x, y, z) = v?.as_vec3()?;
    Some([x, y, z])
}

/// Parse the `ManAvatar.asset` text.
pub fn parse_avatar(text: &str) -> Option<HumanAvatar> {
    let docs = yaml::parse_documents(text);
    let avatar = docs.iter().find(|d| d.type_name == "Avatar")?;
    let a = avatar.body.get("m_Avatar")?;
    let human = a.get("m_Human")?.get("data")?;

    let skeleton = human.get("m_Skeleton")?.get("data")?;
    let pose = human.get("m_SkeletonPose")?.get("data")?.get("m_X")?.as_seq();
    let mut nodes = Vec::new();
    for (i, node) in skeleton.get("m_Node")?.as_seq().iter().enumerate() {
        let parent = node.i64_of("m_ParentId")?;
        let axes = node.i64_of("m_AxesId")?;
        let x = pose.get(i)?;
        nodes.push(HumanNode {
            parent: (parent >= 0).then_some(parent as usize),
            axes: (axes >= 0).then_some(axes as usize),
            rest_t: vec3_of(x.get("t"))?,
            rest_q: quat_of(x.get("q"))?,
        });
    }
    let mut axes = Vec::new();
    for ax in skeleton.get("m_AxesArray")?.as_seq() {
        let limit = ax.get("m_Limit")?;
        axes.push(Axes {
            pre: quat_of(ax.get("m_PreQ"))?,
            post: quat_of(ax.get("m_PostQ"))?,
            sgn: vec3_of(ax.get("m_Sgn"))?,
            min: vec3_of(limit.get("m_Min"))?,
            max: vec3_of(limit.get("m_Max"))?,
            length: ax.f32_of("m_Length")?,
        });
    }

    let bone_node = hex_u32s(human.str_of("m_HumanBoneIndex")?).into_iter().map(|i| (i != u32::MAX).then_some(i as usize)).collect();
    let bone_mass = human.get("m_HumanBoneMass")?.as_seq().iter().filter_map(Value::as_f32).collect();

    let colliders: Vec<[f32; 3]> =
        human.get("m_ColliderArray")?.as_seq().iter().filter_map(|c| vec3_of(c.get("m_X")?.get("t"))).collect();
    let bone_collider_offset = hex_u32s(human.str_of("m_ColliderIndex")?)
        .into_iter()
        .map(|i| colliders.get(i as usize).copied().unwrap_or([0.0; 3]))
        .collect();

    // The avatar's own skeleton (the model's bones, by name hash) and the human → avatar map.
    let node_avatar: Vec<usize> = hex_u32s(a.str_of("m_HumanSkeletonIndexArray")?).into_iter().map(|i| i as usize).collect();
    let avatar_name_hash = hex_u32s(a.str_of("m_SkeletonNameIDArray")?);
    let avatar_parent = a
        .get("m_AvatarSkeleton")?
        .get("data")?
        .get("m_Node")?
        .as_seq()
        .iter()
        .map(|n| n.i64_of("m_ParentId").map(|p| (p >= 0).then_some(p as usize)))
        .collect::<Option<Vec<_>>>()?;

    Some(HumanAvatar {
        scale: human.f32_of("m_Scale")?,
        arm_stretch: human.f32_of("m_ArmStretch").unwrap_or(0.0),
        root_t: vec3_of(human.get("m_RootX")?.get("t"))?,
        root_q: quat_of(human.get("m_RootX")?.get("q"))?,
        nodes,
        axes,
        bone_node,
        bone_mass,
        bone_collider_offset,
        node_avatar,
        avatar_parent,
        avatar_name_hash,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_words_are_little_endian() {
        assert_eq!(hex_u32s("0100000010000000ffffffff"), vec![1, 16, u32::MAX]);
    }
}
