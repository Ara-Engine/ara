//! `--verify-loading` captures: the loading window's real-run milestones and every
//! state from fixed statuses, rendered offscreen in both themes.
use super::{Imports, Splash, Stage, Status};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

/// `--verify-loading` output: the real run's milestones, then every state, in both
/// themes, as `loading-<name>-<theme>.png`.
pub(super) struct Captures {
    dir: PathBuf,
    taken: BTreeSet<&'static str>,
}
impl Captures {
    pub(super) fn new(dir: &Path) -> Self {
        Self {
            dir: dir.to_owned(),
            taken: BTreeSet::new(),
        }
    }
    /// Capture each milestone of the real run once: preparing, the first imported
    /// source, half way, the review or opening.
    pub(super) fn milestone(&mut self, splash: &mut Splash, status: &Status) -> anyhow::Result<()> {
        let half = status
            .imports
            .as_ref()
            .is_some_and(|i| (i.done + i.failed.len()) * 2 >= i.total && i.pending > 0);
        let name = match status.stage() {
            Stage::Preparing => "run-preparing",
            Stage::Importing if half => "run-half",
            Stage::Importing if status.latest.is_some() => "run-importing",
            Stage::Importing => return Ok(()),
            Stage::Review => "run-review",
            Stage::Opening => "run-opening",
        };
        if self.taken.insert(name) {
            self.both(splash, status, name)?;
        }
        Ok(())
    }
    /// Every state from fixed statuses, for review at any time.
    pub(super) fn states(&mut self, splash: &mut Splash, real: &Status) -> anyhow::Result<()> {
        let base = Status {
            elapsed: 42,
            imports: None,
            latest: None,
            skip: false,
            opening: false,
            ..real.clone()
        };
        let importing = Imports {
            done: 36,
            total: 115,
            pending: 79,
            failed: Vec::new(),
        };
        let failed = |n: usize| -> Vec<(String, String)> {
            (0..n)
                .map(|i| {
                    (
                        format!("assets/kit/broken_{i}.gltf"),
                        "ARA4302 the glTF buffer `broken.bin` is missing\nimport-worker exited"
                            .into(),
                    )
                })
                .collect()
        };
        let states = [
            ("preparing", base.clone()),
            (
                "importing",
                Status {
                    imports: Some(importing.clone()),
                    latest: Some("assets/kit/container_blue.gltf".into()),
                    ..base.clone()
                },
            ),
            (
                "open-now",
                Status {
                    imports: Some(Imports {
                        done: 81,
                        pending: 32,
                        failed: failed(2),
                        ..importing.clone()
                    }),
                    latest: Some("assets/third_party/barrel_stove/barrel_stove_2k.gltf".into()),
                    skip: true,
                    elapsed: 95,
                    ..base.clone()
                },
            ),
            (
                "review",
                Status {
                    imports: Some(Imports {
                        done: 112,
                        pending: 0,
                        failed: failed(3),
                        ..importing.clone()
                    }),
                    elapsed: 188,
                    ..base.clone()
                },
            ),
            (
                "review-many",
                Status {
                    imports: Some(Imports {
                        done: 109,
                        pending: 0,
                        failed: failed(6),
                        ..importing.clone()
                    }),
                    elapsed: 188,
                    ..base.clone()
                },
            ),
            // A very long path and a long run: the path keeps its start and file name
            // around "…"; the timer, the count and the button stay in the card.
            (
                "long-path",
                Status {
                    project: "Outpost with an unusually long project name for a loading window".into(),
                    imports: Some(Imports {
                        done: 81,
                        pending: 32,
                        failed: vec![(
                            format!("assets/third_party/{}/broken_container_variant_with_a_long_name_8k.gltf", "nested_folder/".repeat(6)),
                            "ARA4302 the glTF buffer `broken_container_variant_with_a_long_name_8k.bin` is missing from its folder".into(),
                        )],
                        ..importing.clone()
                    }),
                    latest: Some(format!(
                        "assets/props/{}rock_large_moss_covered_variant_04_8k.glb",
                        "industrial/scrapyard/rock_formations/".repeat(4)
                    )),
                    skip: true,
                    elapsed: 3725,
                    ..base.clone()
                },
            ),
            (
                "opening-early",
                Status {
                    imports: Some(Imports {
                        done: 81,
                        pending: 34,
                        ..importing.clone()
                    }),
                    opening: true,
                    elapsed: 9,
                    ..base.clone()
                },
            ),
            (
                "opening",
                Status {
                    imports: Some(Imports {
                        done: 115,
                        pending: 0,
                        ..importing.clone()
                    }),
                    opening: true,
                    elapsed: 191,
                    ..base.clone()
                },
            ),
        ];
        for (name, status) in states {
            self.both(splash, &status, name)?;
        }
        Ok(())
    }
    fn both(&mut self, splash: &mut Splash, status: &Status, name: &str) -> anyhow::Result<()> {
        std::fs::create_dir_all(&self.dir)?;
        for (theme, light) in [("dark", false), ("light", true)] {
            splash.show(&Status {
                light,
                ..status.clone()
            })?;
            splash
                .ui_gpu
                .pixels()?
                .save(self.dir.join(format!("loading-{name}-{theme}.png")))?;
        }
        splash.show(status)
    }
}
