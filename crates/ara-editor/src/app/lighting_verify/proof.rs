//! Semantic authoring/history checks and independent axis-aligned blend expectations.
use super::*;
use ara_components::post::{PostProcessVolume, PostValues};
pub(super) fn snapshot(editor: &Editor, exclude: Option<&str>) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Lighting Scene is unavailable")?;
    let mut records = json!(doc.records());
    if let Some(id) = exclude {
        records
            .as_object_mut()
            .ok_or("Lighting record map is invalid")?
            .remove(id);
    }
    let canonical = ara_json::to_canonical_string(&records);
    Ok(
        json!({"doc":editor.state.doc,"revision":editor.host.store().revision(),"count":records.as_object().map_or(0,|records|records.len()),"ids":records.as_object().map(|records|records.keys().collect::<Vec<_>>()),"bytes":canonical.len(),"blake3":blake3::hash(canonical.as_bytes()).to_hex().to_string()}),
    )
}
pub(super) fn record(editor: &Editor, id: &str) -> ara_ui::Result<Value> {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|d| format!("{d:?}"))?;
    Ok(json!(
        resolved
            .get(id)
            .ok_or("Lighting target disappeared")?
            .record
    ))
}
fn same(a: &Value, b: &Value) -> bool {
    a["doc"] == b["doc"]
        && a["blake3"].is_string()
        && a["blake3"] == b["blake3"]
        && a["bytes"] == b["bytes"]
}
fn near(value: &Value, expected: f64) -> bool {
    value
        .as_f64()
        .is_some_and(|value| value.is_finite() && (value - expected).abs() < 0.0001)
}
fn text(shell: &Shell, id: &str) -> String {
    shell
        .ui
        .find(id)
        .and_then(|i| shell.ui.nodes[i].text.as_ref())
        .map_or(String::new(), |text| text.value.clone())
}
/// Distance to an axis-aligned box is the Euclidean length of its outside distances.
/// This is independent of the runtime inverse-transform/clamped-closest-point code.
pub(super) fn box_weight(point: [f32; 3], half: [f32; 3], blend: f32, weight: f32) -> f32 {
    let distance = point
        .into_iter()
        .zip(half)
        .map(|(p, h)| (p.abs() - h).max(0.).powi(2))
        .sum::<f32>()
        .sqrt();
    if distance == 0. {
        weight
    } else {
        weight * (1. - distance / blend).clamp(0., 1.)
    }
}
impl LightingCheck {
    pub(super) fn observe(
        &mut self,
        stage: &str,
        editor: &Editor,
        shell: &Shell,
        scene: &super::super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        let act = ACTS[self.index];
        match stage {
            "before" => {
                self.phase = 2;
                return Ok(());
            }
            "preview" => {
                self.checks["scrubPreviewHasNoAuthoringOps"] = json!(
                    same(&self.before, &snapshot(editor, None)?)
                        && self.before["revision"] == editor.host.store().revision()
                );
                self.checks["actualNumericScrubOwnsCapture"] = json!(shell.input.scrubbing());
                self.phase = 5;
                return Ok(());
            }
            "outside" | "blend" | "inside" => return self.blend_sample(stage, editor, scene),
            "winner" => {
                self.fog_winner(editor, shell, scene)?;
                self.phase = 11;
                return Ok(());
            }
            "undo" => {
                let now = snapshot(editor, None)?;
                self.checks["oneNativeUndoRestoresCompleteScene"] = json!(
                    same(&self.before, &now)
                        && self.before["revision"]
                            .as_u64()
                            .is_some_and(|r| now["revision"] == r + 2)
                );
                self.observations["undo"] = now;
                self.phase = 13;
                return Ok(());
            }
            "redo" => {
                let now = snapshot(editor, None)?;
                self.checks["oneNativeRedoRestoresCompleteAction"] = json!(
                    same(&self.after, &now)
                        && self.before["revision"]
                            .as_u64()
                            .is_some_and(|r| now["revision"] == r + 3)
                );
                self.observations["redo"] = now;
                return self.finish_case();
            }
            _ => {}
        }
        self.after = snapshot(editor, None)?;
        if act == Act::Open {
            self.checks["paletteOpenedActualLightingPage"] = json!(
                shell.shown_node("lighting-overview").is_some()
                    && shell.shown_node("lighting-create-global").is_some()
            );
            self.checks["openingHasNoAuthoringOps"] = json!(
                same(&self.before, &self.after)
                    && self.before["revision"] == self.after["revision"]
            );
            return self.finish_case();
        }
        if act == Act::Cage {
            let id = self.target.as_deref().ok_or("Cage target missing")?;
            let layout = shell.layout_report();
            let diagnostics = &layout["viewportDiagnostics"];
            self.checks["nativeInventoryClickSelectedRealVolume"] =
                json!(editor.state.selection == [id]);
            self.checks["selectedVolumeCageUsesCurrentNativeSource"] = json!(
                scene.diagnostic_source(editor).current
                    && diagnostics["nativeSourceCurrent"] == true
            );
            self.checks["actualProjectedCageSegmentsPresent"] = json!(
                diagnostics["selectedVolumes"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| row["entity"] == id))
                    && diagnostics["counts"]["volumes"]
                        .as_u64()
                        .is_some_and(|count| count >= 1)
                    && diagnostics["projectedSegments"]
                        .as_u64()
                        .is_some_and(|count| count >= 12)
            );
            self.checks["selectionAndCageDoNotAuthor"] = json!(
                same(&self.before, &self.after)
                    && self.before["revision"] == self.after["revision"]
            );
            self.observations["viewportDiagnostics"] = diagnostics.clone();
            return self.finish_case();
        }
        self.checks["oneAuthoringTransaction"] = json!(
            self.before["revision"]
                .as_u64()
                .is_some_and(|r| self.after["revision"] == r + 1)
        );
        if act.creation() {
            self.creation(act, editor)?;
        } else {
            self.edit(act, editor)?;
        }
        if matches!(act, Act::ShapeSphere | Act::ShapeBox) {
            self.checks["shapeUsesSharedNativeSelect"] = json!(
                shell
                    .shown_node("enum-PostProcessVolume-shape")
                    .is_some_and(|index| shell.ui.nodes[index].element.tag == "dropdown")
            );
        }
        if act == Act::FogWinner {
            self.phase = 8;
            return Ok(());
        }
        if act.mutation() {
            self.phase = 11;
            Ok(())
        } else {
            self.finish_case()
        }
    }
    fn creation(&mut self, act: Act, editor: &Editor) -> ara_ui::Result<()> {
        let before = self.before["ids"]
            .as_array()
            .ok_or("Creation baseline identities missing")?;
        let after = self.after["ids"]
            .as_array()
            .ok_or("Creation result identities missing")?;
        let added: Vec<_> = after.iter().filter(|id| !before.contains(id)).collect();
        let entity = added
            .first()
            .and_then(|id| id.as_str())
            .ok_or("Native Lighting button created no entity")?
            .to_owned();
        self.checks["oneNewEntityOnly"] =
            json!(added.len() == 1 && after.len() == before.len() + 1);
        self.checks["otherSceneRecordsPreserved"] =
            json!(same(&self.before, &snapshot(editor, Some(&entity))?));
        self.checks["nativeCreationSelectsItsActualIdentity"] =
            json!(editor.state.selection == [entity.clone()]);
        let record = record(editor, &entity)?;
        let key = match act {
            Act::Global => "global",
            Act::Box => "box",
            Act::Sphere => "sphere",
            Act::Fog => "fog",
            _ => "fog2",
        };
        if matches!(act, Act::Fog | Act::FogWinner) {
            let fog: ara_components::render::HeightFog =
                serde_json::from_value(record["HeightFog"].clone())?;
            self.checks["createdComponentMatchesRealFogSchemaDefaults"] =
                json!(fog == ara_components::render::HeightFog::default());
        } else {
            let volume: PostProcessVolume =
                serde_json::from_value(record["PostProcessVolume"].clone())?;
            self.checks["realVolumeShapeAndEmptyOverrides"] = json!(
                volume.overrides.is_empty()
                    && volume.shape
                        == match act {
                            Act::Global => ara_components::post::VolumeShape::Global,
                            Act::Box => ara_components::post::VolumeShape::Box,
                            _ => ara_components::post::VolumeShape::Sphere,
                        }
            );
            if act != Act::Global {
                self.checks["localVolumeHasSizedTransformAndBlend"] = json!(
                    near(&record["PostProcessVolume"]["blendDistance"], 2.)
                        && record["Transform"]["scale"]
                            .as_array()
                            .is_some_and(|values| values.len() == 3
                                && values
                                    .iter()
                                    .all(|value| value.as_f64().is_some_and(|value| value >= 5.)))
                );
            }
        }
        self.ids.insert(key.into(), entity.clone());
        self.target = Some(entity);
        self.observations["createdRecord"] = record;
        Ok(())
    }
    fn edit(&mut self, act: Act, editor: &Editor) -> ara_ui::Result<()> {
        let id = self
            .target
            .as_deref()
            .ok_or("Native Lighting edit target missing")?;
        let after = record(editor, id)?;
        let mut expected = self.observations["beforeTarget"].clone();
        let valid = match act {
            Act::ShapeSphere | Act::ShapeBox => {
                let desired = if act == Act::ShapeSphere {
                    "sphere"
                } else {
                    "box"
                };
                expected["PostProcessVolume"]["shape"] = json!(desired);
                after["PostProcessVolume"]["shape"] == desired
            }
            Act::Priority | Act::Weight | Act::Blend | Act::Scrub => {
                let (field, value) = match act {
                    Act::Priority => ("priority", 23.),
                    Act::Weight => ("weight", 0.4),
                    Act::Blend => ("blendDistance", 3.5),
                    _ => ("weight", 0.),
                };
                let actual = &after["PostProcessVolume"][field];
                expected["PostProcessVolume"][field] = actual.clone();
                if act == Act::Scrub {
                    actual
                        .as_f64()
                        .is_some_and(|value| value > 0.4 && value <= 1.)
                } else {
                    near(actual, value)
                }
            }
            Act::FogDensity => {
                expected["HeightFog"]["density"] = after["HeightFog"]["density"].clone();
                near(&after["HeightFog"]["density"], 0.004)
            }
            Act::Interior => {
                let prior = expected["PostProcessVolume"]["overrides"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                let groups = after["PostProcessVolume"]["overrides"]
                    .as_array()
                    .ok_or("Interior preset did not append override groups")?;
                let n = prior.len();
                let valid = groups.len() == n + 3
                    && groups[..n] == prior[..]
                    && groups[n]["module"] == "exposure"
                    && near(&groups[n]["ev100"], 7.)
                    && groups[n + 1]["module"] == "colorGrading"
                    && near(&groups[n + 1]["whiteBalance"], 4500.)
                    && groups[n + 2]["module"] == "bloom"
                    && near(&groups[n + 2]["bloom"], 0.04);
                expected["PostProcessVolume"]["overrides"] = json!(groups);
                valid
            }
            _ => false,
        };
        self.checks["nativeControlProducedRequestedSchemaValue"] = json!(valid);
        self.checks["untouchedTargetFieldsPreserved"] = json!(after == expected);
        self.checks["allOtherSceneRecordsPreserved"] = json!(same(
            &self.observations["beforeOtherRecords"],
            &snapshot(editor, Some(id))?
        ));
        self.observations["afterTarget"] = after;
        Ok(())
    }
    fn fog_winner(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        scene: &super::super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
            .map_err(|d| format!("{d:?}"))?;
        let (id, row) = resolved
            .iter()
            .find(|(id, row)| {
                !editor.state.hidden.contains(*id) && row.record.contains_key("HeightFog")
            })
            .ok_or("No authored HeightFog winner")?;
        let expected: ara_components::render::HeightFog =
            serde_json::from_value(row.record["HeightFog"].clone())?;
        let source = scene.diagnostic_source(editor);
        let actual = source.frame.and_then(|frame| frame.look.fog);
        let entry = format!(
            "lighting-entry-{:016x}-detail",
            ara_ui::source::stable_id(id)
        );
        self.checks["actualNativeFrameFogUsesFirstEntityByIdentity"] =
            json!(source.current && actual == Some(expected));
        self.checks["lightingPageExplainsEffectiveFogWinner"] = json!(
            text(shell, &entry).contains("effective first entity in ID order")
                && shell.shown_node(&entry).is_some_and(|index| {
                    let node = &shell.ui.nodes[index];
                    node.rect.intersection(node.clip).h >= node.rect.h - 0.5
                })
        );
        self.observations["fogWinner"] = json!({"entity":id,"expected":expected,"actualRetainedFrame":actual,"sourceCurrent":source.current});
        Ok(())
    }
    fn blend_sample(
        &mut self,
        stage: &str,
        editor: &Editor,
        scene: &super::super::viewport::SceneView,
    ) -> ara_ui::Result<()> {
        let source = scene.diagnostic_source(editor);
        let frame = source
            .frame
            .ok_or("Lighting blend proof has no actual retained native frame")?;
        let id = self
            .ids
            .get("box")
            .ok_or("Native box identity unavailable")?;
        let record = record(editor, id)?;
        let volume: PostProcessVolume =
            serde_json::from_value(record["PostProcessVolume"].clone())?;
        let point = editor.state.camera.position().to_array();
        let transform: ara_components::Transform =
            serde_json::from_value(record["Transform"].clone())?;
        if transform.rotation.iter().any(|angle| angle.abs() > 0.0001) {
            return Err("The independent blend oracle requires its authored axis-aligned native box fixture".into());
        }
        let half = transform.scale.map(|scale| scale.abs() * 0.5);
        let relative = std::array::from_fn(|axis| point[axis] - transform.position[axis]);
        let expected = box_weight(relative, half, volume.blend_distance, volume.weight);
        let actual_volume = frame
            .look
            .volumes
            .iter()
            .find(|item| item.volume == volume)
            .ok_or("Created authored box is absent from retained extraction")?;
        let weight = actual_volume.weight_at(point);
        let post = frame
            .look
            .post_at(point)
            .ok_or("Native extracted Scene has no resolved post values")?;
        let default = PostValues::default();
        let correct = source.current
            && (weight - expected).abs() < 0.0001
            && (post.ev100 - (default.ev100 + (7. - default.ev100) * expected)).abs() < 0.001
            && post.exposure
                == if expected > 0. {
                    ara_components::render::ExposureMode::Manual
                } else {
                    default.exposure
                }
            && (post.white_balance
                - (default.white_balance + (4500. - default.white_balance) * expected))
                .abs()
                < 0.01
            && (post.bloom - (default.bloom + (0.04 - default.bloom) * expected)).abs() < 0.0001;
        self.samples.push(json!({"stage":stage,"camera":editor.state.camera,"point":point,"independentWeight":expected,"runtimeWeightAt":weight,"actualNativeFramePost":{"exposure":post.exposure,"ev100":post.ev100,"whiteBalance":post.white_balance,"bloom":post.bloom},"sourceCurrent":source.current,"correct":correct}));
        if stage == "outside" {
            self.phase = 3;
            return Ok(());
        }
        if stage == "blend" {
            self.phase = 7;
            return Ok(());
        }
        let weights: Vec<_> = self
            .samples
            .iter()
            .filter_map(|sample| sample["runtimeWeightAt"].as_f64())
            .collect();
        self.after = snapshot(editor, None)?;
        self.checks["nativeRmbWasdCrossedActualVolumeBoundary"] = json!(
            weights.len() == 3
                && weights[0] == 0.
                && weights[1] > 0.
                && weights[1] < weights[2]
                && (weights[2] - f64::from(volume.weight)).abs() < 0.0001
        );
        self.checks["retainedNativePostMatchesIndependentBlendAtEveryActualCameraPosition"] =
            json!(self.samples.iter().all(|sample| sample["correct"] == true));
        self.checks["cameraNavigationDidNotAuthorScene"] = json!(
            same(&self.before, &self.after) && self.before["revision"] == self.after["revision"]
        );
        self.observations["samples"] = json!(self.samples);
        self.finish_case()
    }
}
