//! Generates worked canonical documents through typed commands and attributed kernel ops.
mod v2_support;
use ara_anim::{
    document::{self, AnimAsset, AnimationCommand, IkSetup},
    ik::*,
    matching::*,
    physical::*,
    retarget::*,
    *,
};
use ara_doc::{AssetIndex, IdGen, RecordId, Workspace};
use ara_ops::{Author, Store, TxnMeta};
use ara_schema::SchemaDb;
use std::path::PathBuf;
fn chain(names: &[&str]) -> ChainData {
    ChainData {
        joints: names.iter().map(|s| (*s).into()).collect(),
        ..Default::default()
    }
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("crates/ara-anim/examples/documents"));
    std::fs::create_dir_all(&output)?;
    let s = v2_support::skeleton(1.0, 20)?;
    for (name, number) in [
        ("ik", 1),
        ("retarget", 2),
        ("motion-matching", 3),
        ("physical", 4),
    ] {
        let mut schema = SchemaDb::new();
        document::register(&mut schema)?;
        let source = format!(
            r#"{{"$schema":"ara://schema/animation/1","id":"ac_v2demo{number}","records":{{}}}}"#
        );
        let doc = document::read(&source, "generated.animation", &schema)
            .map_err(|d| format!("{d:?}"))?;
        let id = doc.id.clone();
        let record = RecordId::parse("an_character")?;
        let mut workspace = Workspace::new(AssetIndex::new());
        workspace.insert(format!("{name}.animation"), doc);
        let mut store = Store::with_ids(schema, workspace, IdGen::seeded(7));
        let mut apply = |command: AnimationCommand, target: RecordId| -> Result<(), Error> {
            command
                .apply(
                    &mut store,
                    TxnMeta::by(Author::Agent("animation-v2-example".into())),
                    id.clone(),
                    target,
                )
                .map_err(|d| Error(format!("{d:?}")))?;
            Ok(())
        };
        apply(
            AnimationCommand::Create(AnimAsset {
                name: format!("Worked {name} character"),
                skeleton: Some(s.data().clone()),
                ..Default::default()
            }),
            record.clone(),
        )?;
        match name {
            "ik" => {
                let left = chain(&["LeftUpperLeg", "LeftLowerLeg", "LeftFoot"]);
                let right = chain(&["RightUpperLeg", "RightLowerLeg", "RightFoot"]);
                apply(
                    AnimationCommand::SetIk(IkSetup {
                        pelvis: "Hips".into(),
                        chains: vec![
                            left.clone(),
                            right.clone(),
                            chain(&["LeftUpperArm", "LeftLowerArm", "LeftHand"]),
                            chain(&["RightUpperArm", "RightLowerArm", "RightHand"]),
                        ],
                        feet: vec![
                            Foot {
                                chain: left,
                                ..Default::default()
                            },
                            Foot {
                                chain: right,
                                ..Default::default()
                            },
                        ],
                        ground: FootSettings {
                            sole_height: 0.04,
                            ..Default::default()
                        },
                    }),
                    record.clone(),
                )?;
            }
            "retarget" => {
                apply(AnimationCommand::SetRig(RigMap::auto(&s)?), record.clone())?;
                let target = v2_support::skeleton(1.3, 20)?;
                let target_id = RecordId::parse("an_target")?;
                apply(
                    AnimationCommand::Create(AnimAsset {
                        name: "1.3x target rig".into(),
                        skeleton: Some(target.data().clone()),
                        ..Default::default()
                    }),
                    target_id.clone(),
                )?;
                apply(AnimationCommand::SetRig(RigMap::auto(&target)?), target_id)?;
            }
            "motion-matching" => {
                apply(
                    AnimationCommand::SetMotionDatabase(MatchConfig {
                        clips: (0..8).map(v2_support::reference).collect(),
                        bones: vec!["LeftFoot".into(), "RightFoot".into()],
                        horizons: vec![9600, 24000],
                        ..Default::default()
                    }),
                    record.clone(),
                )?;
            }
            _ => {
                let profile = RagdollProfile {
                    bones: [
                        ("Hips", None),
                        ("Chest", Some(0)),
                        ("Head", Some(1)),
                        ("LeftUpperLeg", Some(0)),
                        ("LeftLowerLeg", Some(3)),
                        ("RightUpperLeg", Some(0)),
                        ("RightLowerLeg", Some(5)),
                    ]
                    .into_iter()
                    .map(|(joint, parent)| RagdollBone {
                        joint: joint.into(),
                        parent,
                        ..Default::default()
                    })
                    .collect(),
                };
                apply(AnimationCommand::SetRagdoll(profile), record.clone())?;
            }
        }
        let document = store
            .workspace()
            .doc(id.as_str())
            .ok_or("example document is missing")?;
        let text = document::write(document, store.schema());
        let reread = document::read(&text, "generated.animation", store.schema())
            .map_err(|d| format!("{d:?}"))?;
        if text != document::write(&reread, store.schema()) {
            return Err("canonical roundtrip mismatch".into());
        }
        std::fs::write(output.join(format!("{name}.animation")), text)?;
    }
    println!("{}", output.display());
    Ok(())
}
