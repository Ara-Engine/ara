//! Compare the original telemetry format with the production buffered writer.
use super::*;

#[derive(Default)]
struct Sink {
    bytes: Vec<u8>,
    writes: usize,
    flushes: usize,
    fail_once: bool,
}
impl Write for Sink {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.writes += 1;
        if std::mem::take(&mut self.fail_once) {
            return Err(std::io::Error::other("injected telemetry output error"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.flushes += 1;
        Ok(())
    }
}

fn large_reading() -> serde_json::Value {
    serde_json::json!({"status":"valid","report":"target/gpu-load/fixture.json","diagnostic":null,
        "reading":{"samples":(0..8192).map(|index|serde_json::json!({
            "unix_ms":1793155470757_u64+index,"foreign_percent":0.125,
            "raw":[{"pid":index,"name":"background-render-worker \"測定\"\n.exe","birth":134375030353605859_u64,"percent":1.25,"known":false}]
        })).collect::<Vec<_>>()}})
}

#[test]
fn large_report_keeps_byte_identical_output_with_batched_native_writes() {
    let reading = large_reading();
    let mut original = Sink::default();
    writeln!(original, "ARA_DEV_GPU_READING {reading}").expect("Original format writes to memory");
    let mut buffered = Sink::default();
    write_gpu_reading(&mut buffered, &reading).expect("Buffered telemetry writes to memory");
    assert!(
        original.bytes.len() > 1_000_000,
        "The fixture must model a large guard report"
    );
    assert_eq!(buffered.bytes, original.bytes);
    assert!(
        buffered.writes < original.writes / 100,
        "Buffering must reduce native writes: {} versus {}",
        buffered.writes,
        original.writes
    );
    assert_eq!(buffered.flushes, 1);
    println!(
        "telemetry bytes={} original_writes={} buffered_writes={}",
        buffered.bytes.len(),
        original.writes,
        buffered.writes
    );
}

#[test]
fn output_error_still_flushes_the_buffer_before_propagating() {
    let mut sink = Sink {
        fail_once: true,
        ..Default::default()
    };
    let result = write_gpu_reading(&mut sink, &large_reading());
    assert!(result.is_err());
    assert!(
        !sink.bytes.is_empty(),
        "Explicit flush retries the pending buffered prefix"
    );
    assert_eq!(sink.flushes, 1);
}
