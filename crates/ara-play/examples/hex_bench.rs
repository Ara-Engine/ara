//! Optional portable-snapshot hex probe. Run with `cargo run -p ara-play --example hex_bench --release`.
use std::hint::black_box;
use std::time::Instant;

fn previous(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn measure(bytes: &[u8], encode: fn(&[u8]) -> String) -> serde_json::Value {
    for _ in 0..5 {
        black_box(encode(black_box(bytes)));
    }
    let mut samples = Vec::new();
    for _ in 0..16 {
        let start = Instant::now();
        let encoded = encode(black_box(bytes));
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        black_box(encoded);
        samples.push(ms);
    }
    let mut sorted = samples.clone();
    sorted.sort_by(f64::total_cmp);
    serde_json::json!({"p50_ms": sorted[8], "p95_ms": sorted[15], "samples_ms": samples})
}

fn main() {
    let mut rows = Vec::new();
    for size in [32, 1024 * 1024, 8 * 1024 * 1024] {
        let bytes = (0..size).map(|i| (i % 256) as u8).collect::<Vec<_>>();
        assert_eq!(previous(&bytes), ara_play::hex(&bytes));
        rows.push(serde_json::json!({"bytes": size, "before": measure(&bytes, previous), "after": measure(&bytes, ara_play::hex)}));
    }
    println!(
        "{}",
        serde_json::json!({"probe": "portable snapshot lowercase hex only; allocation included, output drop excluded", "rows": rows})
    );
}
