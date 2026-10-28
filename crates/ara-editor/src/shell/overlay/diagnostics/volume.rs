//! Selection cages reuse the renderer-equivalent low-level lighting shape contract.
use super::*;
use std::collections::BTreeMap;
use glam::Mat4;
pub(super) fn collect(wires:&mut Vec<Segment>,volumes:&mut Vec<(String,f32)>,counts:&mut Counts,
    view:&DocumentView,state:&EditorState,transforms:Option<&BTreeMap<String,Mat4>>) {
    for id in &state.selection {
        if state.hidden.contains(id){continue}
        let Some(entity)=view.entities().get(id).filter(|entity|entity.record.contains_key("PostProcessVolume")) else {continue};
        let world=transforms.and_then(|transforms|transforms.get(id).copied()).or_else(||crate::world::world_transform(view.entities(),id));
        let Some(world)=world else {continue};
        let Some(volume)=crate::shell::lighting::bounds::from_entity(id,&entity.record,world) else {continue};
        for [from,to] in volume.segments() {wires.push(Segment{from,to,tint:Tint::Volume});}
        counts.volumes+=1;volumes.push((volume.entity,volume.blend_distance));
    }
}
