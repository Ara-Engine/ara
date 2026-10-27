use super::*;

impl ContentDrags {
    pub(super) fn observe(
        &self,
        act: Act,
        editor: &Editor,
        shell: &Shell,
    ) -> ara_ui::Result<(bool, Value)> {
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let changed = editor.host.store().revision() != self.before_revision;
        let preview = self
            .trace
            .iter()
            .find(|trace| trace["stage"] == "preview")
            .map(|trace| &trace["drop"]);
        let (ok, detail) = match act {
            Act::MultiReorder => {
                let mut roots: Vec<_> = resolved
                    .iter()
                    .filter(|(id, entity)| entity.parent.is_none() && IDS.contains(&id.as_str()))
                    .collect();
                roots.sort_by(|(a, ea), (b, eb)| ea.order.cmp(&eb.order).then(a.cmp(b)));
                let roots: Vec<_> = roots.into_iter().map(|(id, _)| id.as_str()).collect();
                (
                    roots == [IDS[0], IDS[3], IDS[1], IDS[2], IDS[4]]
                        && editor.host.store().revision() == self.before_revision + 1,
                    json!({"roots":roots,"expected":[IDS[0],IDS[3],IDS[1],IDS[2],IDS[4]]}),
                )
            }
            Act::Reparent => {
                let entity = resolved
                    .get(IDS[1])
                    .ok_or("The reparent fixture entity is missing")?;
                (
                    entity.parent.as_deref() == Some(IDS[0]) && changed,
                    json!({"parent":entity.parent}),
                )
            }
            Act::Cycle => (
                !changed && preview.is_some_and(|preview| preview["feedback"]["valid"] == false),
                json!({"refused":!changed,"preview":preview}),
            ),
            Act::Cancel => (
                !changed && !shell.content_pending(),
                json!({"unchanged":!changed,"captureReleased":!shell.content_pending()}),
            ),
            Act::AssetReference => {
                let value = &resolved
                    .get(IDS[1])
                    .ok_or("The asset-reference fixture entity is missing")?
                    .record["MeshRenderer"]["model"];
                (
                    changed
                        && value
                            .as_str()
                            .is_some_and(|reference| reference.starts_with("@assets/rock.glb#")),
                    json!({"reference":value}),
                )
            }
            Act::InvalidAsset => (
                !changed && preview.is_some_and(|preview| preview["feedback"]["valid"] == false),
                json!({"refused":!changed,"preview":preview}),
            ),
            Act::AssetSurface => {
                let placed = resolved
                    .iter()
                    .find(|(id, _)| !self.before_entities.contains(id));
                let position = placed
                    .and_then(|(_, entity)| entity.record.get("Transform"))
                    .and_then(|transform| transform.get("position"));
                let expected = preview.map(|preview| &preview["feedback"]["target"]["position"]);
                let hit = preview.is_some_and(|preview| preview["surfaceHit"].is_array());
                let finite = position.and_then(Value::as_array).is_some_and(|values| {
                    values.len() == 3
                        && values
                            .iter()
                            .all(|value| value.as_f64().is_some_and(f64::is_finite))
                });
                (
                    changed && finite && hit && position == expected,
                    json!({"entity":placed.map(|(id,_)|id),"position":position,"expectedSurface":expected,"nativeSurfaceHit":hit}),
                )
            }
            Act::EntityReference | Act::FloatingReference => {
                let value = &resolved
                    .get(CAMERA)
                    .ok_or("The entity-reference fixture camera is missing")?
                    .record["FollowCamera"]["target"];
                let floating =
                    !matches!(act, Act::FloatingReference) || !editor.state.floating.is_empty();
                (
                    changed && value == IDS[2] && floating && editor.state.selection == [CAMERA],
                    json!({"reference":value,"selection":editor.state.selection,"floating":editor.state.floating.len()}),
                )
            }
            Act::MaterialSurface => {
                let target =
                    preview.and_then(|preview| preview["feedback"]["target"]["entity"].as_str());
                let slot =
                    preview.and_then(|preview| preview["feedback"]["target"]["primitive"].as_u64());
                let material = target
                    .and_then(|id| resolved.get(id))
                    .and_then(|entity| entity.record.get("MeshRenderer"))
                    .and_then(|renderer| renderer.get("materialOverrides"))
                    .and_then(Value::as_array)
                    .and_then(|entries| {
                        entries.iter().find(|entry| {
                            entry["primitive"].as_u64().unwrap_or(0) == slot.unwrap_or(u64::MAX)
                        })
                    })
                    .and_then(|entry| entry["material"].as_str());
                (
                    changed
                        && target.is_some()
                        && material.is_some_and(|reference| {
                            reference.starts_with("@assets/coating.graph#")
                        }),
                    json!({"target":target,"slot":slot,"material":material}),
                )
            }
            Act::Component => {
                let rect = |id| shell.shown_node(id).map(|i| shell.ui.nodes[i].rect.y);
                let mesh = rect("component-MeshRenderer");
                let transform = rect("component-Transform");
                (
                    !changed
                        && mesh
                            .zip(transform)
                            .is_some_and(|(mesh, transform)| mesh < transform),
                    json!({"meshY":mesh,"transformY":transform,"authoringUnchanged":!changed}),
                )
            }
            Act::OsImport => {
                let id = editor
                    .host
                    .store()
                    .workspace()
                    .index()
                    .id_of("assets/os-source.png");
                (
                    id.is_some() && editor.host.root().join("assets/os-source.png").is_file(),
                    json!({"asset":id,"sourceCopied":editor.host.root().join("assets/os-source.png").is_file()}),
                )
            }
        };
        Ok((ok, detail))
    }
}
