use super::*;

// The pre-optimization header/read path validated a SourceDocument, then parsed
// legacy elements again to choose the ID. Retain it as the identity/error oracle.
fn reference(text: &str, path: &str) -> Result<AssetId, Diagnostic> {
    SourceDocument::parse(text, DocumentKind::Markup).map_err(invalid)?;
    if let Some(id) = metadata(text) {
        let id = AssetId::parse(id).map_err(invalid)?;
        if id.prefix() != "ui" {
            return Err(invalid("UI asset IDs start with ui_"));
        }
        return Ok(id);
    }
    let elements = ara_ui_document::parse(text).map_err(invalid)?;
    if let Some(id) = elements
        .first()
        .and_then(|e| AssetId::parse(&e.id).ok())
        .filter(|id| id.prefix() == "ui")
    {
        return Ok(id);
    }
    AssetId::parse(&format!("ui_{:x}", ara_ui_document::stable_id(path))).map_err(invalid)
}

#[test]
fn one_markup_parse_preserves_identity_diagnostics_and_exact_native_bytes() {
    let path = "content/ui/menu.ui";
    for text in [
        "<ui id='root'><text id='label' text='Legacy &amp; source'/></ui>\n",
        "<!-- keep -->\r\n<ui id='root'>\r\n <text id='label' text='Ω &amp; 🙂'/>\r\n</ui>\r\n",
        "<ui id='ui_12345678'/>\n",
        "<!-- ara:asset ui_12345678 -->\n<ui id='ui_87654321'/>\n",
        "<?xml version=\"1.0\"?>\n<!-- ara:asset ui_12345678 -->\n<ui id='root'/>\n",
        "<!-- ara:asset sc_12345678 -->\n<ui id='root'/>\n",
        "<!-- ara:asset not-an-id -->\n<ui id='root'/>\n",
        "<!-- ara:asset ui_12345678 -->\n<ui id='root'><unknown id='child'/></ui>",
        "<!-- ara:asset sc_12345678 -->\n<ui id='root'><unknown id='child'/></ui>",
        "<ui id='root'><text id='duplicate'/><text id='duplicate'/></ui>",
        "<ui id='root' unsupported='value'/>",
        "<ui id='root'>",
    ] {
        let expected = reference(text, path);
        assert_eq!(identity(text, path), expected, "{text}");
        let actual = read(text, path);
        match expected {
            Ok(id) => {
                assert_eq!(header(text, path), Some(("ui".to_owned(), id.clone())));
                assert!(actual.diags.is_empty());
                let doc = actual.doc.expect("valid native markup");
                assert_eq!(doc.id, id);
                assert_eq!(source(&doc).expect("source"), text);
                assert!(validate(&doc, path).is_ok());
                assert_eq!(write(&doc).expect("lossless native write"), text);
            }
            Err(error) => {
                assert_eq!(header(text, path), None);
                assert!(actual.doc.is_none());
                assert_eq!(actual.diags, vec![error.file(path)]);
            }
        }
    }
}
