use super::Harness;
use crate::{Result, query};
use ara_ui_stories::{StepKind, StoryStep};

impl Harness {
    /// Run the story's data-only semantic recipe in order; each failure includes a mini native tree.
    pub fn run(&mut self) -> Result<()> {
        for (index, step) in self.story.steps.clone().iter().enumerate() {
            if let Err(error) = self.run_step(step) {
                return Err(query::fail(
                    &self.ui,
                    format!(
                        "Story {} step {} ({:?}): {error}",
                        self.story.name,
                        index + 1,
                        step.kind
                    ),
                )
                .into());
            }
        }
        Ok(())
    }
    fn run_step(&mut self, step: &StoryStep) -> Result<()> {
        let string = || {
            step.value
                .as_str()
                .ok_or("story step requires a string value")
        };
        match step.kind {
            StepKind::Visible => {
                self.query(&step.query)?;
            }
            StepKind::Focus => self.focus(&step.query)?,
            StepKind::Focused => self.expect_focused(&step.query)?,
            StepKind::Activate => self.activate(&step.query)?,
            StepKind::Text => {
                self.focus(&step.query)?;
                self.type_text(string()?)?;
            }
            StepKind::Value => self.expect_value(&step.query, &step.value)?,
            StepKind::VisibleText => self.expect_text(&step.query, string()?)?,
            StepKind::Action => self.expect_action(string()?, None)?,
            StepKind::Advance => self.advance(u32::try_from(
                step.value
                    .as_u64()
                    .ok_or("advance needs integer milliseconds")?,
            )?)?,
            StepKind::State => {
                let i = self.index(&step.query)?;
                let s = self.ui.nodes[i].state;
                for (key, expected) in step.value.as_object().ok_or("state needs an object")? {
                    let actual = match key.as_str() {
                        "hover" => s.hover,
                        "active" => s.active,
                        "focus" => s.focus,
                        "disabled" => s.disabled,
                        _ => return Err(format!("unknown state {key}").into()),
                    };
                    if expected.as_bool() != Some(actual) {
                        return Err(query::fail(
                            &self.ui,
                            format!("Expected state {key}={expected}, got {actual}"),
                        )
                        .into());
                    }
                }
            }
        }
        Ok(())
    }
}
