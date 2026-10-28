//! Exercise the production boundary with a real shell and project authority.
use super::*;
use crate::shell::{Binding, commands::Target};
use serde_json::json;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        let root = std::env::var_os("CARGO_TARGET_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"))
            .join("panel-boundary-tests")
            .join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
        std::fs::create_dir_all(root.join("levels")).unwrap();
        for (path, value) in [
            (
                "ara.project.json",
                json!({"name":"Panel recovery","main":"levels/main.scene"}),
            ),
            (
                "levels/main.scene",
                json!({"$schema":"ara://schema/scene/1","id":"sc_boundary1","records":{}}),
            ),
        ] {
            std::fs::write(root.join(path), ara_json::to_canonical_string(&value)).unwrap();
        }
        Self(root)
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn panic_quarantines_only_its_panel_and_discards_partial_bindings() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
    let prior = shell.bindings.len();
    let mut markup = String::new();
    shell.with_panel_boundary("inspector", &mut editor, &mut markup, |shell, _, out| {
        out.push_str("<panel id='unfinished'>");
        shell
            .bindings
            .insert(123, Binding::Command("file.quit", Target::None));
        panic!("injected inspector failure <&>");
    });
    assert_eq!(shell.bindings.len(), prior + 1);
    assert!(!shell.bindings.contains_key(&123));
    assert!(!markup.contains("unfinished"));
    assert!(markup.contains("panel-error-inspector"));
    assert!(markup.contains("&lt;&amp;>"));

    // A quarantined panel must not panic or report again on every frame.
    shell.with_panel_boundary("inspector", &mut editor, &mut markup, |_, _, _| {
        panic!("quarantined renderer was called again");
    });
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.ui.find("body-hierarchy").is_some());
    let logs = editor
        .action("logs", json!({"codes":["ARA4500"],"group":true,"limit":20}))
        .unwrap();
    assert_eq!(logs["summary"]["occurrences"], 1);
    assert!(shell.retry_panel("inspector"));
    shell.rebuild(&mut editor).unwrap();
    shell.frame().unwrap();
    assert!(shell.ui.find("panel-error-inspector").is_none());
}

#[test]
fn returned_panel_error_uses_the_same_recovery_path() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [900., 600.]).unwrap();
    let mut markup = String::new();
    shell.with_panel_boundary("assets", &mut editor, &mut markup, |_, _, _| {
        Err("missing imported resource".into())
    });
    assert!(markup.contains("missing imported resource"));
    assert!(!shell.panel_failures["assets"].panicked);
    shell.with_panel_boundary("console", &mut editor, &mut markup, |_, _, out| {
        out.push_str("<text id='other-panel' text='still works' />");
        Ok(())
    });
    assert!(markup.contains("still works"));
}

#[test]
fn generated_markup_style_and_widget_errors_preserve_other_panels_and_bindings() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    let revision = editor.host.store().revision();
    for (index, (generated, reason)) in [
        (
            "<panel id='bad-style' style='align-self:stretch' />",
            "align-self",
        ),
        (
            "<slider id='bad-widget' min='0' max='1' value='NaN' />",
            "nonfinite",
        ),
        ("<panel id='unfinished'>", "ARA4001"),
        (
            "<panel id='repeat'><text id='repeat' /></panel>",
            "Duplicate",
        ),
        ("<text id='raw-color' style='color:#123456' />", "ARA4018"),
    ]
    .into_iter()
    .enumerate()
    {
        let previous = shell.bindings.clone();
        let partial = stable_id("invalid-panel-partial-binding");
        let mut markup = String::new();
        shell.with_panel_boundary("inspector", &mut editor, &mut markup, |shell, _, out| {
            out.push_str(generated);
            shell
                .bindings
                .insert(partial, Binding::Command("file.quit", Target::None));
            Ok(())
        });
        assert!(markup.contains("panel-error-inspector"), "{generated}");
        assert!(
            shell.panel_failures["inspector"].message.contains(reason),
            "{}",
            shell.panel_failures["inspector"].message
        );
        assert!(!shell.panel_failures["inspector"].panicked);
        assert!(!shell.bindings.contains_key(&partial));
        assert!(
            previous
                .iter()
                .all(|(key, value)| shell.bindings.get(key) == Some(value)),
            "Another panel's binding changed"
        );
        assert_eq!(shell.bindings.len(), previous.len() + 1);
        shell.with_panel_boundary("inspector", &mut editor, &mut markup, |_, _, _| {
            panic!("Quarantine must not continuously retry bad markup");
        });
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert!(shell.shown_node("body-hierarchy").is_some());
        assert!(shell.bindings.contains_key(&stable_id("search-hierarchy")));
        assert!(shell.shown_node("panel-error-inspector-retry").is_some());
        let logs = editor
            .action("logs", json!({"codes":["ARA4500"],"group":true,"limit":30}))
            .unwrap();
        assert_eq!(
            logs["summary"]["occurrences"],
            index + 1,
            "Failure must report once, not once per rebuild"
        );
        assert_eq!(editor.host.store().revision(), revision);
        assert!(shell.retry_panel("inspector"));
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert!(shell.shown_node("panel-error-inspector").is_none());
    }
}

#[test]
fn compilation_cache_checks_only_changed_panels_and_invalidates_with_surface_styles() {
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1000., 700.]).unwrap();
    shell.prepare_panel_validation(&editor).unwrap();
    shell.panel_compilation.valid.clear();
    let before = shell.panel_compilation.checks;
    let body = "<text id='panel-cached-value' text='First value' />";
    for _ in 0..20 {
        shell.validate_panel("inspector", body).unwrap();
    }
    assert_eq!(shell.panel_compilation.checks, before + 1);
    assert_eq!(shell.panel_compilation.valid.len(), 1);
    shell.prepare_panel_validation(&editor).unwrap();
    shell.validate_panel("inspector", body).unwrap();
    assert_eq!(
        shell.panel_compilation.checks,
        before + 1,
        "Identical stylesheet/theme preparation must preserve checks"
    );
    shell
        .validate_panel(
            "inspector",
            "<text id='panel-cached-value' text='New value' />",
        )
        .unwrap();
    assert_eq!(shell.panel_compilation.checks, before + 2);
    assert_eq!(
        shell.panel_compilation.valid.len(),
        1,
        "Only the current fingerprint is retained per panel"
    );
    editor
        .action("editor.theme", json!({"light":!editor.state.light}))
        .unwrap();
    shell.prepare_panel_validation(&editor).unwrap();
    assert!(shell.panel_compilation.valid.is_empty());
    shell.validate_panel("inspector", body).unwrap();
    assert_eq!(shell.panel_compilation.checks, before + 3);
    shell.ui.set_stylesheet("panel { gap:1px; }").unwrap();
    shell.prepare_panel_validation(&editor).unwrap();
    assert!(
        shell.panel_compilation.valid.is_empty(),
        "The prepared surface stylesheet changed"
    );
    shell.validate_panel("inspector", body).unwrap();
    assert_eq!(shell.panel_compilation.checks, before + 4);
}

#[test]
fn compiler_probes_keep_correlation_and_retry_memoized_panels() {
    let project = Project::new();
    std::fs::write(
        project.0.join(".ara-behaviour-fixture"),
        "isolated fixture\n",
    )
    .unwrap();
    let mut editor = Editor::open(&project.0).unwrap();
    editor
        .action("editor.layout.show", json!({"panel":"assets"}))
        .unwrap();
    let mut shell = Shell::new(&mut editor, [1100., 750.]).unwrap();
    shell.offscreen = true;
    for (panel, kind, reason) in [
        ("inspector", ProbeFailure::Style, "align-self"),
        ("assets", ProbeFailure::Widget, "nonfinite"),
    ] {
        let correlation = format!("native-compiler-probe-{panel}");
        shell
            .arm_panel_probe(&editor, panel, kind, &correlation)
            .unwrap();
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        let failure = &shell.panel_failures[panel];
        assert!(!failure.panicked);
        assert!(failure.message.contains(reason), "{}", failure.message);
        assert!(
            failure.message.contains(&correlation),
            "{}",
            failure.message
        );
        assert!(!shell.panel_probe_armed());
        assert!(
            shell
                .shown_node(&format!("panel-error-{panel}-retry"))
                .is_some()
        );
        assert!(shell.retry_panel(panel));
        assert!(!shell.memos.contains_key(panel));
        shell.rebuild(&mut editor).unwrap();
        shell.frame().unwrap();
        assert!(!shell.panel_failures.contains_key(panel));
        assert!(shell.shown_node(&format!("panel-error-{panel}")).is_none());
    }
}

/// Explicit CPU evidence only; ordinary tests never produce timing artifacts.
#[test]
#[ignore = "Run explicitly for panel validation cache-hit/miss CPU evidence"]
fn panel_compilation_cache_cpu_benchmark() {
    use std::{hint::black_box, time::Instant};
    let project = Project::new();
    let mut editor = Editor::open(&project.0).unwrap();
    let mut shell = Shell::new(&mut editor, [1440., 900.]).unwrap();
    shell.frame().unwrap();
    let nodes = shell.ui.nodes.len();
    let body = (0..500)
        .map(|i| {
            format!(
                "<field id='benchmark-{i}' text='Inspector property {i}' style='width:180px' />"
            )
        })
        .collect::<String>();
    let changed = body.replace("Inspector property 0", "Inspector property X");
    for _ in 0..16 {
        shell.validate_panel("inspector", &body).unwrap();
    }
    let samples = 500_u32;
    let overhead = Instant::now();
    for _ in 0..samples {
        black_box(&body);
    }
    let overhead = overhead.elapsed();
    let checks = shell.panel_compilation.checks;
    let hit = Instant::now();
    for _ in 0..samples {
        shell.validate_panel("inspector", black_box(&body)).unwrap();
    }
    let hit = hit.elapsed();
    let hit_checks = shell.panel_compilation.checks - checks;
    assert_eq!(hit_checks, 0);
    let uncached_samples = 64_u32;
    let checks = shell.panel_compilation.checks;
    let uncached = Instant::now();
    for _ in 0..uncached_samples {
        shell.panel_compilation.valid.remove("inspector");
        shell.validate_panel("inspector", black_box(&body)).unwrap();
    }
    let uncached = uncached.elapsed();
    let uncached_checks = shell.panel_compilation.checks - checks;
    assert_eq!(uncached_checks, uncached_samples as usize);
    let miss_samples = 64_u32;
    let checks = shell.panel_compilation.checks;
    let miss = Instant::now();
    for i in 0..miss_samples {
        shell
            .validate_panel(
                "inspector",
                black_box(if i % 2 == 0 { &changed } else { &body }),
            )
            .unwrap();
    }
    let miss = miss.elapsed();
    let miss_checks = shell.panel_compilation.checks - checks;
    assert_eq!(miss_checks, miss_samples as usize);
    let work = shell.ui.frame(&mut shell.text).unwrap();
    assert_eq!(shell.ui.nodes.len(), nodes);
    assert_eq!(work, ara_ui::Work::default());
    let timing = |elapsed: std::time::Duration, iterations: u32, checks: usize| {
        json!({
            "iterations":iterations,"totalNanoseconds":elapsed.as_nanos(),
            "nanosecondsPerCall":elapsed.as_secs_f64()*1e9/f64::from(iterations),"compilationChecks":checks
        })
    };
    let metadata = json!({"schemaVersion":2,"benchmark":"panel-compilation-cache","os":std::env::consts::OS,"architecture":std::env::consts::ARCH,"debugAssertions":cfg!(debug_assertions),"input":{"elements":501,"utf8Bytes":body.len(),"fingerprint":"blake3","stylesheet":"current editor surface"},
        "loopOverhead":timing(overhead,samples,0),"cacheHit":timing(hit,samples,hit_checks),
        "uncachedValidation":timing(uncached,uncached_samples,uncached_checks),"cacheMissAlternatingBodies":timing(miss,miss_samples,miss_checks),
        "uncachedNote":"Identical production validation with the current cache entry invalidated per call; invalidation cost is included. This is a counterfactual uncached mode, not a historical binary measurement.",
        "runtimeNodesCreated":shell.ui.nodes.len()-nodes,"layoutPasses":work.layout_passes,"textShapes":work.shaped});
    let output = crate::test_target().join("panel-validation-benchmarks");
    std::fs::create_dir_all(&output).unwrap();
    let path = output.join(format!(
        "{}-{}.json",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&path, ara_json::to_canonical_string(&metadata)).unwrap();
    println!("Panel validation benchmark: {}", path.display());
}
