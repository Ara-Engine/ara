use super::*;
impl Shell {
    pub(super) fn console_panel(&mut self, editor: &mut Editor, out: &mut String) {
        out.push_str("<panel id='console-toolbar' class='toolbar'>");
        self.search(out, "console", editor);
        out.push_str(
            "</panel><scroll id='console-scroll' class='body' style='padding:4px;gap:0px'>",
        );
        let mut shown = 0;
        let logs = editor
            .action("logs", json!({"limit":300}))
            .unwrap_or(Value::Null);
        let query = editor
            .state
            .searches
            .get("console")
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if let Some(entries) = logs["entries"].as_array() {
            for (index, entry) in entries.iter().enumerate() {
                let Ok(diagnostic) =
                    serde_json::from_value::<ara_diag::Diagnostic>(entry["diagnostic"].clone())
                else {
                    continue;
                };
                let label = format!(
                    "{}  {}  ×{}",
                    diagnostic.code, diagnostic.message, entry["count"]
                );
                if !query.is_empty()
                    && !format!("{} {:?}", label, diagnostic.severity)
                        .to_lowercase()
                        .contains(&query)
                {
                    continue;
                }
                shown += 1;
                let (icon, tint) = match diagnostic.severity {
                    ara_diag::Severity::Error => ("circle-x", "destructive"),
                    ara_diag::Severity::Warning => ("triangle-alert", "warning"),
                    _ => ("info", "info"),
                };
                let open = format!("diagnostic-open-{index}");
                self.bindings.insert(
                    ara_ui::source::stable_id(&open),
                    Binding::Diagnostic(diagnostic.clone()),
                );
                out.push_str(&format!("<panel id='diagnostic-{index}' class='row' style='min-height:22px;padding-left:4px;gap:6px;flex-shrink:0'>{}<button id='{open}' class='ghost' style='flex-grow:1;flex-basis:0px;justify-content:start;height:22px' label='{}' description='Select the entity and reveal the field' action='Dispatch'><text id='{open}-code' class='caption mono' text='{}' /><text id='{open}-text' class='fg' text='{}' /><panel id='{open}-fill' class='fill' /><text id='{open}-count' class='caption' text='×{}' /></button>",icons::icon_in(&open,icon,tint),escape(&label),escape(&diagnostic.code.to_string()),escape(&diagnostic.message),entry["count"]));
                for (fix_index, fix) in diagnostic.fixes.iter().enumerate() {
                    out.push_str(&self.bind_button(
                        &format!("fix-{index}-{fix_index}"),
                        &fix.title,
                        Binding::Fix(diagnostic.clone(), fix_index),
                        "primary",
                    ));
                }
                out.push_str("</panel>");
            }
        }
        if shown == 0 {
            out.push_str("</scroll>");
            self.empty(out, "console-empty", "circle-check", if query.is_empty() { "No problems" } else { "No matching messages" }, "Diagnostics from the engine, importers and agents appear here, with one-click fixes.", &[]);
            return;
        }
        out.push_str("</scroll>");
    }
}
