//! Two-phase image ownership across native content generations.
use super::*;
use ara_render::instanced::Textures;
use std::any::Any;

impl NativeResidency {
    pub(super) fn prepare_reusing(
        &mut self,
        gpu: &mut VulkanDevice,
        previous: &dyn Textures,
    ) -> Result<NativeStats> {
        ensure!(
            self.table.is_none() && self.entries.iter().all(|entry| entry.resident.is_none()),
            "native reuse requires an unpublished candidate"
        );
        if let Some(previous) = (previous as &dyn Any).downcast_ref::<Self>() {
            for (key, id) in &self.keys {
                let Some(old_id) = previous.keys.get(key) else {
                    continue;
                };
                let Some(resident) = previous.entries[old_id.0 as usize].resident else {
                    continue;
                };
                ensure!(!resident.borrowed, "native reuse source is not published");
                // Validate device/handle identity before exposing the borrowed row.
                gpu.native_image(resident.image)?;
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
        self.initialize(gpu)
    }

    pub(super) fn commit_reuse(&mut self, previous: &mut dyn Textures) -> Result<()> {
        let previous = (previous as &mut dyn Any).downcast_mut::<Self>();
        // Validate the entire transfer before relinquishing any previous image.
        for (key, id) in &self.keys {
            let Some(resident) = self.entries[id.0 as usize].resident.filter(|r| r.borrowed) else {
                continue;
            };
            ensure!(
                previous.as_ref().is_some_and(|previous| {
                    previous.keys.get(key).is_some_and(|old_id| {
                        previous.entries[old_id.0 as usize]
                            .resident
                            .is_some_and(|old| old.image == resident.image && !old.borrowed)
                    })
                }),
                "native reused image owner changed before publication"
            );
        }
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
}
