use crate::{Error, Skeleton, ensure};
use ara_schema::Schema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

/// Anatomical roles shared by Mixamo, UE Mannequin and Unity Humanoid rigs.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Schema, Serialize, Deserialize,
)]
#[ara(module = "animation")]
#[serde(rename_all = "camelCase")]
pub enum BoneRole {
    /// Locomotion root, possibly separate from the pelvis.
    #[default]
    Root,
    /// Pelvis.
    Hips,
    /// Lower spine.
    Spine,
    /// Upper spine/chest.
    Chest,
    /// Optional upper chest, above the main chest role.
    UpperChest,
    /// Neck.
    Neck,
    /// Head/look joint.
    Head,
    /// Left clavicle.
    LeftShoulder,
    /// Left upper arm.
    LeftUpperArm,
    /// Left forearm.
    LeftLowerArm,
    /// Left wrist/hand effector.
    LeftHand,
    /// Right clavicle.
    RightShoulder,
    /// Right upper arm.
    RightUpperArm,
    /// Right forearm.
    RightLowerArm,
    /// Right wrist/hand effector.
    RightHand,
    /// Left thigh.
    LeftUpperLeg,
    /// Left shin.
    LeftLowerLeg,
    /// Left ankle/foot effector.
    LeftFoot,
    /// Left ball/toes.
    LeftToes,
    /// Right thigh.
    RightUpperLeg,
    /// Right shin.
    RightLowerLeg,
    /// Right ankle/foot effector.
    RightFoot,
    /// Right ball/toes.
    RightToes,
}
/// One semantic bone binding, using a stable importer joint ID.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigBone {
    /// Anatomical role, unique within this rig.
    pub role: BoneRole,
    /// Stable skeleton joint identity.
    pub joint: String,
}
/// Canonical humanoid definition. Partial rigs are legal; ambiguity is never guessed.
#[derive(Debug, Clone, Default, PartialEq, Schema, Serialize, Deserialize)]
#[ara(module = "animation")]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct RigMap {
    /// Unique role-to-joint bindings.
    pub bones: Vec<RigBone>,
}
impl RigMap {
    /// Resolves a role to its skeleton index at setup time.
    pub fn joint(&self, skeleton: &Skeleton, role: BoneRole) -> Option<usize> {
        let id = &self.bones.iter().find(|b| b.role == role)?.joint;
        skeleton.data.joints.iter().position(|j| &j.id == id)
    }
    /// Rejects missing/duplicate joints, duplicate roles and disconnected humanoid limbs.
    pub fn validate(&self, skeleton: &Skeleton) -> Result<(), Error> {
        let mut roles = BTreeSet::new();
        let mut ids = BTreeSet::new();
        ensure(
            self.bones.iter().all(|b| {
                roles.insert(b.role)
                    && ids.insert(&b.joint)
                    && skeleton.data.joints.iter().any(|j| j.id == b.joint)
            }),
            "rig roles/joints must exist and be unique",
        )?;
        for (a, b) in relations() {
            if let (Some(parent), Some(mut child)) =
                (self.joint(skeleton, a), self.joint(skeleton, b))
            {
                while child != parent {
                    child = skeleton.parents[child]
                        .ok_or_else(|| Error("humanoid roles disagree with topology".into()))?
                        as usize;
                }
            }
        }
        Ok(())
    }
    /// Maps explicit semantic metadata first, then known names and unambiguous topology.
    /// Namespace prefixes, separators and case are ignored. Twist/helper bones are skipped.
    pub fn auto(skeleton: &Skeleton) -> Result<Self, Error> {
        let mut out = Self::default();
        for joint in &skeleton.data.joints {
            let role = role(&joint.retarget.role).or_else(|| role(&joint.name));
            if let Some(role) = role {
                ensure(
                    !out.bones.iter().any(|b| b.role == role),
                    "ambiguous humanoid name; provide an explicit rig map",
                )?;
                out.bones.push(RigBone {
                    role,
                    joint: joint.id.clone(),
                });
            }
        }
        // Infer a missing middle joint only when exactly one parent separates the
        // two known endpoints. Longer paths may contain twist bones; leave them explicit.
        for (a, middle, b) in [
            (
                BoneRole::LeftUpperArm,
                BoneRole::LeftLowerArm,
                BoneRole::LeftHand,
            ),
            (
                BoneRole::RightUpperArm,
                BoneRole::RightLowerArm,
                BoneRole::RightHand,
            ),
            (
                BoneRole::LeftUpperLeg,
                BoneRole::LeftLowerLeg,
                BoneRole::LeftFoot,
            ),
            (
                BoneRole::RightUpperLeg,
                BoneRole::RightLowerLeg,
                BoneRole::RightFoot,
            ),
        ] {
            if out.joint(skeleton, middle).is_some() {
                continue;
            }
            if let (Some(root), Some(tip)) = (out.joint(skeleton, a), out.joint(skeleton, b))
                && let Some(parent) = skeleton.parents[tip]
                    .filter(|p| skeleton.parents[*p as usize] == Some(root as u32))
            {
                let id = &skeleton.data.joints[parent as usize].id;
                if !out.bones.iter().any(|b| &b.joint == id) {
                    out.bones.push(RigBone {
                        role: middle,
                        joint: id.clone(),
                    });
                }
            }
        }
        out.bones.sort_by_key(|b| b.role);
        out.validate(skeleton)?;
        Ok(out)
    }
}
fn relations() -> [(BoneRole, BoneRole); 17] {
    use BoneRole::*;
    [
        (Hips, Spine),
        (Spine, Chest),
        (Chest, UpperChest),
        (Chest, Neck),
        (Neck, Head),
        (LeftUpperArm, LeftLowerArm),
        (LeftLowerArm, LeftHand),
        (RightUpperArm, RightLowerArm),
        (RightLowerArm, RightHand),
        (Hips, LeftUpperLeg),
        (LeftUpperLeg, LeftLowerLeg),
        (LeftLowerLeg, LeftFoot),
        (Hips, RightUpperLeg),
        (RightUpperLeg, RightLowerLeg),
        (RightLowerLeg, RightFoot),
        (LeftFoot, LeftToes),
        (RightFoot, RightToes),
    ]
}
fn role(name: &str) -> Option<BoneRole> {
    use BoneRole::*;
    let tail = name.rsplit([':', '|', '/']).next().unwrap_or(name);
    let n: String = tail
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let n = n.strip_prefix("mixamorig").unwrap_or(&n);
    Some(match n {
        "root" | "armature" => Root,
        "hips" | "pelvis" => Hips,
        "spine" | "spine01" => Spine,
        "chest" | "spine2" | "spine03" => Chest,
        "upperchest" => UpperChest,
        "neck" | "neck01" => Neck,
        "head" => Head,
        "leftshoulder" | "claviclel" => LeftShoulder,
        "rightshoulder" | "clavicler" => RightShoulder,
        "leftarm" | "leftupperarm" | "upperarml" => LeftUpperArm,
        "rightarm" | "rightupperarm" | "upperarmr" => RightUpperArm,
        "leftforearm" | "leftlowerarm" | "lowerarml" => LeftLowerArm,
        "rightforearm" | "rightlowerarm" | "lowerarmr" => RightLowerArm,
        "lefthand" | "handl" => LeftHand,
        "righthand" | "handr" => RightHand,
        "leftupleg" | "leftupperleg" | "thighl" => LeftUpperLeg,
        "rightupleg" | "rightupperleg" | "thighr" => RightUpperLeg,
        "leftleg" | "leftlowerleg" | "calfl" => LeftLowerLeg,
        "rightleg" | "rightlowerleg" | "calfr" => RightLowerLeg,
        "leftfoot" | "footl" => LeftFoot,
        "rightfoot" | "footr" => RightFoot,
        "lefttoebase" | "lefttoes" | "balll" => LeftToes,
        "righttoebase" | "righttoes" | "ballr" => RightToes,
        _ => return None,
    })
}
