use super::*;
use crate::source::escape;

/// Semantic styles for the shared searchable select and its root-level popup.
pub const CSS: &str = include_str!("select.css");

impl<T: Clone + PartialEq> Select<T> {
    /// Form control markup; selected text is exposed through the ComboBox role.
    /// Mount the popup separately under the root so panel clips never hide it.
    pub fn control_markup(&self) -> String {
        let selected = self
            .selected
            .as_ref()
            .and_then(|value| self.options.iter().find(|o| &o.value == value));
        let text = selected.map_or("Select…", |o| o.label.as_str());
        format!(
            "<dropdown id='{}' class='kit-select' text='{}' label='{}' description='{}' cursor='pointer' action='ara.select.toggle' disabled='{}' />",
            escape(self.control_id(), b'\''),
            escape(text, b'\''),
            escape(&self.label, b'\''),
            escape(text, b'\''),
            self.options.iter().all(|o| o.disabled.is_some())
        )
    }

    /// Root-level popup with a searchable, scrolling option list. Content wraps
    /// naturally within the kit's 360 logical-pixel maximum and viewport bounds.
    pub fn popover_markup(&self) -> String {
        if !self.open {
            return String::new();
        }
        let esc = |text: &str| escape(text, b'\'');
        let popup = esc(&self.popup_id());
        let mut out = format!(
            "<popover id='{popup}' class='kit-select-popover' anchor='{}' placement='bottom-start'><field id='{}' class='kit-select-search' text='{}' placeholder='{}' label='Filter {}' action='ara.select.query' /><scroll id='{popup}-options' class='kit-select-options'>",
            esc(self.control_id()),
            esc(&self.search_id()),
            esc(&self.query),
            esc(&self.search_hint),
            esc(&self.label)
        );
        let query = self.query.to_lowercase();
        let mut count = 0;
        for (index, option) in self
            .options
            .iter()
            .enumerate()
            .filter(|(_, o)| o.label.to_lowercase().contains(&query))
        {
            count += 1;
            let chosen = self.selected.as_ref() == Some(&option.value);
            let class = if self.active == Some(index) {
                "kit-select-active"
            } else if chosen {
                "kit-select-selected"
            } else {
                ""
            };
            let description = option
                .disabled
                .as_deref()
                .or(option.description.as_deref())
                .unwrap_or(&option.label);
            out.push_str(&format!("<button id='{}' class='kit-select-option {class}' text='{}' label='{}' description='{}' disabled='{}' action='{}' />", esc(&self.option_id(index)), esc(&option.label), esc(&option.label), esc(description), option.disabled.is_some(), esc(&self.choose_action())));
        }
        if count == 0 {
            out.push_str(&format!(
                "<text id='{popup}-empty' class='kit-select-empty' text='No matching options' />"
            ));
        }
        out.push_str("</scroll></popover>");
        out
    }
}
