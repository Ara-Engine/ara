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
        out.push_str("<panel id='asset-types' class='asset-filters' name='enum'><panel id='asset-type-value' class='asset-filter'>");
        let start = out.len();
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
        compact_select(out, start);
        out.push_str("</panel>");
        self.asset_bookmarks_select(editor, out);
        out.push_str("</panel>");
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
/// Mark the dropdown appended since `start` as a compact toolbar select.
pub(in crate::shell) fn compact_select(out: &mut String, start: usize) {
    let markup = if out[start..].contains("kit-select-compact'") {
        out[start..].replacen(
            "kit-select-compact'",
            "kit-select-compact kit-select-filter'",
            1,
        )
    } else {
        out[start..].replacen(
            "class='kit-select'",
            "class='kit-select kit-select-compact kit-select-filter'",
            1,
        )
    };
    out.truncate(start);
    out.push_str(&markup);
}
