//! `probe-timers`: how precisely this OS sleeps and times out, which bounds the player loop's
//! link latency and tick jitter.

use std::net::UdpSocket;
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::json;

fn percentiles(mut samples: Vec<f64>) -> serde_json::Value {
    samples.sort_by(f64::total_cmp);
    let at =
        |p: f64| samples[((p * samples.len() as f64).ceil() as usize).clamp(1, samples.len()) - 1];
    json!({ "p50Ms": at(0.5), "p95Ms": at(0.95), "maxMs": at(1.0) })
}

/// Measures and writes the report to `out` (and stdout).
pub fn run(out: &std::path::Path) -> Result<()> {
    let measure = |n: usize, f: &mut dyn FnMut()| {
        (0..n)
            .map(|_| {
                let t = Instant::now();
                f();
                t.elapsed().as_secs_f64() * 1000.0
            })
            .collect::<Vec<_>>()
    };
    let sleep_1ms = measure(200, &mut || std::thread::sleep(Duration::from_millis(1)));
    let sleep_250us = measure(200, &mut || std::thread::sleep(Duration::from_micros(250)));
    let socket = UdpSocket::bind("127.0.0.1:0")?;
    socket.set_read_timeout(Some(Duration::from_millis(2)))?;
    let mut buf = [0u8; 16];
    let recv_2ms = measure(50, &mut || {
        let _ = socket.recv(&mut buf);
    });
    let report = json!({
        "sleep1ms": percentiles(sleep_1ms),
        "sleep250us": percentiles(sleep_250us),
        "socketReadTimeout2ms": percentiles(recv_2ms),
    });
    let text = serde_json::to_string_pretty(&report)?;
    println!("{text}");
    std::fs::write(out, text)?;
    Ok(())
}
