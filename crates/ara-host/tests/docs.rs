use ara_cmd::Registry;
use ara_host::docs;
use ara_schema::SchemaDb;
use serde_json::Value;

#[test]
fn structured_reference_round_trips_and_covers_every_catalog_and_example() {
    let mut db = SchemaDb::new();
    assert!(ara_runtime::engine::register_schemas(&mut db).is_empty());
    let mut registry = Registry::builtin(&mut db);
    ara_runtime::engine::register_commands(&mut registry, &mut db);
    let methods = ara_host::core_methods();
    let text = docs::structured(&db, &registry, &methods, "reference");
    let value: Value = serde_json::from_str(&text).expect("docs JSON");
    assert_eq!(
        text,
        ara_json::to_canonical_string(&ara_json::sort_keys(&value))
    );
    assert_eq!(value["format"], docs::FORMAT);
    assert_eq!(
        value["types"]["Light"]["fields"]["color"]["editor"]["temperature"],
        true
    );
    assert_eq!(
        value["types"]["MeshRenderer"]["fields"]["emissiveFactor"]["editor"]["hdr"],
        true
    );
    assert!(value["types"]["PostOverride"]["fields"]["lut"].is_object());
    assert!(value["types"].get("PostProcessVolume").is_some());
    assert!(value["types"].get("PostProcess").is_none());
    for t in db.types() {
        assert!(value["types"].get(&t.name).is_some(), "missing {}", t.name);
        for f in t.fields() {
            let field = &value["types"][&t.name]["fields"][&f.name];
            if f.editor != ara_schema::EditorHints::default() {
                assert_eq!(
                    field["editor"],
                    serde_json::to_value(&f.editor).expect("editor hints"),
                    "{}.{}",
                    t.name,
                    f.name
                );
            }
            for key in [
                "type",
                "unit",
                "range",
                "default",
                "doc",
                "replication",
                "examples",
            ] {
                assert!(
                    field.get(key).is_some(),
                    "{}.{} missing {key}",
                    t.name,
                    f.name
                );
            }
        }
    }
    for command in registry.infos() {
        assert!(
            value["commands"][&command.name]["parameters"].is_object(),
            "{}",
            command.name
        );
    }
    for method in methods {
        assert!(value["methods"][&method.name]["params"].is_object());
        assert!(
            !value["methods"][&method.name]["results"]["properties"]
                .as_object()
                .expect("documented result object")
                .is_empty(),
            "missing result contract for {}",
            method.name
        );
    }
    for code in ara_diag::catalog::all() {
        assert_eq!(
            value["diagnostics"][code.code.to_string()]["owner"],
            code.owner
        );
    }
    let examples = docs::csharp_examples();
    let pages = docs::generate(&db, &registry, &ara_host::core_methods(), "reference");
    let full = docs::llms_full(&pages, "reference");
    for t in db.components() {
        let id = format!("types/{}", t.name);
        let matching: Vec<_> = examples.iter().filter(|e| e.page == id).collect();
        assert_eq!(
            matching.len(),
            1,
            "exactly one compiled example for {}",
            t.name
        );
        assert!(full.contains(&matching[0].code));
        assert!(
            pages
                .iter()
                .find(|p| p.id == id)
                .expect("type page")
                .body
                .contains(&matching[0].code)
        );
    }
    for e in examples {
        assert!(
            pages.iter().any(|p| p.id == e.page),
            "orphan region {}",
            e.page
        );
        assert!(
            value["csharpExamples"]
                .as_array()
                .expect("examples")
                .iter()
                .any(|x| x["region"] == e.region)
        );
    }
    let light = docs::search(&pages, "Flicker", 10);
    assert!(light.iter().any(|p| p.id == "types/Light"));
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/csharp-docs");
    std::fs::create_dir_all(&dir).expect("scratch docs output");
    std::fs::write(dir.join("docs.json"), &text).expect("reference artifact");
}
