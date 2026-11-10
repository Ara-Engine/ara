//! Public diagnostics retain exact traversal order, paths, messages and fixes.
use ara_diag::{Diagnostic, FixAction, Location, codes};
use ara_schema::{SchemaDb, Validator, ValueType};
use serde_json::json;

fn schema() -> SchemaDb {
    let mut db = SchemaDb::new();
    let problems = db.add_json_types(&json!({"$schema":"ara://schema/types/1","types":{
        "Phase":{"kind":"enum","doc":"Phase.","variants":{
            "open":{"doc":"Open.","aliases":["old"]},
            "legacy":{"doc":"Legacy.","deprecated":"use open"}
        }},
        "Leaf":{"kind":"params","doc":"Nested input.","fields":{
            "id":{"type":"string","doc":"Stable item identity."},
            "amount":{"type":"f32","doc":"Amount.","range":[0,10],"aliases":["oldAmount"],"deprecated":"use total"},
            "state":{"type":"Phase","doc":"State."},
            "tag":{"type":"string","doc":"Required tag.","required":true}
        }},
        "Probe":{"kind":"component","doc":"Container.","deprecated":"replace probe","fields":{
            "children":{"type":"list<Leaf>","doc":"List."},
            "slots":{"type":"keyed<Leaf>","doc":"Keyed list."},
            "groups":{"type":"map<list<Leaf>>","doc":"Named lists."},
            "optional":{"type":"option<Leaf>","doc":"Optional input.","deprecated":"replace optional"},
            "vector":{"type":"vec2","doc":"Vector.","range":[0,1]}
        }}
    }}));
    assert!(problems.is_empty(), "{problems:?}");
    db
}

fn base() -> Location {
    Location {
        file: Some("levels/é.scene".into()),
        doc: Some("sc_test".into()),
        record: Some("e_test".into()),
        path: Some("fallback".into()),
        command: Some(7),
        ..Default::default()
    }
}

fn renamed(path: &str) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_FIELD,
        "field `oldAmount` of `Leaf` was renamed",
    )
    .path(path)
    .with_fix(
        "Rename `oldAmount` to `amount`",
        FixAction::Rename {
            record: None,
            path: path.into(),
            to: "amount".into(),
        },
    )
}

fn phase_alias(path: &str) -> Diagnostic {
    Diagnostic::error(
        codes::UNKNOWN_VARIANT,
        "`old` is not a `Phase`; expected one of: open, legacy",
    )
    .path(path)
    .with_fix(
        "Use `open`",
        FixAction::Set {
            record: None,
            path: path.into(),
            value: json!("open"),
        },
    )
}

fn deprecated_amount(path: &str) -> Diagnostic {
    Diagnostic::warning(codes::DEPRECATED, "field `amount` is deprecated: use total").path(path)
}

fn missing(path: &str) -> Diagnostic {
    Diagnostic::error(
        codes::MISSING_REQUIRED,
        "missing required field `tag` of `Leaf`",
    )
    .path(path)
}

#[test]
fn nested_containers_keep_diagnostic_bytes_order_locations_and_fixes() {
    let db = schema();
    let input = json!({
        "children":[
            {"amount":-1,"state":"old","oldAmount":2},
            {"tag":"ok","amount":2,"state":"legacy"},
            false
        ],
        "slots":[{"id":"s_a","tag":"ok","amount":11},
            {"id":"s_a","amount":20},{"id":"bad!","tag":"ok"},{"tag":"ok"}],
        "groups":{"café.key]":[{"tag":"ok","state":"old"}],
            "next":[{"tag":"ok","amount":"bad"}]},
        "optional":{"oldAmount":3},
        "vector":[-1,2]
    });
    let mut actual = Vec::new();
    Validator::new(&db, &mut actual)
        .at(base())
        .component("Probe", &input);
    let expected = vec![
        Diagnostic::warning(
            codes::DEPRECATED,
            "component `Probe` is deprecated: replace probe",
        )
        .path("Probe"),
        deprecated_amount("Probe.children[#0].amount"),
        Diagnostic::error(codes::OUT_OF_RANGE, "-1 is outside the range [0, 10]")
            .path("Probe.children[#0].amount"),
        phase_alias("Probe.children[#0].state"),
        renamed("Probe.children[#0].oldAmount"),
        missing("Probe.children[#0].tag"),
        deprecated_amount("Probe.children[#1].amount"),
        Diagnostic::warning(codes::DEPRECATED, "`Phase.legacy` is deprecated: use open")
            .path("Probe.children[#1].state"),
        Diagnostic::error(
            codes::TYPE_MISMATCH,
            "expected a `Leaf` object, got a boolean",
        )
        .path("Probe.children[#2]"),
        deprecated_amount("Probe.slots[s_a].amount"),
        Diagnostic::error(codes::OUT_OF_RANGE, "11 is outside the range [0, 10]")
            .path("Probe.slots[s_a].amount"),
        Diagnostic::error(codes::INVALID_KEYED_ITEM, "item id `s_a` is used twice")
            .path("Probe.slots[s_a]"),
        Diagnostic::error(
            codes::INVALID_KEYED_ITEM,
            "keyed items need an `id` like `s_7c`",
        )
        .path("Probe.slots"),
        Diagnostic::error(
            codes::INVALID_KEYED_ITEM,
            "keyed items need an `id` like `s_7c`",
        )
        .path("Probe.slots"),
        phase_alias("Probe.groups[café.key]][#0].state"),
        deprecated_amount("Probe.groups[next][#0].amount"),
        Diagnostic::error(
            codes::TYPE_MISMATCH,
            "expected a number (`f32`), got a string",
        )
        .path("Probe.groups[next][#0].amount"),
        Diagnostic::warning(
            codes::DEPRECATED,
            "field `optional` is deprecated: replace optional",
        )
        .path("Probe.optional"),
        Diagnostic::warning(
            codes::DEPRECATED,
            "field `optional` is deprecated: replace optional",
        )
        .path("Probe.optional"),
        renamed("Probe.optional.oldAmount"),
        missing("Probe.optional.tag"),
        Diagnostic::error(
            codes::OUT_OF_RANGE,
            "component 0 (-1) is outside the range [0, 1]",
        )
        .path("Probe.vector"),
        Diagnostic::error(
            codes::OUT_OF_RANGE,
            "component 1 (2) is outside the range [0, 1]",
        )
        .path("Probe.vector"),
    ]
    .into_iter()
    .map(|d| d.within(&base()))
    .collect::<Vec<_>>();
    assert_eq!(actual, expected);
    assert_eq!(
        serde_json::to_vec(&actual).expect("diagnostics"),
        serde_json::to_vec(&expected).expect("diagnostics")
    );
}

#[test]
fn empty_roots_and_independent_calls_keep_their_own_paths() {
    let db = schema();
    let ty: ValueType = "list<map<option<Leaf>>>".parse().expect("type");
    let mut actual = Vec::new();
    let mut validator = Validator::new(&db, &mut actual).at(base()).allow_symbols();
    validator.value(&ty, None, &json!([{"é":{"oldAmount":2}},{"next":null}]), "");
    validator.value(&ValueType::Bool, None, &json!(null), "");
    validator.value(&ValueType::Entity, None, &json!("$c1"), "Symbol");
    let valid = json!({"children":[{"tag":"ok","state":"open"}],
        "slots":[{"id":"s_1","tag":"ok"},{"id":"s_2","tag":"ok"}],
        "groups":{"é":[{"tag":"ok"}]},"vector":[0,1]});
    validator.value(&ValueType::Named("Probe".into()), None, &valid, "Separate");
    let expected = vec![
        renamed("[#0][é].oldAmount"),
        missing("[#0][é].tag"),
        Diagnostic::error(codes::TYPE_MISMATCH, "expected a boolean, got null"),
    ]
    .into_iter()
    .map(|d| d.within(&base()))
    .collect::<Vec<_>>();
    assert_eq!(actual, expected);
}
