//! Run: cargo run -p ara-ui-test --example capture -- pause 2 <output-directory>
use ara_schema::SchemaDb;
use ara_ui_stories::StoryBook;
use ara_ui_test::{Harness, Result};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "pause".into());
    let variant: usize = args.next().unwrap_or_else(|| "0".into()).parse()?;
    let output = args
        .next()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("evidence/manual")
        });
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db)?;
    let book = StoryBook::read(include_str!("catalogue.stories"), &db)?;
    let story = book
        .stories()?
        .into_iter()
        .map(|(_, story)| story)
        .find(|s| s.name == name)
        .ok_or("unknown story name")?;
    let view = story.views.get(variant).ok_or("view index out of range")?;
    let mut test = Harness::mount(&story, view, &db)?;
    let capture = test.capture()?;
    for path in capture.write(&output, &format!("{name}-{variant}"))? {
        println!("{}", path.display());
    }
    test.run()?;
    println!(
        "Semantic recipe passed; {} diagnostics",
        capture.report.diagnostics.len()
    );
    Ok(())
}
