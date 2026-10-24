//! Material document skeletons use the authoritative graph format (A8).
use ara_cmd::{EditCtx, Registry};
use ara_diag::Diagnostic;
use ara_doc::{AssetId, Edit};

fn create(cx: &mut EditCtx<'_, '_>, path: &str, id: Option<&str>) -> Result<AssetId, Diagnostic> {
    let id = cx.create_document(path, "graph", id)?;
    let material = crate::dsl::parse(
        "material Material : surface budget 1 { let color : ColorLinear = linear(1, 1, 1); output baseColor = color; }",
        id.as_str(),
    )
    .expect("the built-in empty material is valid DSL");
    let document = material
        .to_graph(ara_graph::Editor::default())?
        .to_document()?;
    for (record, value) in document.records() {
        cx.apply_edit(Edit::Create {
            doc: id.clone(),
            id: record.clone(),
            record: value.clone(),
        })?;
    }
    Ok(id)
}

pub(crate) fn register(registry: &mut Registry) {
    registry.document_template("material", create);
}
