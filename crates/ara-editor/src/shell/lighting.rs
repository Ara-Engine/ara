//! Schema-backed lighting inventory; current-selection editing uses the shared Inspector.
use super::*;
use commands::Target;
use ara_components::post::{PostProcessVolume,PostVolume,VolumeShape};
use glam::Vec3;
mod authoring;
pub(in crate::shell) mod bounds;
mod model;
mod view;
#[cfg(test)]
mod tests;

pub(crate) use authoring::{Create,Preset};
pub(super) const CSS:&str=include_str!("lighting/lighting.css");
const PAGE:usize=64;

/// Local reading position only; authoring and selection remain typed authority operations.
#[derive(Default)]
pub(crate) struct View {
    start:usize,
    query:String,
}

impl Shell {
    /// Show the scene-wide lighting inventory and its existing schema Inspector.
    pub(crate) fn open_lighting(&mut self,editor:&mut Editor)->ara_ui::Result<()> {
        editor.action("editor.layout.show",json!({"panel":"lighting"}))?;
        editor.action("editor.layout.show",json!({"panel":"inspector"}))?;
        editor.action("editor.focus",json!({"panel":"lighting"}))?;
        self.rebuild(editor)
    }

    /// A row explicitly selects its real entity; background edits never move the reading position.
    pub(crate) fn inspect_lighting(&mut self,editor:&mut Editor,target:&Target)->ara_ui::Result<()> {
        let entity=target.entity(&editor.state).ok_or("Select a light, atmosphere, height fog or post-process volume")?.to_owned();
        let resolved=self.resolved(editor)?;
        if resolved.get(&entity).is_none_or(|row|!model::lighting(&row.record)) {
            return Err("That entity no longer has a lighting component; select a current row".into());
        }
        editor.action("editor.select",json!({"entities":[entity]}))?;
        self.reveal_lighting(editor,&entity)?;
        self.open_lighting(editor)
    }

    fn reveal_lighting(&mut self,editor:&mut Editor,entity:&str)->ara_ui::Result<()> {
        let resolved=self.resolved(editor)?;
        let query=editor.state.searches.get("lighting").map_or(String::new(),|query|query.trim().to_lowercase());
        let filtered=model::rows(&resolved,editor,&query);
        let position=if let Some(position)=filtered.iter().position(|row|row.id==entity) { position }
        else {
            editor.action("editor.search",json!({"panel":"lighting","query":""}))?;
            model::rows(&resolved,editor,"").iter().position(|row|row.id==entity).unwrap_or(0)
        };
        self.lighting.query=editor.state.searches.get("lighting").map_or(String::new(),|query|query.trim().to_lowercase());
        self.lighting.start=position/PAGE*PAGE;
        Ok(())
    }

    /// Page all matching model records; search is applied before the mounted-row bound.
    pub(crate) fn page_lighting(&mut self,editor:&mut Editor,next:bool)->ara_ui::Result<()> {
        if next { self.lighting.start=self.lighting.start.saturating_add(PAGE); }
        else { self.lighting.start=self.lighting.start.saturating_sub(PAGE); }
        self.rebuild(editor)
    }

    /// Focused lighting fields use exactly the existing enum/optional/collection/scrub controls.
    pub(super) fn lighting_inspector(&mut self,editor:&mut Editor,out:&mut String)->ara_ui::Result<()> {
        self.nested_swatches.clear();
        let resolved=self.resolved(editor)?;
        let selected_lighting=editor.state.selection.iter().any(|entity|resolved.get(entity).is_some_and(|row|model::lighting(&row.record)));
        if !selected_lighting {
            self.empty(out,"lighting-inspector-empty","sun","Select lighting","Select a light, sky, height fog or post-process volume in Lighting or Hierarchy. The fields use the engine schema and normal Undo.",&[]);
            return Ok(());
        }
        let document=editor.host.store().workspace().doc(&editor.state.doc).ok_or("The lighting scene is unavailable")?;
        let fields=crate::inspector::fields(editor.host.store().schema(),document,&resolved,&editor.state.selection,&editor.agent_fields());
        let width=self.shown_node("inspector-scroll").or_else(||self.shown_node("body-inspector")).map_or(320.,|index|self.ui.nodes[index].rect.w);
        let label=self.label_px(width);
        self.bind_command("lighting-inspector-full","window.inspector",Target::None);
        out.push_str("<panel id='lighting-inspector-heading' class='lighting-heading'><text id='lighting-inspector-title' class='lighting-title' text='Lighting selection' /><text id='lighting-inspector-help' class='lighting-help' text='Local volume size and orientation come from Transform. Optional override checkboxes inherit when unchecked; each edited value uses normal Undo.' /><button id='lighting-inspector-full' class='lighting-button' text='All components' description='Show the full Inspector, including component add, remove and reorder' action='Dispatch' /></panel><scroll id='inspector-scroll' style='min-height:0px;flex-grow:1;flex-basis:0px'><panel id='inspector-components' class='inspector-components'>");
        let mut component=String::new();
        for field in fields.into_iter().filter(|field|model::component(field.path.split('.').next().unwrap_or("")) || field.path.starts_with("Transform.")) {
            let head=field.path.split('.').next().unwrap_or("");
            if head!=component {
                if !component.is_empty() { out.push_str("</panel>"); }
                component=head.into();
                out.push_str(&format!("<panel id='lighting-section-{}' class='component-section'><text id='lighting-heading-{}' class='lighting-component-title' text='{}' />",escape(head),escape(head),escape(&inspector_panel::sentence(head))));
            }
            self.property_row(editor,&field,label,out);
            self.fields.insert(field.path.clone(),field);
        }
        if !component.is_empty() { out.push_str("</panel>"); }
        out.push_str("</panel></scroll>");
        Ok(())
    }

}
