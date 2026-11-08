//! CPU-only document-index probe: `cargo run -p ara-doc --example read_header_bench`.
use ara_doc::{AssetId, read_header};
use ara_schema::SchemaDb;
use serde_json::{Map, Value, json};
use std::{hint::black_box, time::Instant};

fn reference(text: &str, db: &SchemaDb) -> Option<(String, AssetId)> {
    let root: Map<String, Value> = ara_json::parse(text).ok()?.as_object()?.clone();
    let (kind, _) = db.kind_for_schema(root.get("$schema")?.as_str()?).ok()?;
    Some((
        kind.name.clone(),
        AssetId::parse(root.get("id")?.as_str()?).ok()?,
    ))
}

fn measure(name: &str, text: &str, run: impl Fn(&str) -> Option<(String, AssetId)>) {
    for _ in 0..8 {
        black_box(run(text));
    }
    let mut samples = Vec::with_capacity(40);
    for _ in 0..40 {
        let start = Instant::now();
        black_box(run(text));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    println!(
        "{}",
        json!({"case":name, "bytes":text.len(), "p50_ms":samples[20], "p95_ms":samples[37], "samples_ms":samples})
    );
}

fn main() {
    let mut schema = SchemaDb::new();
    ara_doc::register(&mut schema);
    let reverse = std::env::args().any(|arg| arg == "--reverse");
    for count in [32, 10_000] {
        let records: Map<String, Value> = (0..count).map(|i| (format!("e_{i:08}"), json!({
            "name":format!("Object {i}"),
            "Transform":{"position":[i as f32 * 0.01,1.0,2.0],"rotation":[0,45,0],"scale":[1,1,1]},
            "MeshRenderer":{"model":"@models/large.glb#as_12345678","tint":[1,0.5,0.25,1]}
        }))).collect();
        let text = ara_json::to_canonical_string(
            &json!({"$schema":"ara://schema/scene/1", "id":"sc_12345678", "records":records}),
        );
        assert_eq!(read_header(&text, &schema), reference(&text, &schema));
        for baseline in if reverse {
            [false, true]
        } else {
            [true, false]
        } {
            let name = format!(
                "header_{count}_{}",
                if baseline { "reference" } else { "current" }
            );
            if baseline {
                measure(&name, &text, |text| reference(text, &schema));
            } else {
                measure(&name, &text, |text| read_header(text, &schema));
            }
        }
    }
}
