//! Regenerate the reviewed public schemas and minimal story example using canonical writers.
use ara_schema::SchemaDb;
use ara_ui_stories::{StoryBook, catalogue};
use ara_ui_test::Result;

fn main() -> Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let schema_dir = root.join("schema");
    std::fs::create_dir_all(&schema_dir)?;
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db)?;
    let kind = db.kind("stories").ok_or("story kind")?;
    std::fs::write(
        schema_dir.join("stories.schema.json"),
        ara_json::to_canonical_string(&db.doc_json_schema(kind)),
    )?;
    std::fs::write(
        schema_dir.join("capture-ui.schema.json"),
        ara_json::to_canonical_string(&ara_ui_test::report_schema()),
    )?;
    let story = catalogue()
        .into_iter()
        .find(|s| s.component == "button")
        .ok_or("button")?;
    let mut book = StoryBook::new("us_button")?;
    book.edit(
        ara_doc::Edit::Create {
            doc: book.document().id.clone(),
            id: ara_doc::RecordId::parse("st_button")?,
            record: ara_json::to_value(&story)?
                .as_object()
                .ok_or("story object")?
                .clone(),
        },
        &db,
    )?;
    std::fs::write(root.join("examples/button.stories"), book.write(&db))?;
    Ok(())
}
