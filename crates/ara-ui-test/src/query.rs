use crate::{Query, QueryBy};
use ara_ui::{Ui, widgets::Widget};
use std::{fmt, path::PathBuf};

/// A semantic or visual assertion failure with a readable native tree and optional diff artifact.
#[derive(Debug)]
pub struct Failure {
    /// What was expected and what actually happened.
    pub message: String,
    /// Mini retained tree, including hidden/disabled/focused state.
    pub tree: String,
    /// Golden difference PNG, if comparison generated one.
    pub diff_image: Option<PathBuf>,
}
impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n{}", self.message, self.tree)?;
        if let Some(path) = &self.diff_image {
            write!(f, "\nDiff image: {}", path.display())?;
        }
        Ok(())
    }
}
impl std::error::Error for Failure {}

pub(crate) fn visible(ui: &Ui, i: usize) -> bool {
    ui.paint_order.contains(&i) && ui.nodes[i].rect.intersects(ui.nodes[i].clip)
}
pub(crate) fn label(ui: &Ui, i: usize) -> &str {
    let n = &ui.nodes[i];
    n.element
        .attrs
        .get("label")
        .map(String::as_str)
        .unwrap_or_else(|| {
            n.text
                .as_ref()
                .map(|t| t.value.as_str())
                .unwrap_or(&n.element.id)
        })
}
pub(crate) fn role(ui: &Ui, i: usize) -> &'static str {
    // Share the role resolver with the production AccessKit projection, including hidden nodes.
    let role = Some(ara_ui::accessibility::role_for_tag(
        &ui.nodes[i].element.tag,
    ));
    match role {
        Some(accesskit::Role::Button) => "button",
        Some(accesskit::Role::TextInput) => "textbox",
        Some(accesskit::Role::CheckBox) => "checkbox",
        Some(accesskit::Role::Slider) => "slider",
        Some(accesskit::Role::ProgressIndicator) => "progressbar",
        Some(accesskit::Role::ComboBox) => "combobox",
        Some(accesskit::Role::List) => "list",
        Some(accesskit::Role::Splitter) => "separator",
        Some(accesskit::Role::ScrollView) => "scrollview",
        Some(accesskit::Role::Tooltip) => "tooltip",
        Some(accesskit::Role::TabList) => "tablist",
        Some(accesskit::Role::Tab) => "tab",
        Some(accesskit::Role::Menu) => "menu",
        Some(accesskit::Role::Dialog) => "dialog",
        Some(accesskit::Role::Tree) => "tree",
        Some(accesskit::Role::Table) => "table",
        Some(accesskit::Role::Image) => "img",
        Some(accesskit::Role::Label) => "text",
        _ => "generic",
    }
}
pub(crate) fn value(ui: &Ui, i: usize) -> serde_json::Value {
    use serde_json::json;
    match ui
        .widgets
        .get(&ara_ui::source::stable_id(&ui.nodes[i].element.id))
    {
        Some(Widget::Input(input)) => json!(input.value()),
        Some(Widget::Toggle(value)) => json!(value),
        Some(Widget::Slider { value, .. } | Widget::Splitter(value)) => json!(value),
        Some(Widget::Choice { selected, .. }) => json!(selected),
        Some(Widget::Scroll(offset)) => json!(offset),
        None => serde_json::Value::Null,
    }
}
pub(crate) fn find(ui: &Ui, query: &Query) -> Vec<usize> {
    ui.paint_order
        .iter()
        .copied()
        .filter(|i| visible(ui, *i))
        .filter(|i| {
            let n = &ui.nodes[*i];
            let matched = match query.by {
                QueryBy::Role => role(ui, *i) == query.value,
                QueryBy::Label => label(ui, *i) == query.value,
                QueryBy::Text => n
                    .text
                    .as_ref()
                    .is_some_and(|text| text.value == query.value),
                QueryBy::TestId => {
                    n.element.attrs.get("test-id").unwrap_or(&n.element.id) == &query.value
                }
            };
            matched && query.name.as_ref().is_none_or(|name| label(ui, *i) == name)
        })
        .collect()
}
pub(crate) fn tree_dump(ui: &Ui) -> String {
    let mut out = String::new();
    for (i, n) in ui.nodes.iter().enumerate() {
        let mut depth = 0;
        let mut parent = n.element.parent;
        while let Some(p) = parent {
            depth += 1;
            parent = ui.nodes[p].element.parent;
        }
        out.push_str(&format!(
            "{}<{} #{} name={:?} value={}{}{}{}>\n",
            "  ".repeat(depth),
            n.element.tag,
            n.element.id,
            label(ui, i),
            value(ui, i),
            if visible(ui, i) { "" } else { " hidden" },
            if n.state.disabled { " disabled" } else { "" },
            if n.state.focus { " focused" } else { "" }
        ));
    }
    out
}
pub(crate) fn fail(ui: &Ui, message: impl Into<String>) -> Failure {
    Failure {
        message: message.into(),
        tree: tree_dump(ui),
        diff_image: None,
    }
}
