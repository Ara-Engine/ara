//! Schema DB behaviour: derive output, dynamic types, validation, canonical form,
//! paths and upgrades.

use ara_diag::{Diagnostic, FixAction, codes};
use ara_schema::{AssetRef, FieldPath, Schema, SchemaDb, TypeDesc, Validator, path};
use serde_json::{Value, json};

/// Hit points of a damageable entity.
#[derive(Schema)]
#[ara(component, module = "game", version = 2, alias = "Hp", upgrade = upgrade_health)]
#[allow(dead_code)]
pub struct Health {
    /// Maximum hit points.
    #[ara(default = 100, range(min = 1, max = 10000), example = 150, replicate(audience = Relevant))]
    pub max: f32,
    /// Current hit points.
    #[ara(alias = "hp", replicate(audience = Relevant, quantize = 0.5, rollback), editor(slider))]
    pub current_value: f32,
    /// Regeneration delay after taking damage.
    #[ara(unit = Seconds, default = 2.5, since = 2, editor(group = "Regen", step = 0.1))]
    pub regen_delay: f32,
    /// Runtime-only bookkeeping.
    #[ara(skip)]
    pub scratch: u64,
}

/// Version 1 stored `max` as an integer percentage of 200.
fn upgrade_health(from: u32, v: &mut Value) -> Result<(), String> {
    if from == 1
        && let Some(pct) = v.get("max").and_then(Value::as_f64)
    {
        v["max"] = json!(pct * 2.0);
    }
    Ok(())
}

/// Rarity tier of loot.
#[derive(Schema)]
#[allow(dead_code)]
pub enum Rarity {
    /// Found everywhere.
    Common,
    /// Hard to find.
    #[ara(alias = "uncommon")]
    Rare,
    /// One per run.
    Legendary,
}

/// One inventory slot.
#[derive(Schema)]
#[allow(dead_code)]
pub struct Slot {
    /// Stable slot ID (`s_7c`).
    pub id: String,
    /// Item count.
    pub count: u32,
    /// Item rarity.
    pub rarity: Rarity,
}

/// Items carried by an entity.
#[derive(Schema)]
#[ara(component)]
#[allow(dead_code)]
pub struct Inventory {
    /// Slots, addressed by ID.
    #[ara(keyed)]
    pub slots: Vec<Slot>,
    /// Icon shown in the HUD.
    #[ara(asset_kind = "prefab")]
    pub icon: Option<AssetRef>,
}

/// Parameters of a test command.
#[derive(Schema)]
#[ara(params)]
#[allow(dead_code)]
pub struct DamageParams {
    /// Damage to apply.
    pub amount: f32,
    /// Damage multiplier.
    #[ara(default = 1)]
    pub scale: f32,
    /// Optional source label.
    pub source: Option<String>,
}

fn db() -> SchemaDb {
    let mut db = SchemaDb::new();
    db.register::<Health>()
        .register::<Inventory>()
        .register::<DamageParams>();
    let problems = db.verify();
    assert!(problems.is_empty(), "{problems:#?}");
    db
}

fn get<'a>(db: &'a SchemaDb, name: &str) -> &'a TypeDesc {
    db.get(name).expect("registered")
}

fn validate(db: &SchemaDb, ty: &str, v: Value) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let obj = v.as_object().expect("object").clone();
    Validator::new(db, &mut out).fields(get(db, ty), &obj, ty);
    out
}

#[test]
fn derive_produces_reflection() {
    let db = db();
    insta::assert_snapshot!(ara_json::to_canonical_string(&db.reflection()));
}

#[test]
fn reflection_round_trips_through_dynamic_types() {
    let rust = db();
    let mut file = rust.reflection();
    file["$schema"] = json!(ara_schema::TYPES_SCHEMA);
    file.as_object_mut()
        .expect("object")
        .shift_remove("documents");

    let mut dynamic = SchemaDb::new();
    let problems = dynamic.add_json_types(&file);
    assert!(problems.is_empty(), "{problems:#?}");
    assert!(dynamic.verify().is_empty());
    assert_eq!(
        ara_json::to_canonical_string(&dynamic.reflection()),
        ara_json::to_canonical_string(&rust.reflection())
    );
}

#[test]
fn dynamic_types_are_verified() {
    let mut db = SchemaDb::new();
    let problems = db.add_json_types(&json!({
        "$schema": "ara://schema/types/1",
        "types": {
            "Loot": {
                "kind": "component",
                "doc": "Dropped loot.",
                "fields": {
                    "weight": { "type": "f32", "doc": "", "range": [5, 1] },
                    "tier": { "type": "Tier", "doc": "Loot tier." }
                }
            },
            "Broken": { "kind": "component", "doc": "x", "fields": { "a": { "type": "float", "doc": "a" } } }
        }
    }));
    assert_eq!(problems.len(), 1, "{problems:#?}");
    assert!(problems[0].message.contains("unknown type `float`"));

    let messages: Vec<String> = db.verify().into_iter().map(|d| d.message).collect();
    assert_eq!(
        messages,
        [
            "`Loot`: field `weight` is missing its doc comment",
            "`Loot`: field `weight`: empty range [5, 1]",
            "`Loot`: field `tier` has unknown type `Tier`",
        ]
    );
}

#[test]
fn validation_reports_paths_and_fixes() {
    let db = db();
    let diags = validate(
        &db,
        "Health",
        json!({ "max": 0, "curentValue": 3, "regenDelay": "soon" }),
    );
    let summary: Vec<(String, String)> = diags
        .iter()
        .map(|d| {
            (
                d.code.to_string(),
                d.location.path.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("ARA1103".into(), "Health.max".into()),
            ("ARA1101".into(), "Health.curentValue".into()),
            ("ARA1102".into(), "Health.regenDelay".into()),
        ]
    );
    assert_eq!(
        diags[1].fixes[0].action,
        FixAction::Rename {
            record: None,
            path: "Health.curentValue".into(),
            to: "currentValue".into()
        }
    );

    let diags = validate(
        &db,
        "Inventory",
        json!({ "slots": [{ "id": "s_1", "rarity": "legendry" }, { "id": "s_1" }], "icon": "prefabs/x" }),
    );
    let found: Vec<_> = diags.iter().map(|d| d.code).collect();
    assert_eq!(
        found,
        [
            codes::UNKNOWN_VARIANT,
            codes::INVALID_KEYED_ITEM,
            codes::INVALID_REF
        ]
    );
    assert_eq!(
        diags[0].location.path.as_deref(),
        Some("Inventory.slots[s_1].rarity")
    );

    let diags = validate(&db, "DamageParams", json!({ "scale": 2 }));
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].code, codes::MISSING_REQUIRED);
}

#[test]
fn canonical_form_orders_fields_and_omits_defaults() {
    let db = db();
    let v = json!({ "regenDelay": 2.5, "currentValue": 7, "max": 100.0 });
    assert_eq!(
        db.canonical_value(&"Health".parse().expect("type"), &v),
        json!({ "currentValue": 7 })
    );

    let inv = json!({ "slots": [{ "rarity": "rare", "count": 0, "id": "s_1" }] });
    assert_eq!(
        ara_json::to_canonical_string(
            &db.canonical_value(&"Inventory".parse().expect("type"), &inv)
        ),
        "{\n  \"slots\": [\n    {\n      \"id\": \"s_1\",\n      \"rarity\": \"rare\"\n    }\n  ]\n}\n"
    );
}

#[test]
fn json_schema_for_a_component() {
    let db = db();
    insta::assert_snapshot!(ara_json::to_canonical_string(
        &db.type_json_schema(get(&db, "Inventory"))
    ));
}

#[test]
fn paths_resolve_and_materialize_defaults() {
    let db = db();
    let record = TypeDesc {
        name: "Thing".into(),
        doc: "A test record.".into(),
        module: String::new(),
        version: 1,
        aliases: Vec::new(),
        deprecated: None,
        shape: ara_schema::Shape::Struct {
            role: ara_schema::StructRole::Record(ara_schema::RecordInfo {
                prefix: "t".into(),
                components: true,
            }),
            fields: Vec::new(),
        },
        upgrade: None,
    };
    let p: FieldPath = "Inventory.slots[s_1].count".parse().expect("path");
    let resolved = db.resolve_path(&record, &p).expect("resolves");
    assert_eq!(resolved.ty, ara_schema::ValueType::U32);

    let mut rec = json!({ "Inventory": { "slots": [{ "id": "s_1" }] } })
        .as_object()
        .expect("object")
        .clone();
    let prev = path::set(&mut rec, &resolved.steps, Some(json!(4))).expect("set");
    assert_eq!(prev, None);
    assert_eq!(rec["Inventory"]["slots"][0]["count"], json!(4));

    let missing: FieldPath = "Health.max".parse().expect("path");
    let resolved = db.resolve_path(&record, &missing).expect("resolves");
    assert!(
        path::set(&mut rec, &resolved.steps, Some(json!(5))).is_err(),
        "components are never materialized"
    );

    for bad in [
        "Inventory.slots[s_1].cnt",
        "Inventry.slots",
        "Inventory.slots[s_1].count.x",
    ] {
        let err = db
            .resolve_path(&record, &bad.parse().expect("path"))
            .expect_err(bad);
        assert!(
            err.code == codes::INVALID_PATH || err.code == codes::UNKNOWN_TYPE,
            "{bad}: {err}"
        );
    }
}

#[test]
fn normalization_renames_and_upgrades() {
    let db = db();
    let mut v = json!({ "max": 60, "hp": 9 });
    let mut out = Vec::new();
    assert!(db.normalize_component(get(&db, "Health"), 1, &mut v, "Health", &mut out));
    assert_eq!(v, json!({ "max": 120.0, "currentValue": 9 }));
    let codes: Vec<_> = out.iter().map(|d| d.code).collect();
    assert_eq!(codes, [codes::UPGRADED, codes::RENAMED]);

    let mut out = Vec::new();
    assert!(!db.normalize_component(get(&db, "Health"), 3, &mut v, "Health", &mut out));
    assert_eq!(out[0].code, codes::NEWER_VERSION);
}
