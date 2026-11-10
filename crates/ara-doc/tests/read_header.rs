//! Indexing must accept exactly the strict JSON/header shape the full parser accepts.
use ara_doc::{AssetId, read_header};
use ara_schema::SchemaDb;
use serde_json::{Value, json};

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    ara_doc::register(&mut db);
    db
}

fn reference(text: &str, db: &SchemaDb) -> Option<(String, AssetId)> {
    let root = ara_json::parse(text).ok()?;
    let (kind, _) = db.kind_for_schema(root.get("$schema")?.as_str()?).ok()?;
    Some((
        kind.name.clone(),
        AssetId::parse(root.get("id")?.as_str()?).ok()?,
    ))
}

#[test]
fn indexing_rejects_duplicates_and_malformed_payloads_even_after_the_header() {
    let db = schema();
    let suffixes = [
        r#", "records": {"e_test": {"Transform": {"position": 1, "position": 2}}}}"#,
        r#", "records": {"e_test": {"name": 1, "n\u0061me": 2}}}"#,
        r#", "records": {"e_test": [{"value": 1, "value": 2}]}}"#,
        r#", "records": {"e_test": {"a":1,"b":2,"c":3,"d":4,"e":5,"a":6}}}"#,
        r#", "records": {"e_test": {"a":1,"b":2,"c":3,"d":4,"e":5,"e":6}}}"#,
        r#", "records": {"e_test": [1, 2,]}}"#,
        r#", "records": {"e_test": 1e999}}"#,
        r#", "records": {"e_test": "\uD800"}}"#,
        r#", "records": {}} true"#,
        r#", "id": "sc_87654321"}"#,
        r#", "\u0069d": "sc_87654321"}"#,
    ];
    for suffix in suffixes {
        let text = format!(r#"{{"$schema":"ara://schema/scene/1","id":"sc_12345678"{suffix}"#);
        assert_eq!(read_header(&text, &db), None, "{text}");
        assert_eq!(reference(&text, &db), None, "bad fixture: {text}");
    }
}

#[test]
fn indexing_preserves_header_order_escaping_and_non_string_header_semantics() {
    let db = schema();
    for text in [
        r#"{"records":{"e_a":{"name":"escaped \" slash \\ and \u263a","values":[null,true,-3,18446744073709551615,1e-20]}},"id":"sc_12345678","$schema":"ara://schema/scene/1"}"#,
        r#"{"\u0069d":"sc_12345678","$schema":"ara://schema/scene/1","records":{}}"#,
        r#"{"id":"sc_12345678","$schema":null}"#,
        r#"{"id":{},"$schema":"ara://schema/scene/1"}"#,
        r#"{"id":[],"$schema":"ara://schema/scene/1"}"#,
        r#"{"id":true,"$schema":"ara://schema/scene/1"}"#,
        r#"{"id":123,"$schema":"ara://schema/scene/1"}"#,
        r#"{"id":"invalid","$schema":"ara://schema/scene/1"}"#,
        r#"{"id":"sc_12345678","$schema":"unknown"}"#,
        r#"{"id":"pf_12345678","$schema":"ara://schema/scene/1"}"#,
        r#"[{"id":"sc_12345678","$schema":"ara://schema/scene/1"}]"#,
    ] {
        assert_eq!(read_header(text, &db), reference(text, &db), "{text}");
    }
}

#[test]
fn indexing_keeps_the_full_parsers_recursion_limit() {
    let db = schema();
    for depth in [120, 126, 127, 128, 129, 256] {
        let text = format!(
            r#"{{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{}0{}}}"#,
            "[".repeat(depth),
            "]".repeat(depth)
        );
        assert_eq!(
            read_header(&text, &db),
            reference(&text, &db),
            "depth {depth}"
        );
    }
}

fn payload() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(Value::from),
        ".{0,30}".prop_map(Value::String),
    ]
    .prop_recursive(5, 128, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            prop::collection::btree_map(".{0,20}", inner, 0..8)
                .prop_map(|items| Value::Object(items.into_iter().collect())),
        ]
    })
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        failure_persistence: None,
        ..Default::default()
    })]
    #[test]
    fn indexing_matches_full_strict_parsing_for_nested_payloads(value in payload()) {
        let db = schema();
        let text = ara_json::to_canonical_string(&json!({"$schema":"ara://schema/scene/1","id":"sc_12345678","records":value}));
        proptest::prop_assert_eq!(read_header(&text, &db), reference(&text, &db));
    }
}
