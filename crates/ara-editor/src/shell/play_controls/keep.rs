//! Exact field selection is an authoring input, not a deferred approval queue.
use super::*;
use serde::Deserialize;
use std::collections::BTreeSet;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Row {
    entity: String,
    path: String,
    eligible: bool,
    driven: bool,
    play_start: Value,
    play: Value,
    edit: Value,
    status: String,
    reason: Option<String>,
}
pub(super) struct Choice {
    world: String,
    entities: Vec<String>,
    rows: Vec<Row>,
    selected: BTreeSet<(String, String)>,
    include_driven: bool,
}
impl Choice {
    pub(super) fn read(
        world: String,
        entities: Vec<String>,
        include_driven: bool,
        value: &Value,
    ) -> ara_ui::Result<Self> {
        let rows: Vec<Row> = serde_json::from_value(
            value
                .get("keepCandidates")
                .ok_or("Runtime diff did not include Keep eligibility")?
                .clone(),
        )?;
        if rows.iter().any(|row| !entities.contains(&row.entity)) {
            return Err("Runtime diff returned fields outside the chosen entities".into());
        }
        let mut identities = BTreeSet::new();
        if rows
            .iter()
            .any(|row| !identities.insert((&row.entity, &row.path)))
        {
            return Err("Runtime diff returned the same entity field twice".into());
        }
        Ok(Self {
            world,
            entities,
            rows,
            selected: BTreeSet::new(),
            include_driven,
        })
    }
    fn enabled(&self, row: &Row) -> bool {
        row.eligible && (!row.driven || self.include_driven)
    }
}
impl Shell {
    pub(super) fn toggle_keep_field(
        &mut self,
        editor: &mut Editor,
        entity: &str,
        path: &str,
        value: Value,
    ) -> ara_ui::Result<()> {
        let selected = value
            .as_bool()
            .ok_or("Keep field selection requires a checkbox value")?;
        let choice = self
            .play_controls
            .keep
            .as_mut()
            .ok_or("Refresh the runtime field list first")?;
        let row = choice
            .rows
            .iter()
            .find(|row| row.entity == entity && row.path == path)
            .ok_or("That runtime field is no longer in the list")?;
        if !choice.enabled(row) {
            return Err(
                "This runtime field cannot be kept; its current scene value is preserved".into(),
            );
        }
        let key = (entity.into(), path.into());
        if selected {
            choice.selected.insert(key);
        } else {
            choice.selected.remove(&key);
        }
        self.rebuild(editor)
    }
    pub(super) fn toggle_keep_driven(
        &mut self,
        editor: &mut Editor,
        value: Value,
    ) -> ara_ui::Result<()> {
        let include = value
            .as_bool()
            .ok_or("Driven fields require a checkbox value")?;
        let choice = self
            .play_controls
            .keep
            .as_mut()
            .ok_or("Refresh the runtime field list first")?;
        choice.include_driven = include;
        if !include {
            let rows = &choice.rows;
            choice.selected.retain(|(entity, path)| {
                rows.iter()
                    .any(|row| &row.entity == entity && &row.path == path && !row.driven)
            });
        }
        self.rebuild(editor)
    }
    /// The only Keep apply path: exact pairs are captured before the async job;
    /// the host obtains a fresh diff and rechecks authored conflicts at commit.
    pub(crate) fn apply_keep_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.play_idle(editor)?;
        let choice = self
            .play_controls
            .keep
            .as_ref()
            .ok_or("Choose runtime fields before applying Keep")?;
        if editor.play_world.as_deref() != Some(choice.world.as_str()) {
            return Err("The Play world changed; refresh its runtime changes".into());
        }
        if choice.selected.is_empty() {
            return Err("Tick at least one eligible runtime field to keep".into());
        }
        let fields = choice
            .selected
            .iter()
            .map(|(entity, path)| json!({"entity":entity,"path":path}))
            .collect::<Vec<_>>();
        let include_driven = choice.rows.iter().any(|row| {
            row.driven
                && choice
                    .selected
                    .contains(&(row.entity.clone(), row.path.clone()))
        });
        self.play_job(editor,"play.keep",json!({"fields":fields,"includeDriven":include_driven,"why":"Keep explicitly selected runtime fields"}),jobs::Phase::Keep)?;
        self.rebuild(editor)
    }
    pub(crate) fn cancel_keep_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.play_idle(editor)?;
        self.play_controls.keep = None;
        self.rebuild(editor)
    }
    pub(crate) fn refresh_keep_fields(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        self.play_idle(editor)?;
        let choice = self
            .play_controls
            .keep
            .as_ref()
            .ok_or("Open runtime changes before refreshing")?;
        if editor.play_world.as_deref() != Some(choice.world.as_str()) {
            return Err("The Play world changed; choose its entities and Keep again".into());
        }
        self.play_job(
            editor,
            "play.diff",
            json!({"entities":choice.entities}),
            jobs::Phase::KeepPreview {
                entities: choice.entities.clone(),
                include_driven: choice.include_driven,
            },
        )?;
        self.rebuild(editor)
    }
    pub(super) fn keep_fields_report(&self) -> Value {
        self.play_controls.keep.as_ref().map_or(Value::Null,|choice|json!({"world":choice.world,"entities":choice.entities,
            "includeDriven":choice.include_driven,"selected":choice.selected.iter().map(|(entity,path)|json!({"entity":entity,"path":path})).collect::<Vec<_>>(),
            "fields":choice.rows.iter().map(|row|json!({"entity":row.entity,"path":row.path,"eligible":row.eligible,"driven":row.driven,"status":row.status,
                "reason":row.reason,"playStart":row.play_start,"play":row.play,"edit":row.edit})).collect::<Vec<_>>()}))
    }
    pub(super) fn keep_fields_markup(&mut self, editor: &Editor, out: &mut String) {
        let Some(choice) = self.play_controls.keep.take() else {
            return;
        };
        let pending = self.play_controls.pending.is_some();
        let current = editor.play_world.as_deref() == Some(choice.world.as_str());
        out.push_str("<panel id='keep-fields' class='notice' style='flex-direction:column;gap:6px;min-width:0px'><text id='keep-fields-title' class='heading' text='Choose authority fields to save to the scene' /><text id='keep-fields-hint' class='caption' style='white-space:normal' text='Tick exact fields below. Authored conflicts and unsupported changes stay unchanged. Apply saves the selected fields in one Undo step.' />");
        self.bindings.insert(
            ara_ui::source::stable_id("keep-fields-driven"),
            Binding::Play(Action::KeepDriven),
        );
        out.push_str(&format!("<toggle id='keep-fields-driven' checked='{}' text='Include values written every tick' label='Include values written every tick' style='width:100%;height:auto;min-height:24px;padding-right:28px;white-space:normal' description='Driven fields such as physics poses require an explicit choice; they may immediately change again in Play' action='Dispatch'{} />",choice.include_driven,if pending {" disabled='true'"} else {""}));
        out.push_str("<scroll id='keep-fields-scroll' style='max-height:280px;flex-basis:auto;flex-shrink:1;min-height:0px'><panel id='keep-fields-rows' style='flex-direction:column;gap:6px;min-width:0px'>");
        if choice.rows.is_empty() {
            out.push_str("<text id='keep-fields-empty' class='caption' text='No runtime differences for these entities.' />");
        }
        for row in &choice.rows {
            let id = format!(
                "keep-field-{:016x}",
                ara_ui::source::stable_id(&format!("{}#{}", row.entity, row.path))
            );
            let selected = choice
                .selected
                .contains(&(row.entity.clone(), row.path.clone()));
            let enabled = choice.enabled(row) && current && !pending;
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Play(Action::KeepField {
                    entity: row.entity.clone(),
                    path: row.path.clone(),
                }),
            );
            let label = format!(
                "{} · {}{}",
                row.entity,
                row.path,
                if row.driven { " · driven" } else { "" }
            );
            let reason = row
                .reason
                .as_deref()
                .unwrap_or(if row.driven && !choice.include_driven {
                    "Enable driven fields to select this value"
                } else {
                    "Save this exact field"
                });
            out.push_str(&format!("<panel id='{id}-row' style='flex-direction:column;gap:2px;min-width:0px'><toggle id='{id}' checked='{selected}' text='{}' label='{}' style='width:100%;height:auto;min-height:24px;padding-right:28px;white-space:normal' description='{}' action='Dispatch'{} /><text id='{id}-values' class='caption' style='white-space:normal' text='{}' />",escape(&label),escape(&label),escape(reason),if enabled {""} else {" disabled='true'"},escape(&format!("Start: {} · Scene: {} · Play: {}",value(&row.play_start),value(&row.edit),value(&row.play)))));
            if !enabled && !pending {
                out.push_str(&format!("<text id='{id}-reason' class='caption' style='white-space:normal' text='{}' />",escape(if current {reason} else {"The Play world changed"})));
            }
            out.push_str("</panel>");
        }
        out.push_str("</panel></scroll><panel id='keep-fields-actions' class='row' style='gap:6px;flex-wrap:wrap'>");
        for (id, command, label, disabled) in [
            (
                "keep-fields-refresh",
                "play.keep.refresh",
                "Refresh",
                pending || !current,
            ),
            ("keep-fields-cancel", "play.keep.cancel", "Cancel", pending),
            (
                "keep-fields-apply",
                "play.keep.apply",
                "Apply selected",
                pending || !current || choice.selected.is_empty(),
            ),
        ] {
            self.bind_command(id, command, commands::Target::None);
            out.push_str(&format!(
                "<button id='{id}' class='ghost' text='{label}' action='Dispatch'{} />",
                if disabled { " disabled='true'" } else { "" }
            ));
        }
        out.push_str("</panel></panel>");
        self.play_controls.keep = Some(choice);
    }
}
fn value(value: &Value) -> String {
    if value.is_null() {
        "default / unset".into()
    } else {
        let text = super::super::panels::display(value);
        if text.chars().count() > 120 {
            format!("{}…", text.chars().take(120).collect::<String>())
        } else {
            text
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_read_only_diff_never_selects_fields_and_rejects_ambiguous_or_wrong_scope_rows() {
        let row = json!({"entity":"e_one","path":"Light.intensity","eligible":true,"driven":false,"playStart":1000,"play":2500,"edit":1000,"status":"eligible","reason":null});
        let choice = Choice::read(
            "play:1".into(),
            vec!["e_one".into()],
            false,
            &json!({"keepCandidates":[row.clone()]}),
        );
        assert!(choice.is_ok_and(|choice| choice.selected.is_empty()));
        assert!(
            Choice::read(
                "play:1".into(),
                vec!["e_other".into()],
                false,
                &json!({"keepCandidates":[row.clone()]})
            )
            .is_err()
        );
        assert!(
            Choice::read(
                "play:1".into(),
                vec!["e_one".into()],
                false,
                &json!({"keepCandidates":[row.clone(),row]})
            )
            .is_err()
        );
        assert!(
            Choice::read(
                "play:1".into(),
                vec!["e_one".into()],
                false,
                &json!({"changes":[]})
            )
            .is_err()
        );
    }
}
