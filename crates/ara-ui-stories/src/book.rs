use crate::{Result, Story, UiStoryFixture};
use ara_doc::{Document, Edit, Op};
use ara_schema::{DocKindDesc, SchemaDb};

/// Register story records and their standard Ara document kind in a host schema database.
pub fn register(db: &mut SchemaDb) -> Result<()> {
    db.register::<Story>().register::<UiStoryFixture>();
    let kind = DocKindDesc {
        name: "stories".into(),
        version: 1,
        doc: "Reproducible native UI component stories, fixture models and semantic recipes."
            .into(),
        id_prefix: "us".into(),
        extension: "stories".into(),
        records: vec!["Story".into()],
        single_root: false,
    };
    if let Some(existing) = db.kind(&kind.name) {
        if existing != &kind {
            return Err("conflicting ui-stories document kind".into());
        }
    } else {
        db.add_kind(kind).map_err(|d| d.to_string())?;
    }
    Ok(())
}

/// An immutable-view story document; all authoring mutations produce reversible Ara ops.
pub struct StoryBook {
    document: Document,
}
impl StoryBook {
    /// Start a new document; use `edit(Create)` to add story records.
    pub fn new(id: &str) -> Result<Self> {
        let document = Document::new("stories", ara_doc::AssetId::parse(id)?);
        if document.id.prefix() != "us" {
            return Err("UI story document IDs must use the us_ prefix".into());
        }
        Ok(Self { document })
    }
    /// Strictly read and schema-validate canonical Ara story text.
    pub fn read(text: &str, db: &SchemaDb) -> Result<Self> {
        let read = ara_doc::read(text, "component.stories", db, None);
        let errors: Vec<_> = read
            .diags
            .iter()
            .filter(|d| d.is_error())
            .map(ToString::to_string)
            .collect();
        if !errors.is_empty() {
            return Err(errors.join("\n").into());
        }
        let book = Self {
            document: read.doc.ok_or("invalid story document")?,
        };
        if book.document.kind != "stories" {
            return Err("expected ui-stories document kind".into());
        }
        for (_, story) in book.stories()? {
            story.validate(db)?;
        }
        Ok(book)
    }
    /// Apply one typed edit atomically and return its reversible operation.
    pub fn edit(&mut self, edit: Edit, db: &SchemaDb) -> Result<Op> {
        let mut candidate = self.document.clone();
        let op = candidate.apply(edit, db).map_err(|d| d.to_string())?;
        for record in candidate.records().values() {
            let story: Story = serde_json::from_value(record.clone().into())?;
            story.validate(db)?;
        }
        self.document = candidate;
        Ok(op)
    }
    /// Decode typed stories in stable record-ID order.
    pub fn stories(&self) -> Result<Vec<(String, Story)>> {
        self.document
            .records()
            .iter()
            .map(|(id, record)| {
                Ok((
                    id.to_string(),
                    serde_json::from_value(record.clone().into())?,
                ))
            })
            .collect()
    }
    /// Read-only access for protocol inspection and host transactions.
    pub fn document(&self) -> &Document {
        &self.document
    }
    /// Write using Ara's canonical document writer: reflected field order, omitted defaults and LF.
    pub fn write(&self, db: &SchemaDb) -> String {
        ara_doc::write(&self.document, db)
    }
}
