#![allow(clippy::unwrap_used)]
use ara_anim::{
    document, ik::Chain, matching::MotionDatabase, physical::PhysicalAnimation,
    retarget::Retargeter, *,
};
use ara_doc::RecordId;
use ara_schema::SchemaDb;
#[test]
fn worked_canonical_documents_cook_end_to_end() {
    let mut db = SchemaDb::new();
    document::register(&mut db).unwrap();
    for (name, text) in [
        ("ik", include_str!("../examples/documents/ik.animation")),
        (
            "retarget",
            include_str!("../examples/documents/retarget.animation"),
        ),
        (
            "motion-matching",
            include_str!("../examples/documents/motion-matching.animation"),
        ),
        (
            "physical",
            include_str!("../examples/documents/physical.animation"),
        ),
    ] {
        let doc = document::read(text, &format!("{name}.animation"), &db).unwrap();
        assert_eq!(document::write(&doc, &db), text);
        let asset = document::asset(&doc, &RecordId::parse("an_character").unwrap()).unwrap();
        let skeleton = Skeleton::cook(asset.skeleton.unwrap()).unwrap();
        match name {
            "ik" => {
                for chain in &asset.ik.unwrap().chains {
                    Chain::cook(&skeleton, chain).unwrap();
                }
            }
            "retarget" => {
                let target = document::asset(&doc, &RecordId::parse("an_target").unwrap()).unwrap();
                let target_skeleton = Skeleton::cook(target.skeleton.unwrap()).unwrap();
                Retargeter::cook(
                    &skeleton,
                    &asset.rig.unwrap(),
                    &target_skeleton,
                    &target.rig.unwrap(),
                )
                .unwrap();
            }
            "motion-matching" => {
                let config = asset.motion_database.unwrap();
                let clips: Vec<_> = config
                    .clips
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        let asset = document::asset(
                            &doc,
                            &RecordId::parse(&format!("an_clip{i}")).unwrap(),
                        )
                        .unwrap();
                        (
                            r.clone(),
                            CookedClip::cook(
                                &asset.clip.unwrap(),
                                &skeleton,
                                CompressionSettings::default(),
                            )
                            .unwrap()
                            .0,
                        )
                    })
                    .collect();
                let borrowed: Vec<_> = clips.iter().map(|(r, c)| (r.clone(), c)).collect();
                let database = MotionDatabase::cook(&config, &skeleton, &borrowed).unwrap();
                let controller = Controller::compile_with_matching(
                    &asset.controller.unwrap(),
                    &skeleton,
                    clips,
                    vec![(
                        ara_schema::AssetRef("@motion-matching.animation#an_character".into()),
                        database,
                    )],
                )
                .unwrap();
                let mut actor = Animator::new(&controller, &skeleton).unwrap();
                actor.tick(&controller, &skeleton, |_| {}).unwrap();
            }
            _ => {
                PhysicalAnimation::cook(&skeleton, &asset.ragdoll.unwrap()).unwrap();
            }
        }
    }
}
