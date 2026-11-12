//! `ARA_RENDERLAB_CPU_LOG=<file>`: one JSON line per rendered frame saying where the CPU's time
//! goes (a measurement aid; unset, nothing is logged and nothing is timed beyond the clock reads).
//!
//! Fields, all in ms: `frame` (0-based), `wall_ms` (since the previous frame began), `bridge_ms`
//! (Ara's extract and sync before it calls Renderlab), `render_call_ms` (wall time around
//! Renderlab's `render_view`), Renderlab's own `FrameReport::cpu` parts (`preparation`,
//! `visibility`, `packing`, `upload`, `encode`, `submit`, `poll`, `shadow_preparation`,
//! `shadow_upload`, `shadow_encode`), `unitemized_ms` (`render_call_ms` minus those parts: the
//! Renderlab CPU time not itemized yet), `handoff_ms` (the shared-image delivery to the
//! presenter) and `in_flight_ms` (from the frame's submit until the bridge took it).
use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

/// Renderlab's `FrameReport::cpu` parts, by name.
pub(crate) type CpuParts = [(&'static str, Duration); 10];

pub(crate) struct CpuLog {
    out: BufWriter<File>,
    last_begin: Option<Instant>,
    frame: u64,
}

/// A frame's row from its `begin` until its `finish` knows the handoff.
pub(crate) struct CpuRow {
    frame: u64,
    wall: Duration,
    bridge: Duration,
    render_call: Duration,
    parts: CpuParts,
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.
}

impl CpuLog {
    /// The log named by `ARA_RENDERLAB_CPU_LOG`, if set and creatable.
    pub(crate) fn from_env() -> Option<Self> {
        let file = File::create(std::env::var_os("ARA_RENDERLAB_CPU_LOG")?).ok()?;
        Some(Self {
            out: BufWriter::new(file),
            last_begin: None,
            frame: 0,
        })
    }

    /// Starts the row of the frame that began at `began`.
    pub(crate) fn begin(
        &mut self,
        began: Instant,
        bridge: Duration,
        render_call: Duration,
        parts: CpuParts,
    ) -> CpuRow {
        let wall = self
            .last_begin
            .map_or(Duration::ZERO, |last| began.duration_since(last));
        self.last_begin = Some(began);
        self.frame += 1;
        CpuRow {
            frame: self.frame - 1,
            wall,
            bridge,
            render_call,
            parts,
        }
    }

    /// Writes the row once the handoff is done.
    pub(crate) fn finish(&mut self, row: CpuRow, handoff_ms: f64, in_flight: Duration) {
        let itemized: f64 = row.parts.iter().map(|(_, part)| ms(*part)).sum();
        let mut line = format!(
            "{{\"frame\":{},\"wall_ms\":{:.3},\"bridge_ms\":{:.3},\"render_call_ms\":{:.3}",
            row.frame,
            ms(row.wall),
            ms(row.bridge),
            ms(row.render_call)
        );
        for (name, part) in row.parts {
            line.push_str(&format!(",\"{name}\":{:.3}", ms(part)));
        }
        line.push_str(&format!(
            ",\"unitemized_ms\":{:.3},\"handoff_ms\":{handoff_ms:.3},\"in_flight_ms\":{:.3}}}\n",
            ms(row.render_call) - itemized,
            ms(in_flight)
        ));
        // A log that cannot be written is dropped, never an error in the frame path.
        let _ = self.out.write_all(line.as_bytes()).and_then(|()| self.out.flush());
    }
}
