//! Stream action journals without retaining their diagnostic/report records.
use super::Recorded;
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};

#[derive(Default)]
struct Metadata {
    seed: u64,
    host_seed: Option<u64>,
    unknown_host_seed: bool,
    actions: u64,
}
impl Metadata {
    fn observe(&mut self, record: &Value, line: usize) -> ara_ui::Result<()> {
        self.seed = record["seed"].as_u64().unwrap_or(self.seed);
        self.actions += 1;
        match &record["hostSeed"] {
            Value::Null => self.unknown_host_seed = true,
            value => {
                let seed = value
                    .as_u64()
                    .ok_or_else(|| format!("Replay line {line}: invalid hostSeed"))?;
                if seed != self.seed || self.host_seed.is_some_and(|previous| previous != seed) {
                    return Err(format!(
                        "Replay line {line}: inconsistent hostSeed/generator seed"
                    )
                    .into());
                }
                self.host_seed = Some(seed);
            }
        }
        Ok(())
    }
    fn known_host_seed(&self) -> Option<u64> {
        (!self.unknown_host_seed)
            .then_some(self.host_seed)
            .flatten()
    }
}

fn visit(
    path: &Path,
    mut action: impl FnMut(usize, &Value) -> ara_ui::Result<()>,
) -> ara_ui::Result<Metadata> {
    let mut metadata = Metadata::default();
    for (line, text) in BufReader::new(File::open(path)?).lines().enumerate() {
        let text = text?;
        if text.trim().is_empty() {
            continue;
        }
        let line = line + 1;
        let record: Value =
            serde_json::from_str(&text).map_err(|error| format!("Replay line {line}: {error}"))?;
        if record["phase"] == "begin" {
            metadata.observe(&record, line)?;
            action(line, &record)?;
        }
    }
    if metadata.actions == 0 {
        return Err("The monkey journal contains no recorded actions".into());
    }
    Ok(metadata)
}

pub(super) fn seed(path: &Path) -> ara_ui::Result<u64> {
    let metadata = visit(path, |_, _| Ok(()))?;
    Ok(metadata.known_host_seed().unwrap_or(metadata.seed))
}

pub(super) struct Replay {
    pub seed: u64,
    pub original_host_seed: Option<u64>,
    pub actions: VecDeque<Recorded>,
}
pub(super) fn read(path: &Path) -> ara_ui::Result<Replay> {
    let mut actions = VecDeque::new();
    let metadata = visit(path, |line, record| {
        actions.push_back(Recorded {
            action: serde_json::from_value(record["action"].clone())
                .map_err(|error| format!("Replay action on line {line}: {error}"))?,
            cleanup: record["cleanup"].as_bool().unwrap_or(false),
        });
        Ok(())
    })?;
    Ok(Replay {
        seed: metadata.seed,
        original_host_seed: metadata.known_host_seed(),
        actions,
    })
}
