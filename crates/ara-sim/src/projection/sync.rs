//! Shared preflight and publication for full and local authoring resolution.
use super::*;

impl EditWorld {
    pub(super) fn sync_full(&mut self, rows: Resolved) -> Result<()> {
        let changed: BTreeSet<_> = rows
            .keys()
            .chain(self.resolved.keys())
            .filter(|id| rows.get(*id) != self.resolved.get(*id))
            .cloned()
            .collect();
        self.sync_changed(&rows, &changed)?;
        self.resolved = rows;
        Ok(())
    }

    pub(super) fn sync_local(&mut self, rows: Resolved) -> Result<()> {
        let changed: BTreeSet<_> = rows
            .iter()
            .filter(|(id, row)| self.resolved.get(*id) != Some(*row))
            .map(|(id, _)| id.clone())
            .collect();
        self.sync_changed(&rows, &changed)?;
        // Keep the complete authored baseline; runtime mutations are never this diff's input.
        self.resolved.extend(rows);
        Ok(())
    }

    fn sync_changed(&mut self, rows: &Resolved, changed: &BTreeSet<String>) -> Result<()> {
        self.mapping
            .retain(|_, entity| self.world.get_entity(*entity).is_ok());
        // Preflight every changed row before component writes or external publication.
        for id in changed {
            if let Some(row) = rows.get(id) {
                for (name, value) in &row.record {
                    if ara_doc::is_component_key(name) {
                        self.registry.validate(name, value)?;
                        let complete = crate::dynamic::materialize(
                            &self.registry.schema,
                            &ara_schema::ValueType::Named(name.clone()),
                            value,
                        );
                        self.registry.validate(name, &complete)?;
                        if let Some(codec) = self.registry.typed.get(name) {
                            (codec.check)(complete)?;
                        }
                    }
                }
            }
        }
        for id in changed {
            if rows.contains_key(id) && !self.mapping.contains_key(id) {
                let entity = self
                    .world
                    .spawn((Identity(id.clone()), SceneMetadata::default()))
                    .id();
                self.mapping.insert(id.clone(), entity);
            }
        }
        // Detach before deletion: Bevy's Children relationship otherwise cascades.
        for id in changed {
            if let Some(&entity) = self.mapping.get(id) {
                self.world.entity_mut(entity).remove::<ChildOf>();
            }
        }
        for id in changed {
            let Some(row) = rows.get(id) else { continue };
            let entity = self.mapping[id];
            let previous = self.resolved.get(id);
            for (name, &component_id) in &self.ids {
                let old = previous.and_then(|row| row.record.get(name));
                let new = row.record.get(name);
                if old == new {
                    continue;
                }
                if let Some(value) = new {
                    self.registry
                        .write(&mut self.world, &self.ids, entity, name, value)?;
                } else {
                    self.world.entity_mut(entity).remove_by_id(component_id);
                }
            }
            for name in &self.registry.external {
                let old = previous.and_then(|row| row.record.get(name));
                let new = row.record.get(name);
                if old != new {
                    let value = new.map(|v| {
                        crate::dynamic::materialize(
                            &self.registry.schema,
                            &ara_schema::ValueType::Named(name.clone()),
                            v,
                        )
                    });
                    self.world
                        .resource_mut::<ExternalWrites>()
                        .0
                        .push(ExternalWrite {
                            entity,
                            component: name.clone(),
                            value,
                        });
                }
            }
            self.world.entity_mut(entity).insert(metadata(row));
            if let Some(parent) = &row.parent {
                let parent = *self
                    .mapping
                    .get(parent)
                    .ok_or_else(|| SimError(format!("missing parent {parent}")))?;
                self.world.entity_mut(entity).insert(ChildOf(parent));
            }
        }
        for id in changed {
            if !rows.contains_key(id)
                && let Some(entity) = self.mapping.remove(id)
            {
                self.world.despawn(entity);
            }
        }
        Ok(())
    }
}
