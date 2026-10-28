//! Stream action journals without retaining their diagnostic/report records.
mod writer;
use super::Recorded;
use serde_json::Value;
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader},
    path::Path,
};
pub(super) use writer::{Ticket, Writer};

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
    // Keep only the current begin until its end, next begin, or EOF. Legacy
    // incomplete begins still replay; a new exact abort cancels only this begin.
    let mut pending: Option<(usize, Value)> = None;
    for (line, text) in BufReader::new(File::open(path)?).lines().enumerate() {
        let text = text?;
        if text.trim().is_empty() {
            continue;
        }
        let line = line + 1;
        let record: Value =
            serde_json::from_str(&text).map_err(|error| format!("Replay line {line}: {error}"))?;
        if record["phase"] == "begin" {
            if let Some((line, record)) = pending.take() {
                metadata.actions += 1;
                action(line, &record)?;
            }
            metadata.observe(&record, line)?;
            pending = Some((line, record));
        } else if record["phase"] == "end" {
            if pending
                .as_ref()
                .is_some_and(|(_, begin)| begin["step"] == record["step"])
                && let Some((line, begin)) = pending.take()
            {
                metadata.actions += 1;
                action(line, &begin)?;
            }
        } else if record["phase"] == "abort" {
            let matches = pending.as_ref().is_some_and(|(_, begin)| {
                record["wasDispatched"] == false
                    && record["ticket"].as_u64().is_some()
                    && record["ticket"] == begin["ticket"]
                    && record["step"].as_u64().is_some()
                    && record["step"] == begin["step"]
            });
            if !matches {
                return Err(format!("Replay line {line}: abort must match an uncompleted begin's exact ticket and step").into());
            }
            pending = None;
        }
    }
    if let Some((line, record)) = pending {
        metadata.actions += 1;
        action(line, &record)?;
    }
    if metadata.actions == 0 {
        return Err("The monkey journal contains no recorded actions to replay".into());
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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests;
