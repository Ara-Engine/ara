use super::*;

impl SnapCheck {
    pub(super) fn capture(
        &mut self,
        act: Act,
        stage: &str,
        editor: &Editor,
        shell: &mut Shell,
        gpu: &mut NativeUi,
        floats: &windows::Floats,
    ) -> ara_ui::Result<()> {
        shell.leave();
        let Some(host) = windows::resolve_scene_host(editor, shell, floats, gpu.dpi()) else {
            return Err("Snap capture has no hosting Scene window".into());
        };
        let target = host.target;
        let surface = host.surface.map(str::to_owned);
        let rect = host.viewport;
        let dpi = host.dpi;
        let native = surface
            .as_deref()
            .and_then(|id| floats.windows.iter().find(|float| float.id == id));
        gpu.select_target(target);
        std::fs::create_dir_all(&self.output)?;
        let stem = format!("snap-{:02}-{}-{stage}", self.index + 1, act.label());
        gpu.pixels()?
            .save(self.output.join(format!("{stem}.png")))?;
        gpu.select_target(0);
        if let Some(id) = &surface {
            shell.enter(id);
        }
        std::fs::write(
            self.output.join(format!("{stem}.layout.json")),
            ara_json::to_canonical_string(&shell.layout_report()),
        )?;
        let records = fixture::records(editor)?;
        let preview=editor.gesture.as_ref().map(|gesture|json!({"base":gesture.base,"roots":gesture.current.iter().map(|(id,matrix)|(id,matrix.to_cols_array())).collect::<std::collections::BTreeMap<_,_>>()}));
        self.trace.push(json!({"stage":stage,"surface":surface,"window":native.map(|float|float.sdl),"windowRect":native.map(|float|float.rect),"viewport":rect,"dpi":dpi,"camera":editor.state.camera,"doc":editor.state.doc,"hidden":editor.state.hidden,"locked":editor.state.locked,"readiness":self.readiness,"readinessSample":"before-native-frame","records":records,"revision":editor.host.store().revision(),"selection":editor.state.selection,"gesture":preview,"readout":shell.ui.find("gizmo-readout-text").and_then(|index|shell.ui.nodes[index].text.as_ref().map(|text|text.value.as_str())),"from":self.from,"to":self.to}));
        shell.leave();
        Ok(())
    }

    pub(super) fn result(&self, act: Act, editor: &Editor) -> ara_ui::Result<(bool, Value)> {
        let after = fixture::records(editor)?;
        let preview = self.trace.iter().find(|trace| trace["stage"] == "preview");
        let undo = self.trace.iter().find(|trace| trace["stage"] == "undo");
        let redo = self.trace.iter().find(|trace| trace["stage"] == "redo");
        let committed = self.committed.as_ref().unwrap_or(&after);
        let vec3 = |value: &Value| {
            serde_json::from_value::<[f32; 3]>(value.clone())
                .map(Vec3::from)
                .map_err(|error| error.to_string())
        };
        let before_position = vec3(&self.before[SOURCE]["Transform"]["position"])?;
        let position = vec3(&committed[SOURCE]["Transform"]["position"])?;
        let actual = position - before_position;
        let contact = if act.vertex() {
            self.source
                .zip(self.target)
                .is_some_and(|(source, target)| (source + actual - target).length() < 0.003)
        } else {
            self.target
                .zip(self.normal)
                .is_some_and(|(target, normal)| {
                    self.vertices
                        .iter()
                        .map(|vertex| (*vertex + actual - target).dot(normal))
                        .min_by(f32::total_cmp)
                        .is_some_and(|distance| (distance - 0.125).abs() < 0.003)
                })
        };
        let no_authoring_preview = act.drop()
            || preview.is_some_and(|trace| {
                trace["records"] == self.before && trace["revision"] == self.revision
            });
        let unchanged = after == self.before && editor.host.store().revision() == self.revision;
        let source_unchanged =
            committed[SOURCE] == self.before[SOURCE] && committed[CHILD] == self.before[CHILD];
        let one_undo = undo.is_some_and(|trace| {
            trace["records"] == self.before && trace["revision"] == self.revision + 2
        });
        let one_redo = redo.is_some_and(|trace| {
            trace["records"] == *committed && trace["revision"] == self.revision + 3
        });
        let one_commit = self
            .trace
            .iter()
            .find(|trace| trace["stage"] == "after")
            .is_some_and(|trace| trace["revision"] == self.revision + 1);
        let initial = crate::world::local_transform(
            self.before[SOURCE]
                .as_object()
                .ok_or("Snap source record missing")?,
        );
        let transformed = crate::world::local_transform(
            committed[SOURCE]
                .as_object()
                .ok_or("Snap source record missing after contact")?,
        );
        let same_orientation = [
            (initial.x_axis, transformed.x_axis),
            (initial.y_axis, transformed.y_axis),
            (initial.z_axis, transformed.z_axis),
        ]
        .into_iter()
        .all(|(a, b)| (a - b).abs().max_element() < 1e-5);
        let staged = preview.is_some_and(|trace| trace["gesture"].is_object());
        let ok = if act.commits() {
            contact
                && same_orientation
                && no_authoring_preview
                && one_commit
                && one_undo
                && one_redo
                && committed[CHILD] == self.before[CHILD]
        } else if act == Act::DeletedTarget {
            source_unchanged
                && committed.get(TARGET).is_none()
                && editor.host.store().revision() == self.revision + 1
                && staged
        } else {
            unchanged && no_authoring_preview && if act == Act::Escape { staged } else { true }
        };
        Ok((
            ok && editor.gesture.is_none(),
            json!({"actualWorldTranslation":actual.to_array(),"expectedPointerTranslation":self.delta.to_array(),"actualMeshContact":contact,"orientationAndScaleRetained":same_orientation,"noAuthoringDuringPreview":no_authoring_preview,"oneCommit":one_commit,"oneUndoRestoresAll":one_undo,"oneRedoRestoresAll":one_redo,"unchanged":unchanged,"sourceUnchanged":source_unchanged,"sourceVertex":self.source.map(|point|point.to_array()),"targetPoint":self.target.map(|point|point.to_array()),"targetNormal":self.normal.map(|point|point.to_array()),"gridIncrement":5.,"surfaceOffset":if act.vertex(){0.}else{0.125},"gestureReleased":editor.gesture.is_none()}),
        ))
    }
}
