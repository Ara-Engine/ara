use crate::{Material, ParameterValue, TextureSource};
use ara_diag::{Diagnostic, FixAction};
use ara_doc::refs::{AssetIndex, resolve};

pub(crate) fn repair(
    value: &mut ParameterValue,
    index: &AssetIndex,
) -> Result<Option<Diagnostic>, Diagnostic> {
    if let ParameterValue::Texture(TextureSource::Asset(reference)) = value {
        let (canonical, note) = resolve(&reference.0, None, index)?;
        reference.0 = canonical;
        Ok(note)
    } else {
        Ok(None)
    }
}
impl Material {
    /// Atomically fills/repairs texture @path#id references using ara-doc's index.
    /// Call during save/import, before persisting either canonical authoring view.
    pub fn resolve_assets(
        &mut self,
        index: &AssetIndex,
    ) -> Result<Vec<Diagnostic>, Vec<Diagnostic>> {
        let mut parameters = self.parameters.clone();
        let mut notes = Vec::new();
        let mut errors = Vec::new();
        for (id, p) in &mut parameters {
            match repair(&mut p.default,index){
                Ok(Some(note))=>notes.push(note.record(id).path("params.value").with_fix("Write the canonical asset reference",FixAction::Command{command:"graph.param.set".into(),params:serde_json::json!({"node":id,"name":"value","value":p.default})})),
                Ok(None)=>(),
                Err(d)=>errors.push(d.record(id).path("params.value").with_fix("Use a semantic fallback until the asset exists",FixAction::Command{command:"graph.param.set".into(),params:serde_json::json!({"node":id,"name":"value","value":ParameterValue::zero(p.ty)})})),
            }
        }
        if errors.is_empty() {
            self.parameters = parameters;
            Ok(notes)
        } else {
            Err(errors)
        }
    }
}
