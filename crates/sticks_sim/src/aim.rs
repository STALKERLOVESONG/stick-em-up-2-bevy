//! Aiming the character's weapon: the weapon bone's `LookAt`, hand IK onto the weapon's grips
//! and the head's look-at, as `CharacterControl2D.CorrectAndSetAim` and `OnAnimatorIK` ask for.
//!
//! TODO(UNCERTAIN): Mecanim's IK solver is not recoverable from the data. The arms use an
//! analytic two-bone solution that keeps the animated elbow's side of the shoulder→hand line;
//! the head turns by the shortest rotation that takes its forward axis to the target, limited to
//! 90° (`SetLookAtWeight(1)` leaves `clampWeight` at its 0.5 default, which Unity documents as
//! half of the possible range). Spine and eyes are not involved (`bodyWeight` and `eyesWeight`
//! default to 0).

use unity_compat::{Quat, Vec2, Vec3};

use crate::character::DEFAULT_Z;
use crate::pose::{CharacterRigModel, HumanPose, RigPose, bone};
use crate::visual::BodyPlacement;
use crate::weapons::weapon_rotation;

/// Which weapon is up and where it points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AimedWeapon {
    /// Index into `Rig::weapons`.
    pub weapon: usize,
    /// The corrected aim vector (see [`crate::weapons::corrected_aim`]), in world space.
    pub aim: Vec2,
    pub face_right: bool,
}

/// `SetLookAtWeight(1)` with the default clamp weight of 0.5: up to 90° from forward.
const LOOK_AT_LIMIT: f32 = std::f32::consts::FRAC_PI_2;

/// Rigidly turn a bone and everything below it about `pivot`.
fn rotate_subtree(model: &CharacterRigModel, pose: &mut RigPose, root: usize, pivot: Vec3, q: Quat) {
    let bones = &model.rig().bones;
    let mut in_subtree = vec![false; bones.len()];
    in_subtree[root] = true;
    // Parents precede children in the rig's order.
    for i in root..bones.len() {
        if let Some(p) = bones[i].parent {
            if in_subtree[p] {
                in_subtree[i] = true;
            }
        }
        if in_subtree[i] {
            let (position, rotation) = pose.world[i];
            pose.world[i] = (pivot + q * (position - pivot), q * rotation);
        }
    }
}

/// Move a bone and everything below it.
fn translate_subtree(model: &CharacterRigModel, pose: &mut RigPose, root: usize, delta: Vec3) {
    let bones = &model.rig().bones;
    let mut in_subtree = vec![false; bones.len()];
    in_subtree[root] = true;
    for i in root..bones.len() {
        if let Some(p) = bones[i].parent {
            if in_subtree[p] {
                in_subtree[i] = true;
            }
        }
        if in_subtree[i] {
            pose.world[i].0 = pose.world[i].0 + delta;
        }
    }
}

/// Elbow and wrist positions that put the wrist on `goal` (or as close as the arm reaches),
/// keeping the elbow on the side of the shoulder→goal line where it already is.
fn two_bone(shoulder: Vec3, elbow: Vec3, wrist: Vec3, goal: Vec3) -> (Vec3, Vec3) {
    let upper = (elbow - shoulder).magnitude();
    let lower = (wrist - elbow).magnitude();
    let to_goal = goal - shoulder;
    let reach = to_goal.magnitude();
    if reach < 1e-6 || upper < 1e-6 || lower < 1e-6 {
        return (elbow, wrist);
    }
    let direction = to_goal * (1.0 / reach);
    let distance = reach.clamp((upper - lower).abs() + 1e-4, upper + lower - 1e-4);
    let along = (upper * upper - lower * lower + distance * distance) / (2.0 * distance);
    let height = (upper * upper - along * along).max(0.0).sqrt();
    let bent = (elbow - shoulder) - direction * (elbow - shoulder).dot(direction);
    let side = if bent.magnitude() > 1e-6 { bent.normalized() } else { direction.cross(Vec3::FORWARD).normalized() };
    (shoulder + direction * along + side * height, shoulder + direction * distance)
}

impl CharacterRigModel {
    /// [`Self::solve`] plus the aiming pass. `facing` is the world rotation of the character
    /// object (the pose lives in its local space); the result is again in that local space.
    pub fn solve_aimed(&self, pose: &HumanPose, facing: Quat, aimed: &AimedWeapon) -> RigPose {
        self.solve_aimed_acting(pose, facing, aimed, None)
    }

    /// [`Self::solve_aimed`] with the weapon's firing animation at `time` seconds (`action`):
    /// after the aim turns the weapon, the clip moves and turns the transforms below the weapon
    /// pivot (the right hand's grip, the sword's blade), and the hands then reach for the grips.
    pub fn solve_aimed_acting(&self, pose: &HumanPose, facing: Quat, aimed: &AimedWeapon, action: Option<(&unity_data::anim::TransformClip, f32)>) -> RigPose {
        let mut solved = self.solve(pose);
        let rig = self.rig();
        let Some(weapon) = rig.weapons.get(aimed.weapon) else { return solved };
        let to_local = facing.conjugate();
        let new_rotation = to_local * weapon_rotation(aimed.aim, aimed.face_right);

        // `weaponPivot.LookAt(pivot + aim, up)`: the weapon bone's world rotation is replaced.
        let (weapon_position, old_rotation) = solved.world[weapon.bone];
        rotate_subtree(self, &mut solved, weapon.bone, weapon_position, new_rotation * old_rotation.conjugate());
        if let Some((clip, time)) = action {
            self.apply_transform_clip(&mut solved, weapon.bone, clip, time);
        }

        // `OnAnimatorIK`: look at `correctedAim + weaponPivot.position`, hands onto the grips.
        let target = weapon_position + to_local * Vec3::new(aimed.aim.x, aimed.aim.y, 0.0);
        self.look_at_with_head(&mut solved, target);
        for (upper, lower, hand, grip) in [
            (bone::LEFT_UPPER_ARM, bone::LEFT_LOWER_ARM, bone::LEFT_HAND, weapon.left_hand),
            (bone::RIGHT_UPPER_ARM, bone::RIGHT_LOWER_ARM, bone::RIGHT_HAND, weapon.right_hand),
        ] {
            let (Some(grip), Some(upper), Some(lower), Some(hand)) = (grip, self.bone_of(upper), self.bone_of(lower), self.bone_of(hand)) else {
                continue;
            };
            let goal = solved.position(grip);
            self.reach_with_arm(&mut solved, upper, lower, hand, goal);
        }
        solved
    }

    /// Set the local transforms a [`unity_data::anim::TransformClip`] animates below `root` (the
    /// animated object) to their values at `time`, carrying each one's subtree along.
    fn apply_transform_clip(&self, solved: &mut RigPose, root: usize, clip: &unity_data::anim::TransformClip, time: f32) {
        let bones = &self.rig().bones;
        let base = &bones[root].path;
        let find = |path: &str| {
            let full = if base.is_empty() { path.to_owned() } else { format!("{base}/{path}") };
            bones.iter().position(|b| b.path == full)
        };
        let mut overrides: Vec<(usize, Option<Vec3>, Option<Quat>)> = Vec::new();
        for (path, [x, y, z]) in &clip.positions {
            let Some(bone) = find(path) else { continue };
            let p = Vec3::new(x.evaluate(time), y.evaluate(time), z.evaluate(time));
            match overrides.iter_mut().find(|o| o.0 == bone) {
                Some(o) => o.1 = Some(p),
                None => overrides.push((bone, Some(p), None)),
            }
        }
        for (path, [x, y, z, w]) in &clip.rotations {
            let Some(bone) = find(path) else { continue };
            let q = Quat::new(x.evaluate(time), y.evaluate(time), z.evaluate(time), w.evaluate(time)).normalized();
            match overrides.iter_mut().find(|o| o.0 == bone) {
                Some(o) => o.2 = Some(q),
                None => overrides.push((bone, None, Some(q))),
            }
        }
        // Parents precede children in the rig's order.
        overrides.sort_by_key(|o| o.0);
        for (bone, local_position, local_rotation) in overrides {
            let Some(parent) = bones[bone].parent else { continue };
            let (parent_position, parent_rotation) = solved.world[parent];
            let (old_position, old_rotation) = solved.world[bone];
            let new_position = local_position.map_or(old_position, |p| parent_position + parent_rotation * p);
            let new_rotation = local_rotation.map_or(old_rotation, |q| parent_rotation * q);
            let turn = new_rotation * old_rotation.conjugate();
            // The bone itself, then everything below it moves rigidly with it.
            translate_subtree(self, solved, bone, new_position - old_position);
            rotate_subtree(self, solved, bone, new_position, turn);
        }
    }

    fn look_at_with_head(&self, solved: &mut RigPose, target: Vec3) {
        let Some(head) = self.bone_of(bone::HEAD) else { return };
        let (position, rotation) = solved.world[head];
        let current = rotation * self.head_forward();
        let wanted = target - position;
        if wanted.magnitude() < 1e-4 {
            return;
        }
        let turn = Quat::from_to(current, wanted);
        // Limit the turn to the clamp range.
        let w = turn.w.clamp(-1.0, 1.0).abs();
        let angle = 2.0 * w.acos();
        let turn = if angle > LOOK_AT_LIMIT {
            let k = LOOK_AT_LIMIT / angle;
            Quat::nlerp(Quat::IDENTITY, turn, k)
        } else {
            turn
        };
        rotate_subtree(self, solved, head, position, turn);
    }

    fn reach_with_arm(&self, solved: &mut RigPose, upper: usize, lower: usize, hand: usize, goal: Vec3) {
        let (s, e, w) = (solved.position(upper), solved.position(lower), solved.position(hand));
        let (new_elbow, new_wrist) = two_bone(s, e, w, goal);
        // Swing the upper arm so the elbow lands on its new spot, then the forearm for the wrist.
        rotate_subtree(self, solved, upper, s, Quat::from_to(e - s, new_elbow - s));
        let wrist_now = solved.position(hand);
        rotate_subtree(self, solved, lower, new_elbow, Quat::from_to(wrist_now - new_elbow, new_wrist - new_elbow));
        // Mecanim's IK may stretch an arm by `m_ArmStretch` (5 % on the `Man`) to reach a goal
        // just beyond it: both bones lengthen along their (now nearly straight) directions.
        // TODO(UNCERTAIN): Mecanim's stretch curve is not recoverable; the length grows only by
        // as much as the goal is out of reach, capped at the avatar's fraction.
        let length = (new_elbow - s).magnitude() + (new_wrist - new_elbow).magnitude();
        let reach = (goal - s).magnitude();
        if length > 1e-6 && reach > length {
            let k = (reach / length).min(1.0 + self.avatar().arm_stretch);
            let elbow_shift = (new_elbow - s) * (k - 1.0);
            translate_subtree(self, solved, lower, elbow_shift);
            let wrist_shift = (new_wrist - new_elbow) * (k - 1.0);
            translate_subtree(self, solved, hand, wrist_shift);
        }
    }

    /// The head's look direction in its own frame. Mecanim measures look-at against the
    /// humanoid T-pose, in which the character looks along the avatar's +z: the axis is the
    /// avatar's +z expressed in the head's rest (skeleton pose) frame.
    ///
    /// (An earlier version took +z of the *prefab's* bone chain; in the animated pose that axis
    /// points into the screen, so looking up rolled the head the wrong way.)
    fn head_forward(&self) -> Vec3 {
        let Some(node) = self.avatar().bone_node.get(bone::HEAD).copied().flatten() else { return Vec3::FORWARD };
        let rest = crate::pose::rest_world(self.avatar());
        rest[node].1.conjugate() * Vec3::FORWARD
    }
}

/// A character's rig for one frame: the solved bones (in the character's local space) and where
/// that space sits in the world.
#[derive(Clone, Debug)]
pub struct PosedCharacter {
    pub solved: RigPose,
    pub placement: BodyPlacement,
}

impl PosedCharacter {
    /// World position of a rig bone.
    pub fn world_position(&self, bone: usize) -> Vec3 {
        self.placement.position + self.placement.rotation * self.solved.position(bone)
    }
}

impl CharacterRigModel {
    /// The character as it stands this frame: the animated pose, turned to face `face_right`,
    /// with the equipped weapon aimed (`weapon` is `None` when nothing is equipped). The model is
    /// turned ±90° about Y so that its local +z is the facing direction.
    pub fn pose_character(&self, pose: &HumanPose, position: Vec2, face_right: bool, weapon: Option<AimedWeapon>) -> PosedCharacter {
        self.pose_character_acting(pose, position, face_right, weapon, None)
    }

    /// [`Self::pose_character`] with the equipped weapon's firing animation (see
    /// [`Self::solve_aimed_acting`]).
    pub fn pose_character_acting(&self, pose: &HumanPose, position: Vec2, face_right: bool, weapon: Option<AimedWeapon>, action: Option<(&unity_data::anim::TransformClip, f32)>) -> PosedCharacter {
        let placement = BodyPlacement {
            position: Vec3::new(position.x, position.y, DEFAULT_Z),
            rotation: Quat::euler_y_degrees(if face_right { 90.0 } else { -90.0 }),
        };
        let solved = match weapon {
            Some(aimed) => self.solve_aimed_acting(pose, placement.rotation, &aimed, action),
            None => self.solve(pose),
        };
        PosedCharacter { solved, placement }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pose::ClipPose;
    use crate::weapons::corrected_aim;
    use std::path::PathBuf;
    use unity_data::humanoid::parse_avatar;
    use unity_data::rig::load_man_rig;

    fn model() -> Option<(CharacterRigModel, PathBuf)> {
        let root = std::env::var_os("STICK_EXPORT_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../assets_exported/ExportedProject"));
        let assets = root.join("Assets");
        assets.join("Game").is_dir().then_some(())?;
        let rig = load_man_rig(&assets).unwrap();
        let avatar = parse_avatar(&std::fs::read_to_string(assets.join("Avatar/ManAvatar.asset")).unwrap()).unwrap();
        Some((CharacterRigModel::new(avatar, rig).unwrap(), assets))
    }

    fn pose_of(assets: &std::path::Path, clip: &str, t: f32) -> HumanPose {
        let text = std::fs::read_to_string(assets.join(format!("AnimationClip/{clip}.anim"))).unwrap();
        ClipPose::compile(&unity_data::anim::parse_clip(&text).unwrap()).sample(t)
    }

    fn aimed(aim: Vec2) -> AimedWeapon {
        AimedWeapon { weapon: 0, aim: corrected_aim(aim, Vec2::new(1.0, 0.0), true), face_right: true }
    }

    const RIGHT: f32 = 90.0;

    fn facing() -> Quat {
        Quat::euler_y_degrees(RIGHT)
    }

    #[test]
    fn two_bone_reaches_a_reachable_goal_and_keeps_the_limb_lengths() {
        let (s, e, w) = (Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.3, -0.1, 0.0), Vec3::new(0.6, 0.0, 0.0));
        let goal = Vec3::new(0.4, 0.3, 0.1);
        let (e2, w2) = two_bone(s, e, w, goal);
        assert!((w2 - goal).magnitude() < 1e-4);
        assert!((((e2 - s).magnitude()) - (e - s).magnitude()).abs() < 1e-4);
        assert!((((w2 - e2).magnitude()) - (w - e).magnitude()).abs() < 1e-4);
        // Out of reach: straight arm pointing at the goal.
        let far = Vec3::new(3.0, 0.0, 0.0);
        let (e3, w3) = two_bone(s, e, w, far);
        let straight = (w3 - s).magnitude();
        assert!((straight - ((e - s).magnitude() + (w - e).magnitude())).abs() < 1e-3);
        assert!(((e3 - s).normalized() - (w3 - s).normalized()).magnitude() < 0.05, "an out-of-reach arm is almost straight");
    }

    #[test]
    fn both_hands_end_on_the_weapon_grips_for_every_aim() {
        let Some((model, assets)) = model() else { return };
        let rifle = &model.rig().weapons[0];
        let (left_grip, right_grip) = (rifle.left_hand.unwrap(), rifle.right_hand.unwrap());
        let left_hand = model.bone_of(bone::LEFT_HAND).unwrap();
        let right_hand = model.bone_of(bone::RIGHT_HAND).unwrap();
        for clip in ["Idle", "Run"] {
            let pose = pose_of(&assets, clip, 0.15);
            for aim in [Vec2::new(5.0, 0.0), Vec2::new(4.0, 3.0), Vec2::new(3.0, -3.0), Vec2::new(0.2, 5.0)] {
                let solved = model.solve_aimed(&pose, facing(), &aimed(aim));
                for (hand, grip, name) in [(left_hand, left_grip, "left"), (right_hand, right_grip, "right")] {
                    let miss = (solved.position(hand) - solved.position(grip)).magnitude();
                    // The grips sit within an arm's reach of the shoulders in every pose.
                    assert!(miss < 0.02, "{clip} aim {aim:?}: {name} hand misses its grip by {miss}");
                }
            }
        }
    }

    #[test]
    fn the_sword_swings_and_comes_back() {
        let Some((model, assets)) = model() else { return };
        let sword = model.rig().weapons.iter().position(|w| w.name == "Sword").unwrap();
        let clip = unity_data::anim::load_weapon_action(&assets, "Sword").expect("Sword Swing.anim");
        assert!((clip.length - 0.2).abs() < 1e-5);
        let pose = pose_of(&assets, "Idle", 0.0);
        let aimed = AimedWeapon { weapon: sword, aim: Vec2::new(1.0, 0.0), face_right: true };
        let plain = model.solve_aimed(&pose, facing(), &aimed);
        let right_grip = model.rig().weapons[sword].right_hand.unwrap();
        let blade = model.rig().bones.iter().position(|b| b.path.ends_with("Sword/Right Hand/Flattener/Sword")).unwrap();
        // At the start and the end the clip holds the rest transforms.
        for t in [0.0, 0.2] {
            let acting = model.solve_aimed_acting(&pose, facing(), &aimed, Some((&clip, t)));
            assert!((acting.position(right_grip) - plain.position(right_grip)).magnitude() < 1e-4, "t {t}");
            assert!((acting.rotation(blade).w - plain.rotation(blade).w).abs() < 1e-3 || (acting.rotation(blade).w + plain.rotation(blade).w).abs() < 1e-3, "t {t}");
        }
        // Mid-swing the grip is thrust forward and the blade has turned.
        let mid = model.solve_aimed_acting(&pose, facing(), &aimed, Some((&clip, 0.1)));
        let moved = (mid.position(right_grip) - plain.position(right_grip)).magnitude();
        assert!((moved - 0.53).abs() < 0.02, "the grip moves 0.68 - 0.15 along the pivot's x: {moved}");
        let dot = |a: Quat, b: Quat| (a.x * b.x + a.y * b.y + a.z * b.z + a.w * b.w).abs();
        assert!(dot(mid.rotation(blade), plain.rotation(blade)) < 0.9, "the blade turns");
        // The right hand follows its grip.
        let right_hand = model.bone_of(bone::RIGHT_HAND).unwrap();
        assert!((mid.position(right_hand) - mid.position(right_grip)).magnitude() < 0.05);
    }

    #[test]
    fn arm_lengths_survive_the_ik() {
        let Some((model, assets)) = model() else { return };
        let pose = pose_of(&assets, "Idle", 0.0);
        let base = model.solve(&pose);
        let solved = model.solve_aimed(&pose, facing(), &aimed(Vec2::new(4.0, 3.0)));
        for (a, b) in [(bone::LEFT_UPPER_ARM, bone::LEFT_LOWER_ARM), (bone::LEFT_LOWER_ARM, bone::LEFT_HAND), (bone::RIGHT_UPPER_ARM, bone::RIGHT_LOWER_ARM), (bone::RIGHT_LOWER_ARM, bone::RIGHT_HAND)] {
            let (a, b) = (model.bone_of(a).unwrap(), model.bone_of(b).unwrap());
            let before = (base.position(b) - base.position(a)).magnitude();
            let after = (solved.position(b) - solved.position(a)).magnitude();
            assert!((before - after).abs() < 1e-3, "bone {a}->{b}: {before} vs {after}");
        }
    }

    #[test]
    fn the_weapon_bone_points_along_the_aim() {
        let Some((model, assets)) = model() else { return };
        let pose = pose_of(&assets, "Idle", 0.0);
        let rifle = &model.rig().weapons[0];
        for aim in [Vec2::new(5.0, 0.0), Vec2::new(4.0, 3.0), Vec2::new(3.0, -3.0)] {
            let a = aimed(aim);
            let solved = model.solve_aimed(&pose, facing(), &a);
            // Local +z of the weapon bone is the aim direction, expressed in the world.
            let forward = facing() * (solved.rotation(rifle.bone) * Vec3::FORWARD);
            let want = Vec3::new(a.aim.x, a.aim.y, 0.0).normalized();
            assert!((forward - want).magnitude() < 1e-4, "aim {aim:?}: {forward:?} vs {want:?}");
            // The launch point rides on that bone, ahead of the shoulder along the aim.
            let launch = solved.position(rifle.launch_point.unwrap());
            let pivot = solved.position(rifle.bone);
            let along = facing() * (launch - pivot);
            assert!(along.dot(want) > 0.3, "the muzzle is in front of the pivot: {along:?}");
        }
    }

    #[test]
    fn aiming_up_tilts_the_head_and_its_helmet() {
        let Some((model, assets)) = model() else { return };
        let pose = pose_of(&assets, "Idle", 0.0);
        let helmet = model.rig().colors.helmet;
        let roll = |aim: Vec2| {
            let solved = model.solve_aimed(&pose, facing(), &aimed(aim));
            (facing() * solved.rotation(helmet)).euler_angles_degrees().z
        };
        let level = roll(Vec2::new(5.0, 0.0));
        let up = roll(Vec2::new(4.0, 4.0));
        let down = roll(Vec2::new(4.0, -4.0));
        // Facing right (+x), looking up turns the helmet counter-clockwise on screen (positive
        // Unity z angle: +x towards +y) and looking down turns it clockwise.
        let signed = |a: f32, b: f32| ((a - b + 540.0) % 360.0) - 180.0;
        let (du, dd) = (signed(up, level), signed(down, level));
        assert!(du > 5.0, "looking up must tilt the head up: {du}");
        assert!(dd < -5.0, "looking down must tilt the head down: {dd}");
        // Looking straight ahead leaves the face level.
        assert!(signed(level, 0.0).abs() < 5.0, "level aim: {level}");
    }

    #[test]
    fn no_weapon_selected_leaves_the_animation_untouched() {
        let Some((model, assets)) = model() else { return };
        let pose = pose_of(&assets, "Run", 0.2);
        let base = model.solve(&pose);
        let a = AimedWeapon { weapon: 99, aim: Vec2::new(1.0, 0.0), face_right: true };
        let solved = model.solve_aimed(&pose, facing(), &a);
        for i in 0..base.world.len() {
            assert_eq!(base.position(i), solved.position(i));
        }
    }
}

