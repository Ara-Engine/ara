use super::Harness;
use crate::{Query, Result, query};
use ara_ui::style::State;
use ara_ui_stories::localize;
use serde_json::Value;

impl Harness {
    /// Assert that a semantic control owns native focus.
    pub fn expect_focused(&self, query: &Query) -> Result<()> {
        let id = self.query(query)?;
        if self.input.focused() == Some(ara_ui::source::stable_id(&id)) {
            return Ok(());
        }
        Err(query::fail(
            &self.ui,
            format!("Expected focus on {id}, got {:?}", self.input.focused()),
        )
        .into())
    }
    /// Assert active/hover/focus/disabled selector state.
    pub fn expect_state(&self, query: &Query, state: State) -> Result<()> {
        let i = self.index(query)?;
        if self.ui.nodes[i].state == state {
            return Ok(());
        }
        Err(query::fail(
            &self.ui,
            format!("Expected state {state:?}, got {:?}", self.ui.nodes[i].state),
        )
        .into())
    }
    /// Assert current display text; `loc:key` expectations resolve against this view's locale.
    pub fn expect_text(&self, query: &Query, text: &str) -> Result<()> {
        let expected = if text.starts_with("loc:") {
            localize(text, &self.locale)?
        } else {
            text.into()
        };
        let i = self.index(query)?;
        if self.ui.nodes[i]
            .text
            .as_ref()
            .is_some_and(|t| t.value == expected)
        {
            return Ok(());
        }
        Err(query::fail(
            &self.ui,
            format!(
                "Expected visible text {expected:?} on {}",
                self.ui.nodes[i].element.id
            ),
        )
        .into())
    }
    /// Assert a committed native value, excluding visual-only IME composition.
    pub fn expect_value(&self, query: &Query, expected: &Value) -> Result<()> {
        let i = self.index(query)?;
        let actual = query::value(&self.ui, i);
        if ara_json::json_eq(&actual, expected) {
            return Ok(());
        }
        Err(query::fail(&self.ui, format!("Expected value {expected}, got {actual}")).into())
    }
    /// Assert every visible control subscribed to a fixture key exposes the expected native value.
    pub fn expect_bound(&self, key: &str, expected: &Value) -> Result<()> {
        let nodes: Vec<_> = self
            .ui
            .paint_order
            .iter()
            .copied()
            .filter(|i| {
                self.ui.nodes[*i]
                    .element
                    .attrs
                    .get("bind")
                    .is_some_and(|v| v == key)
            })
            .collect();
        if nodes.is_empty() {
            return Err(query::fail(&self.ui, format!("No visible bindings for {key}")).into());
        }
        for i in nodes {
            let actual = if self
                .ui
                .widgets
                .contains_key(&ara_ui::source::stable_id(&self.ui.nodes[i].element.id))
            {
                query::value(&self.ui, i)
            } else {
                serde_json::json!(
                    self.ui.nodes[i]
                        .text
                        .as_ref()
                        .map(|t| t.value.as_str())
                        .unwrap_or_default()
                )
            };
            if !ara_json::json_eq(&actual, expected) {
                return Err(query::fail(
                    &self.ui,
                    format!("Binding {key}: expected {expected}, got {actual}"),
                )
                .into());
            }
        }
        Ok(())
    }
    /// Assert that an action was emitted, optionally matching its payload exactly.
    pub fn expect_action(&self, name: &str, value: Option<&Value>) -> Result<()> {
        if self
            .actions
            .iter()
            .any(|a| a.name == name && value.is_none_or(|v| ara_json::json_eq(&a.value, v)))
        {
            return Ok(());
        }
        Err(query::fail(
            &self.ui,
            format!(
                "Expected action {name} {value:?}; emitted {:?}",
                self.actions
            ),
        )
        .into())
    }
}
