//! Per-camera instance motion and draw lists over the renderer's shared content.
use super::*;

pub(in crate::renderer) struct NativeView {
    source: Buffer,
    revision: u64,
    geometry_revisions: Vec<u64>,
    instances: Buffer,
    instance_index: u32,
    visible: Buffer,
    visible_index: u32,
    rows: Vec<InstanceRow>,
    presented: Vec<Mat4>,
    previous_vp: Option<Mat4>,
    culling: Culling,
    /// Passes `visible` holds rows for (`Native::capacity`).
    capacity: usize,
    changes: updates::Updates,
}

impl Native {
    /// Allocate camera-local rows and visible ranges, sharing every static GPU
    /// allocation, material, texture and pipeline with the primary camera.
    pub(in crate::renderer) fn new_view(&self, gpu: &mut VulkanDevice) -> Result<NativeView> {
        let mut allocated = Vec::new();
        let result = (|| {
            let instances = host_buffer(
                gpu,
                128 + self.rows.len() * std::mem::size_of::<InstanceRow>(),
                &mut allocated,
            )?;
            let visible = host_buffer(
                gpu,
                (self.rows.len() * culling::BASE_PASSES * 8).max(16),
                &mut allocated,
            )?;
            Ok(NativeView {
                source: self.materials,
                revision: self.view_revision,
                geometry_revisions: self.geometry_revisions.to_vec(),
                instances,
                instance_index: gpu.storage_buffer(instances)?.index,
                visible,
                visible_index: gpu.storage_buffer(visible)?.index,
                rows: self.rows.to_vec(),
                presented: self.rows.iter().map(|row| row.world).collect(),
                previous_vp: None,
                culling: self.culling.fork(&self.groups),
                capacity: culling::BASE_PASSES,
                changes: updates::Updates::new(self.rows.len()),
            })
        })();
        if result.is_err() {
            for resource in allocated {
                gpu.retire(resource)?;
            }
            gpu.collect()?;
        }
        result
    }

    /// Moving owned camera state never moves or duplicates shared content.
    pub(in crate::renderer) fn swap_view(&mut self, view: &mut NativeView) {
        std::mem::swap(&mut self.instances, &mut view.instances);
        std::mem::swap(&mut self.instance_index, &mut view.instance_index);
        std::mem::swap(&mut self.visible, &mut view.visible);
        std::mem::swap(&mut self.visible_index, &mut view.visible_index);
        std::mem::swap(&mut self.rows, &mut view.rows);
        std::mem::swap(&mut self.presented, &mut view.presented);
        std::mem::swap(&mut self.previous_vp, &mut view.previous_vp);
        std::mem::swap(&mut self.culling, &mut view.culling);
        std::mem::swap(&mut self.capacity, &mut view.capacity);
        std::mem::swap(&mut self.changes, &mut view.changes);
    }
}

impl NativeView {
    pub(in crate::renderer) fn matches(&self, source: &Native) -> bool {
        self.source == source.materials && self.rows.len() == source.rows.len()
    }

    /// Returns whether changed deformation requires this camera's history reset.
    /// Static frames do not visit instances; a changed revision copies only small
    /// cached instance rows and bounds, never source geometry or asset payloads.
    pub(in crate::renderer) fn refresh(&mut self, source: &Native) -> bool {
        if self.revision == source.view_revision {
            return false;
        }
        let deformed = self.geometry_revisions != source.geometry_revisions;
        self.rows.copy_from_slice(&source.rows);
        self.geometry_revisions
            .copy_from_slice(&source.geometry_revisions);
        self.culling.refresh(&source.culling, &source.groups);
        self.changes.invalidate(self.rows.len());
        self.revision = source.view_revision;
        deformed
    }

    pub(in crate::renderer) fn destroy(self, gpu: &mut VulkanDevice) -> Result<()> {
        gpu.retire(Resource::Buffer(self.instances))?;
        gpu.retire(Resource::Buffer(self.visible))?;
        Ok(())
    }
}
