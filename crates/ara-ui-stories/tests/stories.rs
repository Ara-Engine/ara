use ara_doc::{AssetId, Edit, RecordId};
use ara_schema::{Schema, SchemaDb};
use ara_ui_stories::{Result, Story, StoryBook, catalogue, localize, register};
use serde_json::json;

fn db() -> Result<SchemaDb> {
    let mut db = SchemaDb::new();
    register(&mut db)?;
    assert!(db.verify().is_empty(), "{:?}", db.verify());
    Ok(db)
}

#[test]
fn all_stories_validate_and_cover_every_primitive() -> Result<()> {
    let db = db()?;
    let stories = catalogue();
    let mut tags = std::collections::BTreeSet::new();
    for story in &stories {
        story.validate(&db)?;
        for element in ara_ui::source::parse(&story.markup)? {
            tags.insert(element.tag);
        }
        for view in &story.views {
            story.mount(view, &db)?;
        }
    }
    for tag in ara_ui::source::stylesheet::TYPES {
        assert!(tags.contains(*tag), "missing {tag}");
    }
    Ok(())
}

#[test]
fn canonical_document_roundtrip_and_atomic_edits() -> Result<()> {
    let db = db()?;
    let story = catalogue().remove(0);
    let mut book = StoryBook::new("us_examples")?;
    let record = serde_json::to_value(story)?
        .as_object()
        .ok_or("story object")?
        .clone();
    book.edit(
        Edit::Create {
            doc: AssetId::parse("us_examples")?,
            id: RecordId::parse("st_a")?,
            record,
        },
        &db,
    )?;
    let canonical = book.write(&db);
    assert_eq!(StoryBook::read(&canonical, &db)?.write(&db), canonical);
    assert!(!canonical.contains('\r'));
    assert!(!canonical.contains("\"transitions\": []"));
    let before = book.write(&db);
    assert!(
        book.edit(
            Edit::Set {
                doc: AssetId::parse("us_examples")?,
                id: RecordId::parse("st_a")?,
                path: "views".parse()?,
                value: Some(json!([]))
            },
            &db
        )
        .is_err()
    );
    assert_eq!(book.write(&db), before);
    let schema = db.doc_json_schema(db.kind("stories").ok_or("kind")?);
    assert_eq!(schema["$id"], "ara://schema/stories/1");
    assert!(schema["$defs"].get(Story::describe().name).is_some());
    assert!(
        StoryBook::read(
            &canonical.replace("\"records\":", "\"id\":\"us_other\",\"records\":"),
            &db
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn pseudo_locale_changes_captions_not_identity_or_model_data() -> Result<()> {
    let db = db()?;
    let story = catalogue()
        .into_iter()
        .find(|s| s.component == "field")
        .ok_or("field")?;
    let view = &story.views[4];
    let locale = story.locale(&view.locale)?;
    let text = localize("loc:resume", locale)?;
    assert!(text.starts_with("[Rëšümë"));
    assert!(text.chars().count() > "Resume".len());
    let ui = story.mount(view, &db)?;
    let n = &ui.nodes[ui.find("sample").ok_or("sample")?];
    assert_eq!(n.text.as_ref().ok_or("text")?.value, "Outpost");
    assert_eq!(n.element.attrs["action"], "game.edit");
    assert_eq!(n.element.attrs["label"], localize("loc:search", locale)?);
    assert!(localize("loc:missing", locale).is_err());
    Ok(())
}

#[test]
fn malformed_fixture_and_capture_dimensions_fail_before_layout() -> Result<()> {
    let db = db()?;
    let mut story = catalogue().remove(0);
    story.fixture.insert("value".into(), json!("wrong"));
    assert!(story.validate(&db).is_err());
    story.fixture.remove("value");
    story.views[0].dpi = 0;
    assert!(story.validate(&db).is_err());
    story.views[0].dpi = 768;
    story.views[0].width = u32::MAX;
    assert!(story.validate(&db).is_err());
    Ok(())
}
