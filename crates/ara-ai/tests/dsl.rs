#![allow(clippy::unwrap_used)]
//! Text round trips, identity reconciliation, graph projection and checking.
use ara_ai::{ast::*, codes, compile, dsl, projection};

fn source(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/behaviors/{name}.behavior",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn examples_parse_compile_and_print_canonically() {
    for name in ["guard", "soldier"] {
        let b = dsl::parse(&source(name), "gr_example1").unwrap();
        let program = compile(&b).unwrap();
        assert!(!program.states.is_empty());
        let printed = dsl::print(&b);
        let again = dsl::parse(&printed, "gr_example1").unwrap();
        assert_eq!(again, b, "{name} round trip");
        assert_eq!(
            dsl::print(&again),
            printed,
            "{name} canonical text is stable"
        );
        assert!(!printed.contains("n_"), "no IDs in text:\n{printed}");
    }
}

#[test]
fn canonical_printing_minimises_parentheses_and_keeps_meaning() {
    let text = "behavior T { var a: float = (1 + 2) * 3 - -4; var b: bool = not (true and false) or 1 < 2; state S { do wait(a / (2 * 1)); } }";
    let b = dsl::parse(text, "gr_t0000001").unwrap();
    let printed = dsl::print(&b);
    assert!(
        printed.contains("var a: float = (1 + 2) * 3 - -4;"),
        "{printed}"
    );
    assert!(
        printed.contains("var b: bool = not (true and false) or 1 < 2;"),
        "{printed}"
    );
    assert!(printed.contains("do wait(a / (2 * 1));"), "{printed}");
    assert_eq!(dsl::parse(&printed, "gr_t0000001").unwrap(), b);
}

#[test]
fn reconcile_keeps_ids_by_name_and_offers_rename_recovery() {
    let b = dsl::parse(&source("guard"), "gr_guard001").unwrap();
    let chase = b
        .all_states()
        .iter()
        .find(|s| s.name == "Chase")
        .unwrap()
        .id
        .clone();
    let edited = dsl::print(&b).replace("speed: 3,", "speed: 4,");
    let r = dsl::reconcile(&edited, &b).unwrap();
    assert!(r.diagnostics.is_empty());
    assert_eq!(
        r.behavior
            .all_states()
            .iter()
            .find(|s| s.name == "Chase")
            .unwrap()
            .id,
        chase
    );
    // A plain-text rename creates a new identity and suggests the recovery command.
    let renamed = dsl::print(&b).replace("state Chase", "state Pursue");
    let r = dsl::reconcile(&renamed, &b).unwrap();
    assert_ne!(
        r.behavior
            .all_states()
            .iter()
            .find(|s| s.name == "Pursue")
            .unwrap()
            .id,
        chase
    );
    let d = &r.diagnostics[0];
    assert_eq!(d.code, codes::PLAIN_RENAME);
    let ara_diag::FixAction::Command { command, params } = &d.fixes[0].action else {
        panic!("expected a command fix");
    };
    assert_eq!(command, "behavior.rename");
    assert_eq!(params["restoreId"], chase.as_str());
}

#[test]
fn graph_projection_round_trips_with_stable_ids() {
    let b = dsl::parse(&source("soldier"), "gr_soldier1").unwrap();
    let g = projection::to_graph(&b, Default::default()).unwrap();
    assert_eq!(g.header.domain, projection::DOMAIN);
    assert_eq!(
        g.nodes.len(),
        b.vars.len() + b.queries.len() + b.all_states().len()
    );
    let back = projection::from_graph(&g).unwrap();
    assert_eq!(back, b);
}

#[test]
fn syntax_errors_have_positions_and_check_errors_name_the_problem() {
    let e = dsl::parse(
        "behavior X {\n  state S {\n    do wait(1)\n  }\n}",
        "gr_x0000001",
    )
    .unwrap_err();
    assert_eq!(e.code, codes::SYNTAX);
    assert_eq!((e.location.line, e.location.column), (Some(4), Some(3)));
    let e = dsl::parse("behavior X { var state: float; }", "gr_x0000001").unwrap_err();
    assert!(e.message.contains("keyword"), "{}", e.message);

    let b = dsl::parse(
        "behavior X {
           var hp: float = true;
           state A { do moveTo(hp); on sees(heading) -> Nowhere; do jump(); score distnce(self, self); }
         }",
        "gr_x0000001",
    )
    .unwrap();
    let errors = compile(&b).unwrap_err();
    let text: Vec<String> = errors
        .iter()
        .map(|d| format!("{} {}", d.code, d.message))
        .collect();
    let has = |code: ara_diag::Code, needle: &str| {
        errors
            .iter()
            .any(|d| d.code == code && d.message.contains(needle))
    };
    assert!(
        has(codes::TYPE, "`hp` is float but its default is bool"),
        "{text:#?}"
    );
    assert!(has(codes::TYPE, "moveTo target"), "{text:#?}");
    assert!(has(codes::NAME, "unknown variable `heading`"), "{text:#?}");
    assert!(has(codes::TARGET, "unknown state `Nowhere`"), "{text:#?}");
    assert!(has(codes::TARGET, "unknown task `jump`"), "{text:#?}");
    assert!(has(codes::NAME, "did you mean `distance`"), "{text:#?}");
}

#[test]
fn types_flow_through_operators_and_overrides_are_constants() {
    let b = dsl::parse(
        "behavior X {
           var p: vec3 = vec3(1, 0, 2) * 2 + vec3(0, 1, 0);
           var route: points = [vec3(0, 0, 0), vec3(4, 0, 0)];
           var ok: bool = distance(p, self) > 1 and not isSet(route);
           state A { do patrol(route); }
         }",
        "gr_x0000001",
    )
    .unwrap();
    let program = compile(&b).unwrap();
    assert_eq!(program.slots.len(), 3);
    assert_eq!(program.slots[0].ty, Type::Vec3);
}
