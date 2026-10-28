//! Bounded inventory and creation affordances; values stay in the normal Inspector.
use super::*;

impl Shell {
    pub(super) fn lighting_panel(&mut self,editor:&mut Editor,out:&mut String)->ara_ui::Result<()> {
        let resolved=self.resolved(editor)?;
        let query=editor.state.searches.get("lighting").map_or(String::new(),|query|query.trim().to_lowercase());
        if query!=self.lighting.query { self.lighting.query=query.clone(); self.lighting.start=0; }
        let rows=model::rows(&resolved,editor,&query);
        let max_start=rows.len().saturating_sub(1)/PAGE*PAGE;
        self.lighting.start=self.lighting.start.min(max_start);
        let start=self.lighting.start;
        let end=(start+PAGE).min(rows.len());
        let inspect=editor.state.selection.first().and_then(|entity|resolved.get(entity)).is_some_and(|row|model::lighting(&row.record));
        let preset=!editor.state.selection.is_empty() && editor.state.selection.iter().all(|entity|resolved.get(entity).is_some_and(|row|row.record.contains_key("PostProcessVolume")));
        out.push_str("<panel id='lighting-overview' class='lighting-overview'><panel id='lighting-toolbar' class='lighting-toolbar'>");
        self.search(out,"lighting",editor);
        self.bind_command("lighting-inspect-selection","lighting.inspect",Target::None);
        out.push_str(&format!("<button id='lighting-inspect-selection' class='lighting-button' text='Inspect selection' disabled='{}' description='Select a light, sky, height fog or post volume to show it in the schema Inspector' action='Dispatch' /></panel><scroll id='lighting-scroll' class='lighting-scroll'><panel id='lighting-create' class='lighting-card'><text id='lighting-create-title' class='lighting-title' text='Place lights and volumes' /><panel id='lighting-create-actions' class='lighting-toolbar'>",!inspect));
        for (id,command,label) in [
            ("lighting-create-sun","create.light.sun","Sun"),("lighting-create-point","create.light.point","Point light"),("lighting-create-spot","create.light.spot","Spot light"),
            ("lighting-create-global","create.post.global","Global post volume"),("lighting-create-box","create.post.box","Local box"),("lighting-create-sphere","create.post.sphere","Local sphere"),("lighting-create-fog","create.fog","Height fog"),
        ] {
            self.bind_command(id,command,Target::None);
            out.push_str(&format!("<button id='{id}' class='lighting-button' text='{label}' action='Dispatch' />"));
        }
        out.push_str("</panel><text id='lighting-create-help' class='lighting-help' text='Post volumes start without overrides. Select a volume, then add override groups in Inspector. Height Fog is scene-wide; the first visible fog entity in ID order wins.' /></panel>");
        if let Some(post)=model::blended(&resolved,editor) {
            out.push_str(&format!("<panel id='lighting-camera-blend' class='lighting-card'><text id='lighting-camera-title' class='lighting-title' text='Authored blend at Scene camera' /><text id='lighting-camera-values' class='lighting-help' text='{} exposure · EV100 {:.2} · compensation {:+.2} stops · bloom {:.1}% · vignette {:.2}' /><text id='lighting-camera-help' class='lighting-help' text='Volumes blend in ascending priority; equal priorities use entity ID order.' /></panel>",if post.exposure==ara_components::render::ExposureMode::Manual {"Manual"}else{"Automatic"},post.ev100,post.compensation,post.bloom*100.,post.vignette));
        }
        out.push_str("<panel id='lighting-presets' class='lighting-card'><text id='lighting-presets-title' class='lighting-title' text='Add overrides to selected volumes' /><panel id='lighting-preset-actions' class='lighting-toolbar'>");
        for (id,command,label,description) in [
            ("lighting-preset-manual","lighting.preset.manual","Manual exposure","Append an exposure group at EV100 7; existing groups stay intact"),
            ("lighting-preset-interior","lighting.preset.interior","Interior look","Append EV100 7, 4500 K white balance and 4% bloom as three groups in one undoable action"),
        ] {
            self.bind_command(id,command,Target::None);
            out.push_str(&format!("<button id='{id}' class='lighting-button' text='{label}' disabled='{}' description='{description}; select post-process volumes first' action='Dispatch' />",!preset));
        }
        out.push_str("</panel><text id='lighting-presets-help' class='lighting-help' text='Presets append explicit groups. Unchecked optional properties continue to inherit; edit or remove groups in Inspector.' /></panel><text id='lighting-inventory-title' class='lighting-title' text='Scene lighting' />");
        if rows.is_empty() {
            out.push_str("<text id='lighting-none' class='lighting-help' text='No matching lighting entities. Search the whole inventory by name, entity ID or component type, or create lighting above.' />");
        }
        for row in &rows[start..end] {
            let id=format!("lighting-entry-{:016x}",ara_ui::source::stable_id(&row.id));
            self.bind_command(&id,"lighting.inspect",Target::Entity(row.id.clone()));
            out.push_str(&format!("<button id='{id}' class='lighting-entry{}' label='Inspect {}' description='{} · {}' action='Dispatch'><panel id='{id}-heading' class='row'>{}<text id='{id}-name' class='lighting-title' text='{}' /></panel><text id='{id}-detail' class='lighting-help' text='{}' /><text id='{id}-identity' class='lighting-identity' text='{}' /></button>",if editor.state.selection.contains(&row.id){" lighting-selected"}else{""},escape(&row.name),escape(&row.id),escape(&row.description),icons::icon(&id,row.icon),escape(&row.name),escape(&row.description),escape(&row.id)));
        }
        out.push_str("</scroll><panel id='lighting-pages' class='lighting-toolbar lighting-pages'>");
        for (id,command,label,enabled) in [("lighting-previous","lighting.page.previous","Previous",start>0),("lighting-next","lighting.page.next","Next",end<rows.len())] {
            self.bind_command(id,command,Target::None);
            out.push_str(&format!("<button id='{id}' class='lighting-button' text='{label}' disabled='{}' action='Dispatch' />",!enabled));
        }
        out.push_str(&format!("<text id='lighting-page-count' class='lighting-help' text='{}–{} of {} matches' /></panel></panel>",if rows.is_empty(){0}else{start+1},end,rows.len()));
        Ok(())
    }
}
