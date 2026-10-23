use super::*;
impl Shell {
    pub(super) fn assets_panel(&mut self, editor: &Editor, out: &mut String) {
        out.push_str("<panel id='asset-toolbar' class='toolbar'>");
        self.search(out, "assets", editor);
        out.push_str("<text id='asset-hint' class='muted' text='Filter: type:prefab / type:glb' /></panel><scroll id='asset-rows' class='body' />");
        let query = editor
            .state
            .searches
            .get("assets")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        let items: Vec<_> = self
            .browser
            .items
            .iter()
            .filter(|a| {
                if let Some(kind) = query.strip_prefix("type:") {
                    a.kind == kind
                } else {
                    a.path.to_lowercase().contains(&query)
                }
            })
            .cloned()
            .collect();
        self.asset_rows.items.clear();
        self.asset_entries.clear();
        for asset in items {
            let key = ara_ui::source::stable_id(&asset.path);
            self.asset_rows.items.push((
                key,
                format!("{}   {}", asset.kind.to_uppercase(), asset.path),
            ));
            self.asset_entries.insert(key, asset);
        }
    }
}
