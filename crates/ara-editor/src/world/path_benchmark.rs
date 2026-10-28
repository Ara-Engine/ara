//! Explicit CPU comparison of the pre-18df path check and current production method.
use super::*;
use serde_json::json;
use std::{hint::black_box, time::Instant};

const BASELINE: &str = "19701f3889c47638eb32589bdaa5d33e329868f3";
const CURRENT: &str = "production with per-call borrowed raw-reference deduplication";
const SAMPLES: usize = 16;
const ROUNDS: usize = 5;
const SOURCES: usize = 16;

// The exact previous method from 18df2fd3^, with only `self` renamed `view` so
// both implementations can run against the same immutable real DocumentView.
fn previous_missing_sources(
    view: &DocumentView,
    root: &Path,
    known: &BTreeSet<String>,
) -> Vec<(String, Option<String>)> {
    // One disk check per path, not per entity: scenes reuse a few models widely.
    let mut checked: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (id, entity) in view.entities.iter() {
        let Some(reference) = entity
            .record
            .get("MeshRenderer")
            .and_then(|r| r.get("model"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(path) = ara_schema::grammar::parse_ref(reference)
            .ok()
            .and_then(|r| r.path.map(str::to_owned))
        else {
            continue;
        };
        if !known.contains(&path) && !checked.contains_key(&path) {
            let missing = !root.join(&path).is_file();
            checked.insert(path, missing.then(|| id.clone()));
        }
    }
    checked.into_iter().collect()
}

fn fixture(count: usize) -> ara_ui::Result<(DocumentView, ara_doc::Workspace, BTreeSet<String>)> {
    let mut index = ara_doc::AssetIndex::new();
    let paths: Vec<_> = (0..SOURCES)
        .map(|n| format!("models/model-{n:02}.glb"))
        .collect();
    for (n, path) in paths.iter().enumerate() {
        index.insert(format!("as_bench{n:02}"), path, None);
    }
    let workspace = ara_doc::Workspace::new(index);
    assert!(
        workspace.reference_root().is_empty(),
        "Compare correct legacy behavior"
    );
    let scene = ara_doc::AssetId::parse("sc_pathbench")
        .map_err(|error| format!("Benchmark scene: {error}"))?;
    let mut entities = Resolved::new();
    for n in 0..count {
        let id = format!("e_path{n:06}");
        let source = n % SOURCES;
        let record =
            json!({"MeshRenderer":{"model":format!("@{}#as_bench{source:02}",paths[source])}})
                .as_object()
                .ok_or("Benchmark record missing")?
                .clone();
        entities.insert(
            id.clone(),
            ara_doc::prefab::ResolvedEntity {
                record,
                parent: None,
                order: None,
                source: (
                    scene.clone(),
                    ara_doc::RecordId::parse(&id)
                        .map_err(|error| format!("Benchmark record: {error}"))?,
                ),
            },
        );
    }
    Ok((
        DocumentView::with_imports(Arc::new(entities), Arc::new(Imports::new())),
        workspace,
        paths.into_iter().collect(),
    ))
}

fn time(mut call: impl FnMut() -> Vec<(String, Option<String>)>) -> u128 {
    let start = Instant::now();
    for _ in 0..SAMPLES {
        black_box(call());
    }
    start.elapsed().as_nanos()
}
fn median(values: &[u128; ROUNDS]) -> u128 {
    let mut sorted = *values;
    sorted.sort_unstable();
    sorted[ROUNDS / 2]
}

#[test]
#[ignore = "Explicit missing-source CPU benchmark; writes canonical timing evidence under target"]
fn missing_sources_cached_references_cpu_benchmark() -> ara_ui::Result<()> {
    let production_source = blake3::hash(include_str!("mod.rs").as_bytes())
        .to_hex()
        .to_string();
    let output = crate::test_target().join("editor-path-benchmarks");
    let root = output.join(format!("no-source-files-{}", std::process::id()));
    assert!(
        !root.exists(),
        "Known-reference benchmark must need no source files"
    );
    let started = Instant::now();
    let mut results = Vec::new();
    for count in [1_000, 10_000] {
        let (view, workspace, known) = fixture(count)?;
        let previous = previous_missing_sources(&view, &root, &known);
        let current = view.missing_sources(&root, &workspace, &known);
        assert_eq!(
            previous, current,
            "Benchmark implementations differ on the legacy fixture"
        );
        assert!(current.is_empty());
        for _ in 0..4 {
            black_box(previous_missing_sources(
                black_box(&view),
                black_box(&root),
                black_box(&known),
            ));
            black_box(view.missing_sources(
                black_box(&root),
                black_box(&workspace),
                black_box(&known),
            ));
        }
        let mut previous = [0_u128; ROUNDS];
        let mut current = [0_u128; ROUNDS];
        for round in 0..ROUNDS {
            let baseline = || {
                time(|| {
                    previous_missing_sources(black_box(&view), black_box(&root), black_box(&known))
                })
            };
            let production = || {
                time(|| {
                    view.missing_sources(black_box(&root), black_box(&workspace), black_box(&known))
                })
            };
            if round % 2 == 0 {
                previous[round] = baseline();
                current[round] = production();
            } else {
                current[round] = production();
                previous[round] = baseline();
            }
        }
        assert_eq!(
            previous_missing_sources(&view, &root, &known),
            view.missing_sources(&root, &workspace, &known)
        );
        let per_call = |rounds: [u128; ROUNDS]| rounds.map(|ns| ns / SAMPLES as u128);
        let baseline = median(&previous) / SAMPLES as u128;
        let production = median(&current) / SAMPLES as u128;
        results.push(json!({"entities":count,"uniqueSources":SOURCES,"knownSources":known.len(),"equalOutputs":true,"sourceFilesRequired":false,
            "baseline":{"commit":BASELINE,"nanosecondsPerCall":per_call(previous),"medianNanosecondsPerCall":baseline},
            "current":{"implementation":CURRENT,"sourceModule":"crates/ara-editor/src/world/mod.rs","sourceDigestAlgorithm":"blake3","sourceDigest":production_source,"nanosecondsPerCall":per_call(current),"medianNanosecondsPerCall":production},
            "currentOverBaseline":production as f64/baseline.max(1) as f64}));
    }
    assert!(
        !root.exists(),
        "Measured work must leave the absent source tree untouched"
    );
    let report = json!({"schema":"ara.editor.missing-sources-benchmark/1","os":std::env::consts::OS,"architecture":std::env::consts::ARCH,
        "debugAssertions":cfg!(debug_assertions),"fixture":"correct legacy DocumentView; repeated known logical references with stable asset IDs",
        "warmupCallsPerArm":4,"samplesPerRound":SAMPLES,"interleavedRounds":ROUNDS,"order":"baseline first on even rounds, current first on odd rounds",
        "timing":"monotonic wall nanoseconds per call; input setup/assertions/artifact I/O excluded","elapsedMilliseconds":started.elapsed().as_secs_f64()*1000.,"results":results});
    std::fs::create_dir_all(&output)?;
    let artifact = output.join(format!("missing-sources-{}.json", std::process::id()));
    std::fs::write(&artifact, ara_json::to_canonical_string(&report))?;
    println!("Missing-source CPU evidence: {}", artifact.display());
    Ok(())
}
