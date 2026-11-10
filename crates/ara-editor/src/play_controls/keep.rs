//! Keep Runtime Changes: the runtime fields a Play diff offers, and which are ticked.
//! Ticking is an authoring input, not a queue to approve later; nothing is ticked at first.
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{Value, json};

/// One runtime field the diff found changed.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Row {
    /// The entity.
    pub entity: String,
    /// Its `Component.field`.
    pub path: String,
    /// Whether it can be saved to the scene.
    pub eligible: bool,
    /// Whether the simulation drives it (a physics body's transform).
    pub driven: bool,
    /// Its running value.
    pub play: Value,
    /// Its scene value now.
    pub edit: Value,
    /// Why it cannot be kept, when it cannot.
    pub reason: Option<String>,
}

/// The runtime changes of the chosen entities, and the fields ticked to keep.
pub(crate) struct Choice {
    world: String,
    entities: Vec<String>,
    rows: Vec<Row>,
    selected: BTreeSet<(String, String)>,
    include_driven: bool,
}

impl Choice {
    /// Reads a `play.diff` reply for `entities`; refuses rows outside them or repeated.
    pub(super) fn read(
        world: String,
        entities: Vec<String>,
        include_driven: bool,
        value: &Value,
    ) -> ara_ui::Result<Self> {
        let rows: Vec<Row> = serde_json::from_value(
            value
                .get("keepCandidates")
                .ok_or("The runtime diff did not say which fields can be kept")?
                .clone(),
        )?;
        if rows.iter().any(|row| !entities.contains(&row.entity)) {
            return Err("The runtime diff returned fields outside the chosen entities".into());
        }
        let mut seen = BTreeSet::new();
        if rows
            .iter()
            .any(|row| !seen.insert((&row.entity, &row.path)))
        {
            return Err("The runtime diff returned the same field twice".into());
        }
        Ok(Self {
            world,
            entities,
            rows,
            selected: BTreeSet::new(),
            include_driven,
        })
    }

    /// The world the changes were read from.
    pub(super) fn world(&self) -> &str {
        &self.world
    }

    /// The entities chosen.
    pub(super) fn entities(&self) -> &[String] {
        &self.entities
    }

    /// The changed fields.
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Whether simulation-driven fields are offered.
    pub fn include_driven(&self) -> bool {
        self.include_driven
    }

    /// Whether `row` can be ticked.
    pub fn enabled(&self, row: &Row) -> bool {
        row.eligible && (!row.driven || self.include_driven)
    }

    /// Whether `row` is ticked.
    pub fn picked(&self, row: &Row) -> bool {
        self.selected
            .contains(&(row.entity.clone(), row.path.clone()))
    }

    /// How many fields are ticked.
    pub fn count(&self) -> usize {
        self.selected.len()
    }

    pub(super) fn pick(&mut self, entity: &str, path: &str, on: bool) -> ara_ui::Result<()> {
        let row = self
            .rows
            .iter()
            .find(|row| row.entity == entity && row.path == path)
            .ok_or("That runtime field is no longer listed")?;
        if !self.enabled(row) {
            return Err("This runtime field cannot be kept; its scene value stays".into());
        }
        let key = (entity.to_owned(), path.to_owned());
        if on {
            self.selected.insert(key);
        } else {
            self.selected.remove(&key);
        }
        Ok(())
    }

    pub(super) fn driven(&mut self, on: bool) {
        self.include_driven = on;
        if !on {
            let rows = &self.rows;
            self.selected.retain(|(entity, path)| {
                rows.iter()
                    .any(|row| &row.entity == entity && &row.path == path && !row.driven)
            });
        }
    }

    /// The ticked fields as `play.keep` takes them, and whether any is driven.
    pub(super) fn request(&self) -> ara_ui::Result<(Vec<Value>, bool)> {
        if self.selected.is_empty() {
            return Err("Tick at least one runtime field to keep".into());
        }
        let fields = self
            .selected
            .iter()
            .map(|(entity, path)| json!({"entity": entity, "path": path}))
            .collect();
        let driven = self.rows.iter().any(|row| row.driven && self.picked(row));
        Ok((fields, driven))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_diff_ticks_nothing_and_refuses_rows_outside_or_repeated() {
        let row = json!({"entity":"e_one","path":"Light.intensity","eligible":true,"driven":false,"playStart":1000,"play":2500,"edit":1000,"status":"eligible","reason":null});
        let read = |entities: &[&str], value: Value| {
            Choice::read(
                "play:1".into(),
                entities.iter().map(|e| (*e).to_owned()).collect(),
                false,
                &value,
            )
        };
        let choice = read(&["e_one"], json!({"keepCandidates": [row.clone()]}));
        assert!(choice.is_ok_and(|c| c.count() == 0));
        assert!(read(&["e_other"], json!({"keepCandidates": [row.clone()]})).is_err());
        assert!(read(&["e_one"], json!({"keepCandidates": [row.clone(), row]})).is_err());
        assert!(read(&["e_one"], json!({"changes": []})).is_err());
    }

    #[test]
    fn driven_fields_tick_only_when_offered() {
        let row = json!({"entity":"e_one","path":"Transform.position","eligible":true,"driven":true,"playStart":[0,0,0],"play":[0,1,0],"edit":[0,0,0],"status":"eligible","reason":null});
        let mut choice = Choice::read(
            "play:1".into(),
            vec!["e_one".into()],
            false,
            &json!({"keepCandidates": [row]}),
        )
        .expect("a diff");
        assert!(choice.pick("e_one", "Transform.position", true).is_err());
        choice.driven(true);
        assert!(choice.pick("e_one", "Transform.position", true).is_ok());
        assert_eq!(choice.request().map(|(_, driven)| driven).ok(), Some(true));
        choice.driven(false);
        assert_eq!(choice.count(), 0, "unoffering drops the driven tick");
    }
}
