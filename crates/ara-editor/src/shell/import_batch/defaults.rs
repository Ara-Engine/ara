//! Project scope uses the same reflected fields as per-source importers.
use super::*;
impl Shell {
    pub(crate) fn import_defaults_open(&self) -> bool {
        self.import_batch.defaults_open
    }
    pub(crate) fn import_defaults(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.commit_import_fields(editor)?;
        self.import_batch.defaults_open = true;
        if self.import_batch.default_type.is_empty() {
            self.import_batch.default_type = "ara.texture".into();
        }
        self.menu = None;
        editor.action("editor.palette", json!({"open":false}))?;
        editor.action("editor.layout.show", json!({"panel":"inspector"}))?;
        editor.action("editor.focus", json!({"panel":"inspector"}))?;
        self.rebuild(editor)
    }
    pub(crate) fn close_import_defaults(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.commit_import_fields(editor)?;
        self.import_batch.defaults_open = false;
        self.rebuild(editor)
    }
    pub(crate) fn reset_import_defaults(
        &mut self,
        editor: &mut Editor,
        target: &crate::shell::commands::Target,
    ) -> ara_ui::Result<()> {
        let importer = match target {
            crate::shell::commands::Target::Text(value) => value.as_str(),
            _ => self.import_batch.default_type.as_str(),
        };
        let importer = format!("ara.{}", commands::type_key(importer)?);
        let prefix = format!("@import-batch|@default:{importer}|");
        self.drafts.retain(|key, _| !key.starts_with(&prefix));
        commands::apply_changes(
            editor,
            vec![Change::Reset(importer)],
            "Reset importer type defaults",
        )?;
        self.rebuild(editor)
    }
    pub(crate) fn import_defaults_panel(
        &mut self,
        editor: &Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let importer = self.import_batch.default_type.clone();
        settings(&importer)?;
        out.push_str("<scroll id='import-defaults-inspector' style='flex-grow:1;flex-basis:0px;min-height:0px;padding:8px;gap:8px'><text id='import-defaults-title' class='heading' text='Project import defaults' /><text id='import-defaults-scope' class='caption' style='white-space:normal' text='Explicit source fields take priority, then these typed defaults, then built-in importer values. Existing sources which inherit changed fields are reimported; each completed edit has one Undo.' />");
        self.choice_button(
            "import-defaults-type",
            "Importer type",
            choices::Select {
                destination: choices::Destination::ImportGroup,
                current: json!(importer),
                options: ["ara.texture", "ara.gltf", "ara.fbx", "ara.audio"]
                    .into_iter()
                    .map(|value| (json!(value), view::label(value).into()))
                    .collect(),
            },
            out,
        );
        self.import_fields(
            editor,
            &format!("@default:{importer}"),
            &Group {
                importer: importer.clone(),
                sources: Vec::new(),
                project: true,
            },
            out,
        )?;
        out.push_str(&self.bind_button(
            "import-defaults-reset",
            "Reset this type to built-in defaults",
            Binding::Command(
                "assets.import_defaults.reset",
                crate::shell::commands::Target::Text(importer),
            ),
            "quiet",
        ));
        out.push_str(&self.bind_button(
            "import-defaults-close",
            "Back to selection",
            Binding::Command(
                "assets.import_defaults.close",
                crate::shell::commands::Target::None,
            ),
            "quiet",
        ));
        out.push_str("</scroll>");
        Ok(())
    }
}
