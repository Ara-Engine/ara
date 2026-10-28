use super::*;
use input::{command, row, submenu};
use proof::require;

impl HierarchyCheck {
    pub(super) fn flags(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let descendants = [FIRST, CHILD, LINKED, self.linked_inner.as_str()];
        match self.phase {
            1 => {
                self.click_at(
                    window,
                    row(shell, FIRST),
                    scale,
                    false,
                    "first selected root",
                    events,
                )?;
            }
            2 => {
                if let Some(at) = row(shell, LINKED) {
                    let primary = if cfg!(target_os = "macos") { 227 } else { 224 };
                    native::key(window, primary, true, events)?;
                    input::click(window, at, scale, false, events)?;
                    native::key(window, primary, false, events)?;
                    self.next();
                }
            }
            3 | 5 => self.key(window, 11, false, events)?, // H, selected set only.
            4 => {
                require(
                    proof::flags(&editor.state.hidden, &self.before.hidden, &[FIRST, LINKED]),
                    "Hide Selection affected descendants or skipped a selected root",
                )?;
                require(
                    !editor.state.hidden.contains(CHILD)
                        && !editor.state.hidden.contains(&self.linked_inner),
                    "Selected-only hide silently hid unselected children",
                )?;
                proof::unchanged(self, editor)?;
                self.mark(
                    "interaction",
                    editor,
                    shell,
                    json!({"selectedOnlyHidden":[FIRST,LINKED],"childrenFlagsUnchanged":true}),
                );
                self.next();
            }
            6 => {
                require(
                    editor.state.hidden == self.before.hidden,
                    "Mixed selection Hide toggle did not restore the complete set",
                )?;
                self.click_at(
                    window,
                    row(shell, OTHER),
                    scale,
                    true,
                    "unselected outsider context",
                    events,
                )?;
            }
            7 | 10 | 13 => {
                self.click_at(
                    window,
                    command(shell, "edit.lock", false),
                    scale,
                    false,
                    "Entity > Lock Selection",
                    events,
                )?;
            }
            8 => {
                require(
                    proof::flags(&editor.state.locked, &self.before.locked, &[OTHER]),
                    "Context Lock changed the unrelated selected roots",
                )?;
                let selected: BTreeSet<_> =
                    editor.state.selection.iter().map(String::as_str).collect();
                require(
                    selected == BTreeSet::from([FIRST, LINKED]),
                    "Hierarchy context Lock replaced the unrelated selection",
                )?;
                self.mark(
                    "context-lock",
                    editor,
                    shell,
                    json!({"clicked":OTHER,"onlyClickedLocked":true}),
                );
                self.next();
            }
            9 | 12 | 15 | 19 | 23 | 27 => {
                self.click_at(
                    window,
                    row(shell, FIRST),
                    scale,
                    true,
                    "selected branch context",
                    events,
                )?;
            }
            11 => {
                require(
                    proof::flags(
                        &editor.state.locked,
                        &self.before.locked,
                        &[OTHER, FIRST, LINKED],
                    ),
                    "Lock Selection did not affect exactly the selected set",
                )?;
                require(
                    !editor.state.locked.contains(CHILD)
                        && !editor.state.locked.contains(&self.linked_inner),
                    "Selected-only Lock silently locked descendants",
                )?;
                self.mark(
                    "selected-lock",
                    editor,
                    shell,
                    json!({"selectedOnlyLocked":[FIRST,LINKED],"outsiderRemainsLocked":true}),
                );
                self.next();
            }
            14 => {
                require(
                    proof::flags(&editor.state.locked, &self.before.locked, &[OTHER]),
                    "Unlock Selection changed an unrelated locked object",
                )?;
                self.next();
            }
            16 | 20 | 24 | 28 => {
                self.click_at(
                    window,
                    submenu(shell, "Selected Branches"),
                    scale,
                    false,
                    "Entity > Selected Branches",
                    events,
                )?;
            }
            17 => {
                self.click_at(
                    window,
                    command(shell, "edit.hide_descendants", false),
                    scale,
                    false,
                    "Hide Selection and Descendants",
                    events,
                )?;
            }
            18 => {
                require(
                    proof::flags(&editor.state.hidden, &self.before.hidden, &descendants),
                    "Hide branches did not cover the exact selected descendant closure",
                )?;
                proof::unchanged(self, editor)?;
                self.mark(
                    "branch-hidden",
                    editor,
                    shell,
                    json!({"hiddenClosure":descendants}),
                );
                self.next();
            }
            21 => {
                self.click_at(
                    window,
                    command(shell, "edit.unhide_descendants", false),
                    scale,
                    false,
                    "Show Selection and Descendants",
                    events,
                )?;
            }
            22 => {
                require(
                    editor.state.hidden == self.before.hidden,
                    "Show branches did not restore the hidden set",
                )?;
                self.next();
            }
            25 => {
                self.click_at(
                    window,
                    command(shell, "edit.lock_descendants", false),
                    scale,
                    false,
                    "Lock Selection and Descendants",
                    events,
                )?;
            }
            26 => {
                let all = [OTHER, FIRST, CHILD, LINKED, self.linked_inner.as_str()];
                require(
                    proof::flags(&editor.state.locked, &self.before.locked, &all),
                    "Lock branches did not cover the exact selected descendant closure",
                )?;
                proof::unchanged(self, editor)?;
                self.mark(
                    "branch-locked",
                    editor,
                    shell,
                    json!({"lockedClosure":descendants,"unrelatedLockPreserved":OTHER}),
                );
                self.next();
            }
            29 => {
                self.click_at(
                    window,
                    command(shell, "edit.unlock_descendants", false),
                    scale,
                    false,
                    "Unlock Selection and Descendants",
                    events,
                )?;
            }
            _ => {
                require(
                    proof::flags(&editor.state.locked, &self.before.locked, &[OTHER]),
                    "Unlock branches changed an unrelated locked object",
                )?;
                require(
                    editor.state.hidden == self.before.hidden,
                    "Branch actions left unexpected hidden objects",
                )?;
                proof::unchanged(self, editor)?;
                self.pass();
            }
        }
        Ok(())
    }
}
