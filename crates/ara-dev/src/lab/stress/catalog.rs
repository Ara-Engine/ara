use super::contract::{Suite, VERSION};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

pub fn suite() -> Result<Suite> {
    let suite: Suite = serde_json::from_str(include_str!("cases.v1.json"))?;
    ensure!(
        suite.version == VERSION && suite.fps == 60,
        "unsupported stress suite"
    );
    let mut ids = BTreeSet::new();
    for case in &suite.cases {
        ensure!(
            ids.insert(&case.id)
                && !case.id.is_empty()
                && !case.id.contains("..")
                && case
                    .id
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'.'),
            "invalid/duplicate case id"
        );
        ensure!(
            case.frames > 0
                && case.event_frame < case.frames
                && case.reference_frames.iter().all(|f| *f < case.frames),
            "invalid case frames: {}",
            case.id
        );
        ensure!(
            !case.gates.is_empty() && case.gates.iter().all(|g| (1..=6).contains(g)),
            "invalid gates"
        );
    }
    Ok(suite)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn contract_workloads_and_exact_frame_scripts() {
        let suite = suite().unwrap();
        for triangles in [108, 1000] {
            assert!(
                suite
                    .cases
                    .iter()
                    .any(|c| c.bodies == 4000 && c.triangles == triangles)
            );
        }
        for range in [1., 4., 8.] {
            for lpp in [2.4, 27.] {
                assert!(
                    suite
                        .cases
                        .iter()
                        .any(|c| c.range_m == range && c.target_lpp == Some(lpp))
                );
            }
        }
        for speed in [0.25, 1., 10.] {
            assert!(
                suite
                    .cases
                    .iter()
                    .any(|c| c.sun_deg_s == speed && c.elevation_deg <= 10.)
            );
        }
        let repeat = suite
            .cases
            .iter()
            .find(|c| c.id == "repeated-invalidation")
            .unwrap();
        assert_eq!(
            repeat
                .script(suite.fps)
                .iter()
                .filter(|f| f.event_age.is_some())
                .count(),
            300
        );
    }
}
