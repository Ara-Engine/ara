//! A real 100k-row scene, with bounded retained geometry and compact state assertions.
use super::*;
use proof::require;

impl HierarchyCheck {
    pub(super) fn stress(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        match self.phase {
            1 => {
                self.click_at(
                    window,
                    input::row(shell, &fixture::row(2)),
                    scale,
                    false,
                    "100k row 2",
                    events,
                )?;
            }
            2 => self.key(window, 77, true, events)?, // Ctrl+End moves focus, preserving the range anchor.
            3 => {
                require(
                    editor.state.selection == [fixture::row(2)],
                    "Ctrl+End replaced the selected anchor",
                )?;
                if proof::focus(shell).as_deref() != Some(fixture::row(COUNT - 1).as_str()) {
                    return Ok(());
                }
                if let Some(at) = input::row(shell, &fixture::row(COUNT - 1)) {
                    native::key(window, 225, true, events)?;
                    input::click(window, at, scale, false, events)?;
                    native::key(window, 225, false, events)?;
                    self.trace.push(json!({"input":"native-shift-click","row":COUNT-1,"point":at,"priorAnchor":2}));
                    self.next();
                }
            }
            4 => {
                require(
                    proof::exact_range(editor, 2, COUNT),
                    "Shift-click did not select the complete range across 100k virtual rows",
                )?;
                require(
                    proof::focus(shell).as_deref() == Some(fixture::row(COUNT - 1).as_str())
                        && shell.hierarchy_offset > 0.,
                    "The distant range endpoint did not retain native keyboard focus",
                )?;
                require(
                    proof::row_ids(shell).len() < 256 && shell.ui.nodes.len() < 2000,
                    "The 100k hierarchy expanded into an unbounded retained row tree",
                )?;
                require(
                    editor.host.store().revision() == self.before.revision,
                    "A 100k range selection authored content ops",
                )?;
                self.mark("interaction",editor,shell,json!({"rows":COUNT,"selection":proof::selection(editor),"expectedRange":[2,COUNT-1],"visibleRows":proof::row_ids(shell).len(),"retainedNodes":shell.ui.nodes.len()}));
                self.next();
            }
            5 => {
                native::key(window, 225, true, events)?;
                native::tap(window, 82, events)?;
                native::key(window, 225, false, events)?;
                self.next();
            }
            6 => {
                require(
                    proof::exact_range(editor, 2, COUNT - 1),
                    "Shift+Up did not extend/shrink the virtual range from its original anchor",
                )?;
                require(
                    proof::focus(shell).as_deref() == Some(fixture::row(COUNT - 2).as_str()),
                    "Shift+Up lost the intended virtual row focus",
                )?;
                self.next();
            }
            7 => self.key(window, 41, false, events)?,
            8 => {
                require(
                    editor.state.selection == [fixture::row(2)]
                        && proof::focus(shell).as_deref() == Some(fixture::row(COUNT - 1).as_str()),
                    "Escape did not restore the pre-range selection and keyboard focus",
                )?;
                self.mark("range-escape",editor,shell,json!({"restoredSelection":2,"restoredPreRangeFocus":COUNT-1,"contentUnchanged":editor.host.store().revision()==self.before.revision}));
                self.next();
            }
            9 => self.key(window, 74, true, events)?, // Ctrl+Home reveals the first rows without replacing selection.
            10 => {
                if let Some(at) = input::row(shell, &fixture::row(3)) {
                    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
                    native::key(window, primary, true, events)?;
                    input::click(window, at, scale, false, events)?;
                    native::key(window, primary, false, events)?;
                    self.next();
                }
            }
            11 => {
                let selected: BTreeSet<_> =
                    editor.state.selection.iter().map(String::as_str).collect();
                let second = fixture::row(2);
                let third = fixture::row(3);
                require(
                    selected == BTreeSet::from([second.as_str(), third.as_str()]),
                    "Ctrl-click did not toggle the intended 100k row",
                )?;
                self.click_at(
                    window,
                    input::row(shell, &second),
                    scale,
                    false,
                    "100k anchor before deletion",
                    events,
                )?;
            }
            12 => self.key(window, 76, false, events)?,
            13 => {
                let doc = editor
                    .host
                    .store()
                    .workspace()
                    .doc(STRESS)
                    .ok_or("Stress scene disappeared")?;
                require(
                    doc.records().len() == COUNT - 1 && doc.record(&fixture::row(2)).is_none(),
                    "Native Delete did not remove exactly its selected anchor",
                )?;
                if let Some(at) = input::row(shell, &fixture::row(6)) {
                    native::key(window, 225, true, events)?;
                    input::click(window, at, scale, false, events)?;
                    native::key(window, 225, false, events)?;
                    self.next();
                }
            }
            _ => {
                require(
                    editor.state.selection == [fixture::row(6)]
                        && proof::focus(shell).as_deref() == Some(fixture::row(6).as_str()),
                    "A deleted 100k anchor created a stale range or lost focus",
                )?;
                require(
                    editor.host.store().revision() == self.before.revision + 1,
                    "Selection gestures created content changes beyond the one native Delete",
                )?;
                require(
                    editor.state.camera == self.before.camera && editor.state.doc == STRESS,
                    "100k hierarchy navigation changed the camera or active document",
                )?;
                require(
                    shell.input.captured().is_none()
                        && !shell.content_pending()
                        && !shell.tab_pending(),
                    "100k hierarchy interaction left pointer capture active",
                )?;
                self.pass();
            }
        }
        Ok(())
    }
}
