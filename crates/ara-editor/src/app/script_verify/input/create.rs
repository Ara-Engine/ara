//! The Inspector's actual New Script route, inline SDL rename and compiled attach.
use super::*;
impl ScriptCheck {
    pub(super) fn create(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => {
                native::focus_lost(window, events)?;
                let row = shell
                    .ui
                    .nodes
                    .iter()
                    .find(|node| {
                        matches!(shell.bindings.get(&ara_ui::source::stable_id(&node.element.id)),
                    Some(Binding::Select(entity)) if entity==ENTITY)
                    })
                    .map(|node| node.element.id.clone());
                let Some(row) = row else { return Ok(()) };
                if self.reveal(shell, window, scale, &row, events)? {
                    self.click(shell, window, scale, &row, events)?;
                    self.next();
                }
            }
            2 => {
                if editor.state.selection != [ENTITY] {
                    return Ok(());
                }
                if self.reveal(shell, window, scale, "add-component", events)? {
                    self.click(shell, window, scale, "add-component", events)?;
                    self.next();
                }
            }
            3 => {
                if visible(shell, "add-component-script") {
                    self.click(shell, window, scale, "add-component-script", events)?;
                    self.next();
                }
            }
            4 => {
                if visible(shell, "asset-rename-value") {
                    self.click(shell, window, scale, "asset-rename-value", events)?;
                    native::shortcut(window, 4, events)?;
                    native::text(window, "Spinner", events)?;
                    self.next();
                }
            }
            5 => {
                native::tap(window, 40, events)?;
                self.next();
            }
            _ => {
                if editor.host.store().schema().get("Spinner").is_none() {
                    return Ok(());
                }
                let attached = crate::world::resolve(editor.host.store(), &self.doc)
                    .map_err(|e| format!("{e:?}"))?
                    .get(ENTITY)
                    .is_some_and(|entity| entity.record.contains_key("Spinner"));
                if !attached {
                    return Ok(());
                }
                let source = editor
                    .state
                    .asset_path
                    .as_deref()
                    .filter(|path| *path == "Spinner.cs" || path.ends_with("/Spinner.cs"))
                    .ok_or("Native rename did not retain the actual Spinner.cs asset")?;
                let source = editor.host.root().join(source);
                let field = editor
                    .host
                    .store()
                    .schema()
                    .get("Spinner")
                    .and_then(|ty| {
                        ty.fields()
                            .iter()
                            .find(|field| field.name.eq_ignore_ascii_case("speed"))
                    })
                    .map(|field| field.name.clone())
                    .ok_or("Compiled Spinner lacks its reflected Speed field")?;
                editor.apply(vec![json!({"cmd":"property.set","args":{"entity":ENTITY,"path":format!("Spinner.{field}"),"value":SPEED}})],
                    "Native J10 fixture: known ECS speed before Play (protocol setup)",None)?;
                self.proof = Some(Proof::new(editor, &source, ENTITY)?);
                self.source = Some(source.clone());
                self.done(json!({"createdVia":"Inspector Add component > New Script","rename":"SDL TextInput Spinner + Enter",
                    "compiledAndAttached":true,"entity":ENTITY,"source":source,"component":"Spinner",
                    "speedFixture":{"value":SPEED,"ingress":"typed property.set, not a native gesture"}}));
            }
        }
        Ok(())
    }
}
