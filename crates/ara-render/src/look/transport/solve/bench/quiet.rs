//! Cooperative admission for long offline replays on the shared measurement PC.
//! Waiting is outside all event timers. An already-running packet may finish.
use std::{path::Path, time::Duration};

pub fn wait(path: Option<&Path>) {
    let Some(path) = path else { return };
    let mut announced = false;
    while let Ok(metadata) = std::fs::metadata(path) {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let seconds = |key: &str| -> Option<u64> {
            let stamp = text.split_whitespace().find_map(|s| s.strip_prefix(key))?;
            let mut parts = stamp.get(11..19)?.split(':');
            Some(
                parts.next()?.parse::<u64>().ok()? * 3600
                    + parts.next()?.parse::<u64>().ok()? * 60
                    + parts.next()?.parse::<u64>().ok()?,
            )
        };
        // Locks last at most 45 minutes, including a possible midnight crossing.
        // File modification anchors their local start without assuming a timezone.
        let duration = seconds("start=")
            .zip(seconds("expires="))
            .map_or(45 * 60, |(start, end)| (end + 86400 - start) % 86400)
            .min(45 * 60);
        if metadata
            .modified()
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age >= Duration::from_secs(duration))
        {
            break;
        }
        if !announced {
            println!("G3 waiting for quiet window: {}", text.trim());
            announced = true;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    if announced {
        println!("G3 quiet window released; replay resumes");
    }
}
