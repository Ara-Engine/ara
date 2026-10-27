//! Searchable type data filter; ordinary text terms survive a type change.
use super::*;
const TYPES: &[(&str, &str)] = &[
    ("All types", ""),
    ("Models", "mesh"),
    ("Textures", "texture"),
    ("Materials", "material"),
    ("Prefabs", "prefab"),
    ("Scenes", "scene"),
    ("Audio", "audio"),
    ("Scripts", "script"),
    ("VFX", "vfx"),
];
fn selected(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .filter_map(|term| term.strip_prefix("type:").map(str::to_owned))
        .collect()
}
fn with_types(query: &str, kinds: &[String]) -> String {
    query
        .split_whitespace()
        .filter(|term| !term.starts_with("type:"))
        .map(str::to_owned)
        .chain(kinds.iter().map(|kind| format!("type:{kind}")))
        .collect::<Vec<_>>()
        .join(" ")
}
impl Shell {
    pub(super) fn asset_types_markup(&mut self, editor: &Editor, out: &mut String) {
        let query = editor
            .state
            .searches
            .get("assets")
            .map_or("", String::as_str);
        let current = json!(selected(query));
        let mut options: Vec<_> = TYPES
            .iter()
            .map(|(label, kind)| {
                (
                    if kind.is_empty() {
                        json!([])
                    } else {
                        json!([kind])
                    },
                    (*label).to_owned(),
                )
            })
            .collect();
        // Typed search may express an uncommon kind or intersect several kinds.
        // Show that custom state faithfully until the human picks another filter.
        if !options.iter().any(|(value, _)| value == &current) {
            options.push((current.clone(), "Custom type filter".into()));
        }
        out.push_str("<panel id='asset-types' class='row' name='enum' style='height:auto;min-height:30px;flex-shrink:0;flex-wrap:wrap;gap:8px;padding:4px'><text id='asset-type-label' class='caption' text='Type' /><panel id='asset-type-value' style='width:180px;max-width:100%;min-width:0px'>");
        self.choice_button(
            "asset-type-select",
            "Filter asset type",
            choices::Select {
                destination: choices::Destination::AssetType,
                options,
                current,
            },
            out,
        );
        out.push_str("</panel></panel>");
    }
    /// Update the navigation search through its ordinary typed command.
    pub(in crate::shell) fn choose_asset_types(
        &mut self,
        editor: &mut Editor,
        value: Value,
    ) -> ara_ui::Result<()> {
        let kinds: Vec<String> = serde_json::from_value(value)?;
        if kinds
            .iter()
            .any(|kind| kind.chars().any(char::is_whitespace))
        {
            return Err("An asset type must be one search token".into());
        }
        let query = with_types(
            editor
                .state
                .searches
                .get("assets")
                .map_or("", String::as_str),
            &kinds,
        );
        editor.action("editor.search", json!({"panel":"assets","query":query}))?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "type_tests.rs"]
mod tests;
