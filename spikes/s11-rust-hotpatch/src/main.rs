//! Windowless fixed-frame probe: PID and state survive body patches.
use std::time::{Duration, Instant};

fn main() {
    dioxus_devtools::connect(|msg| {
        if let dioxus_devtools::DevserverMsg::HotReload(msg) = msg
            && let Some(table) = msg.jump_table
        {
            eprintln!(
                "ARA_PATCH pid={:?} ours={} entries={} tick={:?}",
                msg.for_pid,
                std::process::id(),
                table.map.len(),
                subsecond::HotFn::current(tick).ptr_address()
            );
            if msg.for_pid == Some(std::process::id()) {
                // SAFETY: The local dx server built this table for this process.
                match unsafe { subsecond::apply_patch(table) } {
                    Ok(()) => eprintln!(
                        "ARA_PATCH applied tick={:?}",
                        subsecond::HotFn::current(tick).ptr_address()
                    ),
                    Err(error) => eprintln!("ARA_PATCH failed: {error}"),
                }
            }
        }
    });
    let start = Instant::now();
    let mut frame = 0_u64;
    loop {
        frame += 1;
        subsecond::HotFn::current(tick).call((frame, start.elapsed()));
        std::thread::sleep(Duration::from_millis(16));
    }
}

fn tick(frame: u64, elapsed: Duration) {
    if frame.is_multiple_of(30) {
        println!(
            "ARA_PROBE pid={} frame={frame} ms={} body=1",
            std::process::id(),
            elapsed.as_millis()
        );
    }
}
