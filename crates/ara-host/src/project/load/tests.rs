use super::*;
use serde_json::json;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT: AtomicU64 = AtomicU64::new(0);

fn scratch() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/project-load-tests")
        .join(format!(
            "{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
    fs::create_dir_all(path.join("content")).expect("fixture directory");
    fs::write(path.join("ara.project.json"), r#"{"layoutVersion":4}"#).expect("project marker");
    path
}

fn scene(id: usize, name: &str) -> String {
    ara_json::to_canonical_string(
        &json!({"$schema":"ara://schema/scene/1","id":format!("sc_{id:08}"),"records":{
            "e_root":{"name":name,"Transform":{"position":[1,2,3]}}
        }}),
    )
}

#[test]
fn parallel_reads_keep_native_formats_and_identical_ordered_diagnostics() {
    let root = scratch();
    for i in 0..32 {
        let (extension,text) = match i%3 {
            0 => ("scene",scene(i,"Valid scene")),
            1 => ("scene",r#"{"$schema":"ara://schema/scene/1","id":"sc_12345678","records":{"e_root":{"name":"a","name":"b"}}}"#.to_owned()),
            _ => ("ui",format!("<ui id='root'><text id='label' text='Menu {i}'/></ui>")),
        };
        fs::write(root.join(format!("content/doc{i:02}.{extension}")), text).expect("fixture file");
    }
    let project = Project::open(&root).expect("project opens");
    fs::remove_file(root.join("content/doc00.scene")).expect("missing reader input");
    let inputs: Vec<_> = project
        .docs
        .iter()
        .map(|file| Input {
            file,
            relative: project.rel(file),
            project: project.project_files.contains(file),
        })
        .collect();
    let expected: Vec<_> = inputs
        .iter()
        .map(|input| read_input(input, &project.db, &project.index))
        .collect();
    let actual = read_parallel(&inputs, &project.db, &project.index, 4);
    assert_eq!(actual.len(), expected.len());
    for (expected, actual) in expected.into_iter().zip(actual) {
        assert_eq!(actual.diags, expected.diags);
        assert_eq!(
            actual.doc.as_ref().map(|doc| project.write_document(doc)),
            expected.doc.as_ref().map(|doc| project.write_document(doc))
        );
    }
}

#[test]
fn public_loading_keeps_fresh_bytes_first_occurrences_and_workspace_overwrite_order() {
    let root = scratch();
    let padding = "x".repeat(8192);
    for i in 0..64 {
        let id = if i == 63 { 0 } else { i };
        fs::write(
            root.join(format!("content/doc{i:02}.scene")),
            scene(id, &padding),
        )
        .expect("fixture scene");
    }
    let project = Project::open(&root).expect("project opens");
    fs::write(
        root.join("content/doc63.scene"),
        scene(0, "Latest disk bytes"),
    )
    .expect("external edit after indexing");
    fs::remove_file(root.join("content/doc61.scene")).expect("removed input");
    fs::write(root.join("content/doc62.scene"), "{").expect("invalid external input");
    let extra = [
        root.join("content/./doc00.scene"),
        root.join("content/doc63.scene"),
    ];
    let (workspace, diagnostics) = documents(&project, &extra);
    let doc = workspace
        .doc("sc_00000000")
        .expect("last distinct file wins");
    assert_eq!(
        doc.record("e_root").expect("entity")["name"],
        "Latest disk bytes"
    );
    assert_eq!(
        workspace.path_of("sc_00000000"),
        Some("content/doc63.scene")
    );
    assert!(
        diagnostics["content/doc61.scene"]
            .iter()
            .any(|d| d.code == codes::IO)
    );
    assert!(
        diagnostics["content/doc62.scene"]
            .iter()
            .any(|d| d.code == codes::JSON_SYNTAX)
    );
    assert_eq!(diagnostics.len(), project.docs.len());
}

#[test]
fn tiny_batches_and_many_small_files_stay_on_the_caller_thread() {
    let root = scratch();
    for i in 0..32 {
        fs::write(
            root.join(format!("content/doc{i:02}.scene")),
            scene(i, "Small"),
        )
        .expect("fixture scene");
    }
    let project = Project::open(&root).expect("project opens");
    let inputs: Vec<_> = project
        .docs
        .iter()
        .map(|file| Input {
            file,
            relative: project.rel(file),
            project: project.project_files.contains(file),
        })
        .collect();
    assert_eq!(worker_count(&inputs[..4]), 1);
    assert_eq!(worker_count(&inputs), 1);
}
