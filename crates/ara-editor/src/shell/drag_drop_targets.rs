//! Resolve content drops against the current retained UI and schema.
use super::*;
impl Shell {
    pub(super) fn content_target(
        &mut self,
        editor: &Editor,
        payload: &Payload,
    ) -> ara_ui::Result<Feedback<Target>> {
        let [x, y] = self.pointer;
        for &i in self.ui.paint_order.iter().rev() {
            let node = &self.ui.nodes[i];
            if !self.ui.shown(i) || !node.rect.contains(x, y) || !node.clip.contains(x, y) {
                continue;
            }
            let Some(binding) = self.bindings.get(&stable_id(&node.element.id)) else {
                continue;
            };
            let indicator = Indicator::Inside(node.rect.intersection(node.clip));
            if let Payload::Entity(entity) = payload
                && let Some(path) = self.entity_drop_field(binding)
            {
                let field = path.split_once('#').map_or(path, |(path, _)| path);
                if self.fields.get(field).is_some_and(|field| field.read_only) {
                    return Ok(reject("This property is read-only", Some(indicator)));
                }
                let path = path.to_owned();
                let resolved = self.resolved(editor)?;
                return Ok(if resolved.contains_key(entity) {
                    accept(
                        Target::EntityField(path),
                        indicator,
                        "Assign entity reference",
                    )
                } else {
                    reject("The dragged entity was deleted", Some(indicator))
                });
            }
            match (payload, binding) {
                (Payload::Asset(asset), Binding::AssetPicker(path, _)) => {
                    let field = path.split_once('#').map_or(path.as_str(), |(path, _)| path);
                    if self.fields.get(field).is_some_and(|field| field.read_only) {
                        return Ok(reject("This property is read-only", Some(indicator)));
                    }
                    return Ok(match self.asset_for_field(editor, asset, path) {
                        Ok(_) => accept(
                            Target::AssetField(path.clone()),
                            indicator,
                            "Assign asset reference",
                        ),
                        Err(reason) => reject(reason, Some(indicator)),
                    });
                }
                (Payload::Entity(_), Binding::AssetPicker(..))
                | (Payload::Asset(_), Binding::Field(..)) => {
                    return Ok(reject(
                        "This field cannot accept that reference",
                        Some(indicator),
                    ));
                }
                (Payload::Asset(asset), Binding::AnimationField(doc, item, key))
                    if key == "clip" =>
                {
                    return Ok(if matches!(asset.kind.as_str(), "glb" | "gltf" | "fbx") {
                        accept(
                            Target::Animation {
                                doc: doc.clone(),
                                item: item.clone(),
                                key: key.clone(),
                            },
                            indicator,
                            "Assign animation clip",
                        )
                    } else {
                        reject("Choose a model containing animation clips", Some(indicator))
                    });
                }
                _ => {}
            }
        }
        if let Payload::Collection(source) = payload {
            for &i in self.ui.paint_order.iter().rev() {
                let node = &self.ui.nodes[i];
                if !node.element.id.starts_with("nest-")
                    || !self.ui.shown(i)
                    || !node.rect.contains(x, y)
                    || !node.clip.contains(x, y)
                {
                    continue;
                }
                let Some(Binding::Collection(row)) =
                    self.bindings.get(&stable_id(&node.element.id))
                else {
                    continue;
                };
                if !matches!(row.intent, collection_adapter::Intent::Select(_)) {
                    continue;
                }
                let placement =
                    kit::row_placement(node.rect, self.pointer, false).unwrap_or(Placement::Before);
                let indicator = kit::row_indicator(node.rect, placement);
                if source.field != row.field || source.pointer != row.pointer {
                    return Ok(reject(
                        "Reorder items within the same list",
                        Some(indicator),
                    ));
                }
                let collection = self
                    .compound_values
                    .get(&row.field)
                    .map(|field| &field.value)
                    .or_else(|| self.fields.get(&row.field).map(|field| &field.value));
                if !collection
                    .and_then(|value| value.pointer(&row.pointer))
                    .is_some_and(Value::is_array)
                {
                    return Ok(reject(
                        "Map entries are ordered by key and cannot be reordered",
                        Some(indicator),
                    ));
                }
                if source == row {
                    return Ok(reject("This item is already here", Some(indicator)));
                }
                return Ok(accept(
                    Target::Collection {
                        row: row.clone(),
                        after: placement == Placement::After,
                    },
                    indicator,
                    "Reorder list item",
                ));
            }
            return Ok(reject("Drop between items in the same list", None));
        }
        if let Payload::Component(source) = payload {
            for &i in self.ui.paint_order.iter().rev() {
                let node = &self.ui.nodes[i];
                let Some(name) = node.element.id.strip_prefix("component-") else {
                    continue;
                };
                if !node.element.attrs.get("class").is_some_and(|class| {
                    class
                        .split_whitespace()
                        .any(|class| class == "component-header")
                }) || !self.ui.shown(i)
                    || !node.rect.contains(x, y)
                    || !node.clip.contains(x, y)
                {
                    continue;
                }
                let placement =
                    kit::row_placement(node.rect, self.pointer, false).unwrap_or(Placement::Before);
                let indicator = kit::row_indicator(node.rect, placement);
                return Ok(if name == source {
                    reject("This component is already here", Some(indicator))
                } else {
                    accept(
                        Target::Component {
                            name: name.into(),
                            after: placement == Placement::After,
                        },
                        indicator,
                        "Reorder component",
                    )
                });
            }
            return Ok(reject(
                "Drop between component headers in the Inspector",
                None,
            ));
        }
        if let Some((entity, placement, indicator)) = self.hierarchy_drop_region() {
            let resolved = self.resolved(editor)?;
            return Ok(match payload {
                Payload::Entity(source) => match hierarchy::commands(
                    &resolved,
                    &editor.state.selection,
                    source,
                    entity.as_deref(),
                    placement,
                    &editor.state.locked,
                ) {
                    Ok(_) => accept(
                        Target::Hierarchy { entity, placement },
                        indicator,
                        match placement {
                            Placement::Inside => "Move inside",
                            Placement::Before => "Move before",
                            Placement::After => "Move after",
                        },
                    ),
                    Err(reason) => reject(reason, Some(indicator)),
                },
                Payload::Asset(asset)
                    if matches!(asset.kind.as_str(), "prefab" | "glb" | "gltf" | "fbx") =>
                {
                    if asset
                        .current_reference(editor.host.store().workspace())
                        .is_none()
                    {
                        reject("This asset is still importing", Some(indicator))
                    } else {
                        accept(
                            Target::Hierarchy { entity, placement },
                            indicator,
                            "Place in hierarchy",
                        )
                    }
                }
                _ => reject(
                    "Only entities, prefabs and models can be placed in the hierarchy",
                    Some(indicator),
                ),
            });
        }
        if let (Payload::Asset(asset), Some(rect)) = (payload, self.viewport())
            && rect.contains(x, y)
        {
            let indicator = Indicator::Inside(rect);
            if material::is_material(asset) {
                return Ok(reject(NO_MATERIAL_TARGET, Some(indicator)));
            }
            if !matches!(asset.kind.as_str(), "prefab" | "glb" | "gltf" | "fbx") {
                return Ok(reject(
                    "Only prefabs and models can be placed in the Scene",
                    Some(indicator),
                ));
            }
            if asset
                .current_reference(editor.host.store().workspace())
                .is_none()
            {
                return Ok(reject("This asset is still importing", Some(indicator)));
            }
            return Ok(
                match overlay::ground_point(&editor.state.camera, rect, self.pointer)
                    .filter(|point| point.is_finite())
                {
                    Some(position) => accept(
                        Target::Viewport(position.to_array()),
                        indicator,
                        "Place in Scene",
                    ),
                    None => reject(NO_GROUND, Some(indicator)),
                },
            );
        }
        Ok(reject(
            "Drop onto a hierarchy, reference field or Scene",
            None,
        ))
    }
    fn entity_drop_field<'a>(&'a self, binding: &'a Binding) -> Option<&'a str> {
        match binding {
            Binding::Field(path, None) | Binding::PickField(path)
                if self.fields.get(path).is_some_and(|field| {
                    matches!(field.widget, crate::inspector::Widget::Entity)
                }) =>
            {
                Some(path)
            }
            Binding::Nested(path, ..) | Binding::PickField(path)
                if path.contains('#')
                    && self.bindings.values().any(
                        |binding| matches!(binding,Binding::PickField(target) if target==path),
                    ) =>
            {
                Some(path)
            }
            _ => None,
        }
    }
    fn hierarchy_drop_region(&self) -> Option<(Option<String>, Placement, Indicator)> {
        let [x, y] = self.pointer;
        for &i in self.ui.paint_order.iter().rev() {
            let node = &self.ui.nodes[i];
            if !self.ui.shown(i) || !node.rect.contains(x, y) || !node.clip.contains(x, y) {
                continue;
            }
            if let Some(Binding::Select(entity)) = self.bindings.get(&stable_id(&node.element.id)) {
                let placement = kit::row_placement(node.rect, self.pointer, true)?;
                return Some((
                    Some(entity.clone()),
                    placement,
                    kit::row_indicator(node.rect, placement),
                ));
            }
        }
        for id in ["hierarchy-scene", "hierarchy-rows", "hierarchy-empty"] {
            if let Some(i) = self.shown_node(id) {
                let node = &self.ui.nodes[i];
                if node.rect.contains(x, y) && node.clip.contains(x, y) {
                    return Some((
                        None,
                        Placement::Inside,
                        Indicator::Inside(node.rect.intersection(node.clip)),
                    ));
                }
            }
        }
        None
    }
}
