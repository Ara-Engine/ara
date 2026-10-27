//! Two-phase image ownership across native content generations.
use super::*;
use ara_render::instanced::Textures;
use std::any::Any;

impl NativeResidency {
    pub(super) fn prepare_reusing(
        &mut self,
        gpu: &mut GpuDevice,
        previous: &dyn Textures,
    ) -> Result<NativeStats> {
        if self.initial.is_none() {
            self.borrow_reusing(gpu, Some(previous))?;
        } else {
            ensure!(
                self.reuse_is_valid(Some(previous)),
                "native reused image owner changed before initialization"
            );
        }
        self.initialize(gpu)
    }

    pub(super) fn borrow_reusing(
        &mut self,
        gpu: &GpuDevice,
        previous: Option<&dyn Textures>,
    ) -> Result<()> {
        ensure!(
            self.table.is_none() && self.entries.iter().all(|entry| entry.resident.is_none()),
            "native reuse requires an unpublished candidate"
        );
        if let Some(previous) = previous.and_then(|old| (old as &dyn Any).downcast_ref::<Self>()) {
            for (key, id) in &self.keys {
                let Some(old_id) = previous.keys.get(key) else {
                    continue;
                };
                let Some(resident) = previous.entries[old_id.0 as usize].resident else {
                    continue;
                };
                ensure!(!resident.borrowed, "native reuse source is not published");
                // Validate device/handle identity before exposing the borrowed row.
                gpu.image_extent(resident.image)?;
                self.entries[id.0 as usize].resident = Some(Resident {
                    borrowed: true,
                    ..resident
                });
            }
            self.stats.resident_bytes = self
                .entries
                .iter()
                .filter_map(|entry| entry.resident)
                .map(|resident| resident.bytes)
                .sum();
        }
        Ok(())
    }

    pub(super) fn commit_reuse(&mut self, previous: &mut dyn Textures) -> Result<()> {
        // Validate the entire transfer before relinquishing any previous image.
        ensure!(
            self.reuse_is_valid(Some(previous)),
            "native reused image owner changed before publication"
        );
        let previous = (previous as &mut dyn Any).downcast_mut::<Self>();
        if let Some(previous) = previous {
            for (key, id) in &self.keys {
                let Some(resident) = self.entries[id.0 as usize].resident.as_mut() else {
                    continue;
                };
                if resident.borrowed {
                    let old_id = previous.keys[key];
                    previous.entries[old_id.0 as usize].resident = None;
                    resident.borrowed = false;
                }
            }
        }
        Ok(())
    }

    pub(super) fn reuse_is_valid(&self, previous: Option<&dyn Textures>) -> bool {
        let previous = previous.and_then(|old| (old as &dyn Any).downcast_ref::<Self>());
        for (key, id) in &self.keys {
            let Some(resident) = self.entries[id.0 as usize].resident.filter(|r| r.borrowed) else {
                continue;
            };
            if !previous.is_some_and(|previous| {
                previous.keys.get(key).is_some_and(|old_id| {
                    previous.entries[old_id.0 as usize]
                        .resident
                        .is_some_and(|old| old.image == resident.image && !old.borrowed)
                })
            }) {
                return false;
            }
        }
        true
    }
}
