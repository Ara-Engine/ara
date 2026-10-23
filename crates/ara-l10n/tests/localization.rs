#![allow(clippy::unwrap_used)]
use ara_cmd::{Batch, Registry, Session};
use ara_doc::{AssetId, RecordId, Workspace};
use ara_l10n::{tools::*, *};
use ara_ops::{Store, TxnMeta};
use ara_schema::SchemaDb;
use serde_json::json;
use std::collections::BTreeMap;

fn entry(key: &str, text: &str, params: &[(&str, ParamKind)]) -> StringEntry {
    StringEntry {
        key: key.into(),
        text: text.into(),
        params: params.iter().map(|(n, k)| (n.to_string(), *k)).collect(),
        status: Status::Reviewed,
        ..Default::default()
    }
}
fn table(locale: &str, entries: Vec<StringEntry>) -> (SchemaDb, Table) {
    let mut db = SchemaDb::new();
    register_schema(&mut db);
    let id = AssetId::parse(if locale == "en" {
        "lc_english1"
    } else {
        "lc_arabic01"
    })
    .unwrap();
    let doc = Table::empty(id.clone());
    let mut workspace = Workspace::default();
    workspace.insert(format!("{locale}.l10n"), doc);
    let mut store = Store::new(db.clone(), workspace);
    let configure = TableCommand::Configure(LocaleConfig {
        locale: locale.into(),
        rtl: locale.starts_with("ar"),
        fonts: vec![FontFace {
            family: if locale.starts_with("ar") {
                "Noto Naskh Arabic".into()
            } else {
                "Noto Sans".into()
            },
            asset: None,
        }],
        ..Default::default()
    });
    let edits = configure
        .edits(store.workspace().doc(id.as_str()).unwrap())
        .unwrap();
    store
        .transact(TxnMeta::by(ara_ops::Author::Human), edits)
        .unwrap();
    for (index, entry) in entries.into_iter().enumerate() {
        let command = TableCommand::Put {
            id: RecordId::parse(&format!("m_{index:04}")).unwrap(),
            entry,
        };
        let edits = command
            .edits(store.workspace().doc(id.as_str()).unwrap())
            .unwrap();
        store
            .transact(TxnMeta::by(ara_ops::Author::Human), edits)
            .unwrap();
    }
    (
        db,
        Table::from_document(store.workspace().doc(id.as_str()).unwrap().clone()).unwrap(),
    )
}
fn num(value: &str) -> Arguments {
    Arguments::from([("n".into(), Argument::Number(value.parse().unwrap()))])
}

#[test]
fn arabic_six_plural_forms_follow_cldr_including_hundreds() {
    let message = Message::parse(
        "{n, plural, zero {zero} one {one} two {two} few {few} many {many} other {other}}",
        &BTreeMap::from([("n".into(), ParamKind::Number)]),
    )
    .unwrap();
    for n in 0..=350 {
        let expected = match n {
            0 => "zero",
            1 => "one",
            2 => "two",
            n if (3..=10).contains(&(n % 100)) => "few",
            n if (11..=99).contains(&(n % 100)) => "many",
            _ => "other",
        };
        assert_eq!(
            message.format("ar", "ar-EG", &num(&n.to_string())).unwrap(),
            expected,
            "n={n}"
        );
    }
    assert_eq!(message.format("ar", "ar", &num("1.5")).unwrap(), "other");
}

#[test]
fn nesting_exact_select_offset_ordinal_and_quoting_are_supported() {
    let signature = BTreeMap::from([
        ("n".into(), ParamKind::Number),
        ("gender".into(), ParamKind::Text),
    ]);
    let message = Message::parse("{gender, select, female {{n, plural, offset:1 =0 {none} one {She and one} other {She and #}}} other {{n, selectordinal, one {#st} two {#nd} few {#rd} other {#th}}}}", &signature).unwrap();
    let mut args = num("3");
    args.insert("gender".into(), Argument::Text("female".into()));
    assert_eq!(
        message.format("en", "en", &args).unwrap(),
        "She and \u{2068}2\u{2069}"
    );
    args.insert("gender".into(), Argument::Text("other".into()));
    assert_eq!(
        message.format("en", "en", &args).unwrap(),
        "\u{2068}3\u{2069}rd"
    );
    assert_eq!(
        Message::parse("You're '{safe}' and ''quoted''", &BTreeMap::new())
            .unwrap()
            .format("en", "en", &Arguments::new())
            .unwrap(),
        "You're {safe} and 'quoted'"
    );
}

#[test]
fn inserted_values_are_isolated_and_cannot_escape_isolation() {
    let message = Message::parse(
        "مرحبا {name}",
        &BTreeMap::from([("name".into(), ParamKind::Text)]),
    )
    .unwrap();
    let args = Arguments::from([(
        "name".into(),
        Argument::Text("Alice\u{2069}\u{202e}42".into()),
    )]);
    assert_eq!(
        message.format("ar", "ar", &args).unwrap(),
        "مرحبا \u{2068}Alice42\u{2069}"
    );
    assert!(message.format("ar", "ar", &Arguments::new()).is_err());
}

#[test]
fn fallback_uses_message_language_and_locale_number_preferences() {
    let (_, en) = table(
        "en",
        vec![
            entry(
                "items",
                "{n, plural, one {one item} other {# items}}",
                &[("n", ParamKind::Number)],
            ),
            entry("resume", "Resume", &[]),
        ],
    );
    let (_, ar) = table("ar", vec![entry("resume", "استئناف", &[])]);
    let catalog = Catalog::new("en", [en, ar]).unwrap();
    assert_eq!(
        catalog.fallback_chain("ar-EG").unwrap(),
        ["ar-EG", "ar", "en"]
    );
    assert_eq!(
        catalog.fallback_chain("ar-Arab-EG").unwrap(),
        ["ar-Arab-EG", "ar-Arab", "ar", "en"]
    );
    assert_eq!(
        catalog
            .format("ar-EG", "resume", &Arguments::new())
            .unwrap(),
        "استئناف"
    );
    assert_eq!(
        catalog.format("ar-EG", "items", &num("1")).unwrap(),
        "one item"
    );
    assert!(
        catalog
            .format("ar-EG", "items", &num("2"))
            .unwrap()
            .contains("٢")
    );
    assert_eq!(
        catalog
            .fonts("ar-EG")
            .unwrap()
            .iter()
            .map(|f| f.family.as_str())
            .collect::<Vec<_>>(),
        ["Noto Naskh Arabic", "Noto Sans"]
    );
}

#[test]
fn missing_extra_diagnostics_have_runnable_fixes_and_canonical_roundtrip() {
    let (db, en) = table("en", vec![entry("menu.resume", "Resume", &[])]);
    let (_, ar) = table("ar", vec![entry("menu.old", "قديم", &[])]);
    let text = ar.write(&db);
    assert_eq!(Table::read(&text, "ar.l10n", &db).unwrap().write(&db), text);
    let catalog = Catalog::new("en", [en, ar]).unwrap();
    let diagnostics = catalog.find_missing("ar").unwrap();
    assert_eq!(
        diagnostics.iter().map(|d| d.code).collect::<Vec<_>>(),
        [codes::MISSING_KEY, codes::EXTRA_KEY]
    );
    assert!(
        diagnostics
            .iter()
            .all(|d| !d.fixes.is_empty() && d.location.doc.is_some())
    );
}

#[test]
fn icu_number_date_and_currency_are_locale_aware() {
    let params = BTreeMap::from([
        ("n".into(), ParamKind::Number),
        ("d".into(), ParamKind::Date),
        ("price".into(), ParamKind::Currency),
    ]);
    let message = Message::parse(
        "{n, number} / {d, date, long} / {price, number, ::currency/USD}",
        &params,
    )
    .unwrap();
    let args = Arguments::from([
        ("n".into(), Argument::Number("1234.5".parse().unwrap())),
        (
            "d".into(),
            Argument::Date(Date::try_new_iso(2026, 10, 23).unwrap()),
        ),
        (
            "price".into(),
            Argument::Currency {
                amount: "12.5".parse().unwrap(),
                code: "USD".into(),
            },
        ),
    ]);
    let en = message.format("en", "en-US", &args).unwrap();
    let ar = message.format("ar", "ar-EG", &args).unwrap();
    assert!(
        en.contains("1,234.5") && en.contains("October") && en.contains("$12.50"),
        "{en}"
    );
    assert!(
        ar.contains("١") && ar.contains("أكتوبر") && ar.contains("US$"),
        "{ar}"
    );
}

#[test]
fn translator_roundtrips_preserve_icu_review_provenance_and_escaping() {
    let (_, en) = table(
        "en",
        vec![entry(
            "items",
            "{n, plural, one {<one> & item} other {# items}}",
            &[("n", ParamKind::Number)],
        )],
    );
    let mut translated = entry(
        "items",
        "{n, plural, one {عنصر & واحد} other {# عناصر}}",
        &[("n", ParamKind::Number)],
    );
    translated.status = Status::MachineTranslated;
    translated.provenance = Some(Provenance {
        provider: "local".into(),
        model: "v1".into(),
        job: "j1".into(),
        source_hash: en.entry("items").unwrap().source_hash(),
    });
    let (_, ar) = table("ar", vec![translated.clone()]);
    let xliff = export_xliff(&en, &ar).unwrap();
    let po = export_po(&en, &ar).unwrap();
    for units in [import_xliff(&xliff).unwrap(), import_po(&po).unwrap()] {
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].target, translated.text);
        assert_eq!(units[0].status, translated.status);
        assert_eq!(units[0].provenance, translated.provenance);
        assert_eq!(import_commands(&en, &ar, units).unwrap().len(), 1);
    }
    assert!(import_xliff("<!DOCTYPE x [<!ENTITY x SYSTEM 'file:///x'>]><xliff/>").is_err());
    assert!(import_po("msgctxt \"items\"\nmsgid_plural \"bad\"\n").is_err());
}

#[test]
fn reload_keeps_last_good_and_publishes_only_at_allowed_boundary() {
    let (_, en) = table("en", vec![entry("resume", "Resume", &[])]);
    let catalog = Catalog::new("en", [en.clone()]).unwrap();
    let mut live = LiveCatalog::new(catalog);
    assert!(
        live.stage(
            1,
            Err(ara_diag::Diagnostic::error(codes::TABLE, "bad edit"))
        )
        .is_err()
    );
    assert_eq!(
        live.current()
            .format("en", "resume", &Arguments::new())
            .unwrap(),
        "Resume"
    );
    live.stage(2, Catalog::new("en", [en])).unwrap();
    assert!(!live.apply_boundary(false));
    assert_eq!(live.generation(), 0);
    assert!(live.apply_boundary(true));
    assert_eq!(live.generation(), 2);
    let (_, updated) = table("en", vec![entry("resume", "Continue", &[])]);
    live.reload_table(3, Ok(updated)).unwrap();
    assert_eq!(
        live.current()
            .format("en", "resume", &Arguments::new())
            .unwrap(),
        "Resume"
    );
    live.apply_boundary(true);
    assert_eq!(
        live.current()
            .format("en", "resume", &Arguments::new())
            .unwrap(),
        "Continue"
    );
}

#[test]
fn generated_keys_have_typed_parameters_and_identifier_collisions_fail() {
    let (_, source) = table(
        "en",
        vec![entry(
            "inventory.items",
            "{n, plural, one {item} other {items}}",
            &[("n", ParamKind::Number)],
        )],
    );
    let generated = generate_keys(&source).unwrap();
    assert!(
        generated.rust.contains("Key<InventoryItemsParams>")
            && generated.rust.contains("pub r#n: Decimal")
    );
    assert!(generated.csharp.contains("InventoryItemsParams(decimal N)"));
    if let Some(directory) = std::env::var_os("ARA_L10N_CAPTURE_DIR") {
        let (_, all_types) = table(
            "en",
            vec![entry(
                "profile",
                "{name} {score, number} {date, date} {price, currency}",
                &[
                    ("name", ParamKind::Text),
                    ("score", ParamKind::Number),
                    ("date", ParamKind::Date),
                    ("price", ParamKind::Currency),
                ],
            )],
        );
        let generated = generate_keys(&all_types).unwrap();
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("keys.rs"),
            generated.rust,
        )
        .unwrap();
        std::fs::write(
            std::path::Path::new(&directory).join("Keys.cs"),
            generated.csharp,
        )
        .unwrap();
    }
    let (_, collision) = table("en", vec![entry("a.b", "a", &[]), entry("a_b", "b", &[])]);
    assert!(generate_keys(&collision).is_err());
}

#[test]
fn extraction_scans_visible_markup_and_csharp_with_reviewable_spans() {
    let markup = "<ui><label id='x' text='Hello &amp; goodbye'/><label>Resume</label></ui>";
    let extracted = extract(SourceKind::Markup, markup, "menu").unwrap();
    assert_eq!(extracted.len(), 2);
    assert_eq!(extracted[0].literal, "Hello & goodbye");
    assert_eq!(&markup[extracted[1].start..extracted[1].end], "Resume");
    let source = "// \"ignored\"\nvar a = \"Hello\\nworld\"; var b = @\"She said \"\"yes\"\"\";";
    let extracted = extract(SourceKind::Csharp, source, "menu").unwrap();
    assert_eq!(extracted.len(), 2);
    assert_eq!(extracted[1].literal, "She said \"yes\"");
    assert!(extract(SourceKind::Csharp, "var x = $\"Hello {name}\";", "menu").is_err());
}

#[test]
fn registry_extraction_translation_provenance_undo_and_stale_response() {
    let (mut db, en) = table("en", vec![entry("resume", "Resume", &[])]);
    let (_, ar) = table("ar", Vec::new());
    let mut registry = Registry::default();
    tools::register(&mut registry, &mut db);
    assert!(
        db.verify().iter().all(|d| !d.is_error()),
        "{:?}",
        db.verify()
    );
    assert!(
        registry.check_examples(&db).is_empty(),
        "{:?}",
        registry.check_examples(&db)
    );
    let request = prepare_translation(&en, &ar, "job1");
    let response = TranslationResponse {
        job: "job1".into(),
        provider: "test".into(),
        model: "v1".into(),
        translations: BTreeMap::from([("resume".into(), "استئناف".into())]),
    };
    let mut workspace = Workspace::default();
    workspace.insert("en.l10n", en.document().clone());
    workspace.insert("ar.l10n", ar.document().clone());
    let mut store = Store::new(db, workspace);
    let batch = Batch::new(
        ar.document().id.to_string(),
        [(
            "l10n.translate_job",
            json!({"request":request,"response":response}),
        )],
    );
    let result = registry
        .apply(&mut store, &batch, &Session::human())
        .unwrap();
    assert!(!result.ops.is_empty());
    let translated = Table::from_document(
        store
            .workspace()
            .doc(ar.document().id.as_str())
            .unwrap()
            .clone(),
    )
    .unwrap();
    assert_eq!(
        translated.entry("resume").unwrap().status,
        Status::MachineTranslated
    );
    assert_eq!(
        translated
            .entry("resume")
            .unwrap()
            .provenance
            .as_ref()
            .unwrap()
            .provider,
        "test"
    );
    assert!(
        registry
            .apply(&mut store, &batch, &Session::human())
            .is_err()
    );
    store.undo_last(&ara_ops::Author::Human).unwrap();
    assert!(
        Table::from_document(
            store
                .workspace()
                .doc(ar.document().id.as_str())
                .unwrap()
                .clone()
        )
        .unwrap()
        .entry("resume")
        .is_none()
    );
}

#[test]
fn pseudo_and_rtl_story_adapter_uses_actual_ui_test_harness() {
    let (_, en) = table("en", vec![entry("resume", "Resume", &[])]);
    let catalog = Catalog::new("en", [en]).unwrap();
    let mut story = ara_ui_stories::catalogue().remove(0);
    story.markup =
        "<ui id='root'><button id='resume' text='loc:resume' label='loc:resume'/></ui>".into();
    story.steps.clear();
    story.transitions.clear();
    let locale = catalog
        .story_locale("en", &BTreeMap::new(), Preview::RtlMirror)
        .unwrap();
    let expected = ara_ui_stories::localize("loc:resume", &locale).unwrap();
    assert!(locale.rtl && expected.starts_with('[') && expected.contains('~'));
    story.locales = vec![locale];
    story.views.truncate(1);
    story.views[0].locale = "en".into();
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db).unwrap();
    let mut harness = ara_ui_test::Harness::mount(&story, &story.views[0], &db).unwrap();
    harness
        .expect_text(&ara_ui_test::Query::test_id("resume"), &expected)
        .unwrap();
    if let Some(directory) = std::env::var_os("ARA_L10N_CAPTURE_DIR") {
        harness
            .capture()
            .unwrap()
            .write(std::path::Path::new(&directory), "pseudo-rtl")
            .unwrap();
    }
}

#[test]
fn arabic_mixed_text_shapes_in_the_native_harness() {
    let signature = [("name", ParamKind::Text), ("n", ParamKind::Number)];
    let (_, en) = table(
        "en",
        vec![entry(
            "greeting",
            "Hello {name}: {n, number} items",
            &signature,
        )],
    );
    let (_, ar) = table(
        "ar",
        vec![entry(
            "greeting",
            "مرحبا {name}: {n, number} عناصر",
            &signature,
        )],
    );
    let catalog = Catalog::new("en", [en, ar]).unwrap();
    let args = Arguments::from([
        ("name".into(), Argument::Text("Alice".into())),
        ("n".into(), Argument::Number("23".parse().unwrap())),
    ]);
    let locale = catalog
        .story_locale(
            "ar-EG",
            &BTreeMap::from([("greeting".into(), args)]),
            Preview::Native,
        )
        .unwrap();
    let expected = locale.messages["greeting"].clone();
    assert!(locale.rtl && expected.contains("٢٣"));
    let mut story = ara_ui_stories::catalogue().remove(0);
    story.markup = "<ui id='root'><text id='greeting' text='loc:greeting'/></ui>".into();
    story.stylesheet = "ui { padding: 24px; } text { font-size: 28px; }".into();
    story.steps.clear();
    story.transitions.clear();
    story.locales = vec![locale];
    story.views.truncate(1);
    story.views[0].locale = "ar-EG".into();
    let mut db = SchemaDb::new();
    ara_ui_stories::register(&mut db).unwrap();
    let mut harness = ara_ui_test::Harness::mount(&story, &story.views[0], &db).unwrap();
    harness
        .expect_text(&ara_ui_test::Query::test_id("greeting"), &expected)
        .unwrap();
    if let Some(directory) = std::env::var_os("ARA_L10N_CAPTURE_DIR") {
        harness
            .capture()
            .unwrap()
            .write(std::path::Path::new(&directory), "arabic-mixed")
            .unwrap();
    }
}
