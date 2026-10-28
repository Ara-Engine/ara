//! Trusted Play attribution before records enter the diagnostic Store or event buffer.
use ara_diag::{Diagnostic, Subject};
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(crate) struct DiagnosticSources {
    slots: BTreeMap<(String, usize), (String, u64)>,
    next: u64,
}

impl DiagnosticSources {
    pub(crate) fn binding(
        &mut self,
        world: &str,
        player: usize,
        source: &str,
    ) -> Result<u64, Diagnostic> {
        let key = (world.to_owned(), player);
        if self
            .slots
            .get(&key)
            .is_none_or(|(previous, _)| previous != source)
        {
            self.next = self.next.checked_add(1).ok_or_else(|| {
                Diagnostic::error(
                    crate::codes::PLAY,
                    "Play diagnostic source sequence exhausted",
                )
            })?;
            self.slots
                .insert(key.clone(), (source.to_owned(), self.next));
        }
        Ok(self.slots[&key].1)
    }

    pub(crate) fn scope(
        &mut self,
        world: &str,
        player: usize,
        source: &str,
        mut diagnostic: Diagnostic,
    ) -> Result<Diagnostic, Diagnostic> {
        let binding = self.binding(world, player, source)?;
        let mut data = match diagnostic.data.take().map(|data| *data) {
            Some(Value::Object(data)) => data,
            Some(original) => Map::from_iter([("original".into(), original)]),
            None => Map::new(),
        };
        if let Some(reported) = data.insert(
            "play".into(),
            json!({"world":world,"player":player,"source":source,"binding":binding}),
        ) {
            data.entry("reportedPlay").or_insert(reported);
        }
        diagnostic.data = Some(Box::new(data.into()));
        diagnostic.subjects.push(Subject::system(format!(
            "play/{world}/{player}/{binding}/{source}"
        )));
        Ok(diagnostic)
    }
}

#[cfg(test)]
mod tests;
