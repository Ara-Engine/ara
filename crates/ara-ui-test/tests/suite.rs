use ara_schema::SchemaDb;
use ara_ui_stories::{StoryBook, catalogue};
use ara_ui_test::{Harness, Result, Tolerance};

#[test]
fn complete_story_suite_goldens_and_semantics() -> Result<()> {
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db)?;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let golden_dir = root.join("goldens");
    let output = root.join("evidence");
    let update = std::env::var("ARA_UI_UPDATE_STORY_GOLDENS").is_ok_and(|v| v == "1");
    std::fs::create_dir_all(&golden_dir)?;
    let stories = catalogue();
    let mut total = 0;
    let mut failures = Vec::new();
    for story in &stories {
        for (v, view) in story.views.iter().enumerate() {
            let name = format!("{}-{v}", story.name);
            let run = || -> Result<()> {
                let mut harness = Harness::mount(story, view, &db)?;
                let capture = harness.capture()?;
                assert!(
                    capture
                        .report
                        .diagnostics
                        .iter()
                        .all(|diagnostic| diagnostic.code != "ARA7102"),
                    "missing glyph coverage in {name}"
                );
                assert!(
                    capture.report.nodes.iter().all(|node| node
                        .text
                        .as_deref()
                        .is_none_or(|text| !text.starts_with("loc:"))),
                    "unresolved caption in {name}"
                );
                if update {
                    std::fs::write(golden_dir.join(format!("{name}.png")), &capture.png)?;
                }
                let expected =
                    std::fs::read(golden_dir.join(format!("{name}.png"))).map_err(|e| {
                        format!(
                            "Missing golden {name}: {e}; explicitly rebaseline and inspect captures"
                        )
                    })?;
                capture.compare(
                    &expected,
                    Tolerance::default(),
                    &output.join("failures"),
                    &name,
                )?;
                capture.write(&output, &name)?;
                harness.run()?;
                if story.steps.iter().any(|step| {
                    matches!(
                        step.kind,
                        ara_ui_stories::StepKind::Activate
                            | ara_ui_stories::StepKind::Text
                            | ara_ui_stories::StepKind::Advance
                    )
                }) {
                    let name = format!("{name}-after");
                    let capture = harness.capture()?;
                    if update {
                        std::fs::write(golden_dir.join(format!("{name}.png")), &capture.png)?;
                    }
                    let expected = std::fs::read(golden_dir.join(format!("{name}.png")))
                        .map_err(|e| format!("Missing interaction golden {name}: {e}"))?;
                    capture.compare(
                        &expected,
                        Tolerance::default(),
                        &output.join("failures"),
                        &name,
                    )?;
                    capture.write(&output, &name)?;
                }
                Ok(())
            };
            if let Err(error) = run() {
                failures.push(format!("{name}: {error}"));
            }
            total += 1;
        }
    }
    // Exercise the persisted format with the exact same catalogue used by the runner.
    let mut book = StoryBook::new("us_catalogue")?;
    for (index, story) in stories.iter().enumerate() {
        book.edit(ara_doc_edit(book.document().id.clone(), index, story)?, &db)?;
    }
    let source = book.write(&db);
    let path = root.join("examples/catalogue.stories");
    if update {
        std::fs::create_dir_all(path.parent().ok_or("example directory")?)?;
        std::fs::write(&path, &source)?;
    }
    assert_eq!(
        std::fs::read_to_string(&path)?,
        source,
        "persisted catalogue is stale"
    );
    assert_eq!(
        StoryBook::read(&source, &db)?.stories()?.len(),
        stories.len()
    );
    println!("{total} complete story views captured, compared and semantically tested");
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n\n").into())
    }
}

fn ara_doc_edit(
    doc: ara_doc::AssetId,
    index: usize,
    story: &ara_ui_stories::Story,
) -> Result<ara_doc::Edit> {
    Ok(ara_doc::Edit::Create {
        doc,
        id: ara_doc::RecordId::parse(&format!("st_{index:x}"))?,
        record: ara_json::to_value(story)?
            .as_object()
            .ok_or("story object")?
            .clone(),
    })
}
