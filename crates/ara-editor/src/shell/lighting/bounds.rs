//! Renderer-equivalent local-volume cages; world lines are projected by the owning Scene surface.
use super::*;
use glam::Mat4;
#[cfg(test)]
use ara_doc::prefab::Resolved;
#[cfg(test)]
use std::collections::BTreeSet;

/// A selected finite shape, using the same decomposed world pose as volume blending.
pub(crate) struct VolumeBounds {
    pub entity:String,
    pub shape:VolumeShape,
    pub world:Mat4,
    /// Actual world-space falloff distance, separate from the solid shape cage.
    pub blend_distance:f32,
}
impl VolumeBounds {
    /// Exact box edges, or three transformed sphere great circles; no per-frame line allocation.
    pub(crate) fn segments(&self)->impl Iterator<Item=[Vec3;2]>+'_ {
        let count=match self.shape {VolumeShape::Global=>0,VolumeShape::Box=>12,VolumeShape::Sphere=>96};
        (0..count).map(move |index| {
            let points=if self.shape==VolumeShape::Box {
                let axis=index/4;
                let other=(axis+1)%3;
                let last=(axis+2)%3;
                let mut a=Vec3::splat(-0.5);
                if ((index%4)&1)!=0 {a[other]=0.5;}
                if ((index%4)&2)!=0 {a[last]=0.5;}
                let mut b=a;
                b[axis]=0.5;
                [a,b]
            } else {
                let (a,b)=[(0,1),(0,2),(1,2)][index/32];
                let point=|step:usize| {
                    let angle=std::f32::consts::TAU*step as f32/32.;
                    let mut point=Vec3::ZERO;
                    point[a]=0.5*angle.cos(); point[b]=0.5*angle.sin(); point
                };
                [point(index%32),point(index%32+1)]
            };
            points.map(|point|self.world.transform_point3(point))
        })
    }
}

/// Build once when the diagnostics collector's source/selection geometry changes.
/// The caller supplies its cached world matrix and visibility decision.
pub(in crate::shell) fn from_entity(entity:&str,record:&ara_doc::Record,actual_world:Mat4)->Option<VolumeBounds> {
    let component=record.get("PostProcessVolume")?;
    let shape:VolumeShape=match component.get("shape") {Some(value)=>serde::Deserialize::deserialize(value).ok()?,None=>VolumeShape::default()};
    let blend_distance=match component.get("blendDistance") {Some(value)=>value.as_f64()? as f32,None=>0.};
    if shape==VolumeShape::Global || !blend_distance.is_finite() {return None;}
    let (scale,rotation,translation)=model::pose(actual_world)?;
    Some(VolumeBounds{entity:entity.into(),shape,world:Mat4::from_scale_rotation_translation(scale,rotation,translation),blend_distance})
}

#[cfg(test)]
pub(super) fn selected(entities:&Resolved,selection:&[String],hidden:&BTreeSet<String>)->Vec<VolumeBounds> {
    selection.iter().filter(|id|!hidden.contains(*id)).filter_map(|entity| {
        from_entity(entity,&entities.get(entity)?.record,crate::world::world_transform(entities,entity)?)
    }).collect()
}
