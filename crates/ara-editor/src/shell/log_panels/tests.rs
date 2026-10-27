//! Console identity must preserve unrelated failures sharing a diagnostic code.
use super::*;
use std::path::PathBuf;

fn entry(message: &str, file: &str, line: u32, origin: &str) -> Entry {
    let mut diagnostic = ara_diag::Diagnostic::error(ara_diag::codes::INVALID_PARAMS, message);
    diagnostic.location.file = Some(file.into());
    diagnostic.location.line = Some(line);
    Entry {
        seq: 1,
        count: 1,
        at: 0,
        origin: origin.into(),
        diagnostic,
    }
}

#[test]
fn collapse_identity_distinguishes_message_source_line_and_origin() {
    let base = entry("Wrong argument", "assets/a.cs", 12, "script");
    let repeat = entry("Wrong argument", "assets/a.cs", 12, "script");
    assert_eq!(base.identity(), repeat.identity());
    for different in [
        entry("Missing reference", "assets/a.cs", 12, "script"),
        entry("Wrong argument", "assets/b.cs", 12, "script"),
        entry("Wrong argument", "assets/a.cs", 13, "script"),
        entry("Wrong argument", "assets/a.cs", 12, "agent"),
    ] {
        assert_ne!(base.identity(), different.identity());
    }
}

#[test]
#[ignore = "microbenchmark writes before/after grouping cost under target"]
fn console_collapse_identity_benchmark() -> ara_ui::Result<()> {
    let rows: Vec<_> = (0..500)
        .map(|i| {
            entry(
                if i % 3 == 0 {
                    "Missing target"
                } else {
                    "Invalid value"
                },
                if i % 5 == 0 {
                    "assets/a.cs"
                } else {
                    "assets/b.cs"
                },
                i % 17,
                "script",
            )
        })
        .collect();
    let before = std::time::Instant::now();
    let mut before_groups = 0;
    for _ in 0..1000 {
        let mut counts = BTreeMap::new();
        for row in &rows {
            *counts
                .entry(row.diagnostic.code.to_string())
                .or_insert(0u64) += row.count;
        }
        before_groups = std::hint::black_box(counts.len());
    }
    let before_ms = before.elapsed().as_secs_f64() * 1000.;
    let after = std::time::Instant::now();
    let mut after_groups = 0;
    for _ in 0..1000 {
        let mut counts = BTreeMap::new();
        for row in &rows {
            *counts.entry(row.identity()).or_insert(0u64) += row.count;
        }
        after_groups = std::hint::black_box(counts.len());
    }
    let report = json!({"rows":rows.len(),"iterations":1000,"beforeMs":before_ms,"afterMs":after.elapsed().as_secs_f64()*1000.,"beforeGroups":before_groups,"afterGroups":after_groups});
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
    let output = target.join("ux-p4/console-collapse-benchmark.json");
    std::fs::create_dir_all(output.parent().ok_or("benchmark output has no parent")?)?;
    std::fs::write(output, ara_json::to_canonical_string(&report))?;
    println!("{report}");
    Ok(())
}
