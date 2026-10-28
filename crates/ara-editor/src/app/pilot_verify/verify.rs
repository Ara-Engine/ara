//! Independent authoring invariants over real typed transactions and parent matrices.
use super::*;
pub(super) fn unchanged(editor: &Editor, before: &Value, revision: u64) -> ara_ui::Result<()> {
    if editor.host.store().revision() != revision || fixture::records(editor)? != *before {
        return Err(
            "Pilot changed authored records during a transient or cancelled gesture".into(),
        );
    }
    Ok(())
}
pub(super) fn committed(
    editor: &Editor,
    before: &Value,
    revision: u64,
    view: &Camera,
    lens: bool,
) -> ara_ui::Result<Value> {
    if editor.host.store().revision() != revision + 1 {
        return Err(
            "Completed native Pilot gesture was not exactly one content transaction".into(),
        );
    }
    let after = fixture::records(editor)?;
    for id in [PARENT, BOX, SPHERE, COOKED] {
        if after[id] != before[id] {
            return Err(format!("Pilot clobbered unrelated fixture {id}").into());
        }
    }
    let transaction = editor
        .host
        .store()
        .history()
        .last()
        .ok_or("Pilot commit history absent")?;
    if transaction.why.as_deref() != Some("Pilot camera navigation")
        || transaction.ops.is_empty()
        || transaction
            .ops
            .iter()
            .any(|op| op.id().as_str() != CAMERA || op.doc().as_str() != editor.state.doc)
    {
        return Err("Pilot gesture did not use one attributed typed Camera transaction".into());
    }
    if after[CAMERA]["Camera"]["priority"] != before[CAMERA]["Camera"]["priority"]
        || after[CAMERA]["Transform"]["scale"] != before[CAMERA]["Transform"]["scale"]
    {
        return Err("Pilot altered camera priority or local scale".into());
    }
    let mut evidence = json!({"revisionBefore":revision,"revisionAfter":editor.host.store().revision(),
        "transaction":transaction,"view":view,"lensOnly":lens});
    if lens {
        if after[CAMERA]["Transform"] != before[CAMERA]["Transform"]
            || after[CAMERA]["Camera"]["fov"].as_f64() != Some(65.)
        {
            return Err("Native lens scrub did not change only FOV55→65".into());
        }
    } else {
        if after[CAMERA]["Camera"] != before[CAMERA]["Camera"] {
            return Err("Native move changed the source Camera lens".into());
        }
        let resolved =
            crate::world::resolve(editor.host.store(), &editor.state.doc).map_err(|problems| {
                format!("Pilot proof cannot resolve its committed Scene: {problems:?}")
            })?;
        let parent =
            crate::world::world_transform(&resolved, PARENT).ok_or("Pilot parent matrix absent")?;
        let before_record: ara_doc::Record = serde_json::from_value(before[CAMERA].clone())?;
        let before_world = parent * crate::world::local_transform(&before_record);
        let (world_scale, _, _) = before_world.to_scale_rotation_translation();
        let (right, up, forward) = view.basis().ok_or("Piloted view basis invalid")?;
        let rotation = Quat::from_mat3(&Mat3::from_cols(right, up, -forward)).normalize();
        let expected = parent.inverse()
            * Mat4::from_scale_rotation_translation(world_scale, rotation, view.position());
        let actual = crate::world::local_transform(
            &resolved
                .get(CAMERA)
                .ok_or("Committed Camera missing")?
                .record,
        );
        let error = actual
            .to_cols_array()
            .into_iter()
            .zip(expected.to_cols_array())
            .map(|(a, b)| (a - b).abs())
            .fold(0., f32::max);
        let world = crate::world::world_transform(&resolved, CAMERA)
            .ok_or("Committed Camera world missing")?;
        let position_error = world.transform_point3(Vec3::ZERO).distance(view.position());
        if !error.is_finite()
            || error > 0.001
            || !position_error.is_finite()
            || position_error > 0.001
        {
            return Err(format!("Native Pilot failed exact local parent inverse: matrix {error}, position {position_error}").into());
        }
        evidence["localMatrixError"] = json!(error);
        evidence["worldPositionError"] = json!(position_error);
        evidence["expectedLocalMatrix"] = json!(expected.to_cols_array());
        evidence["actualLocalMatrix"] = json!(actual.to_cols_array());
    }
    Ok(evidence)
}
pub(super) fn stats(shell: &Shell, report: &Value) -> ara_ui::Result<Value> {
    let stats = &report["stats"];
    let sample = &stats["sample"];
    if stats["samples"].as_u64().is_none_or(|n| n == 0)
        || stats["cadenceMs"] != 250
        || sample["cpuLoopMs"]
            .as_f64()
            .is_none_or(|n| !n.is_finite() || n < 0.)
        || sample["renderInstances"].as_u64().is_none_or(|n| n < 3)
    {
        return Err("Stats HUD has no actual completed CPU/native-instance sample".into());
    }
    let mut rows = Vec::new();
    for n in 0..6 {
        let id = format!("scene-diagnostic-stat-{n}");
        let row = shell
            .shown_node(&id)
            .and_then(|i| shell.ui.nodes[i].text.as_ref())
            .map(|t| t.value.clone())
            .ok_or("Observed stats row not mounted")?;
        rows.push(row);
    }
    for (field, row) in [
        ("gpuLongestPassMs", 1),
        ("geometryPayloadBytes", 3),
        ("compressedTextureBytes", 4),
    ] {
        if sample[field].is_null() && !rows[row].contains("unavailable") {
            return Err(format!("Unknown {field} was shown as a fabricated value").into());
        }
        if !sample[field].is_null()
            && sample[field]
                .as_f64()
                .is_none_or(|n| !n.is_finite() || n < 0.)
        {
            return Err(format!("Observed {field} is not finite/nonnegative").into());
        }
    }
    let cpu = sample["cpuLoopMs"]
        .as_f64()
        .ok_or("Observed CPU loop value missing")?;
    if !rows[0].contains(&format!("{cpu:.2} ms"))
        || !rows[2].contains(&sample["renderInstances"].to_string())
    {
        return Err("HUD labels differ from the actual cached CPU/native-instance sample".into());
    }
    if sample["extent"].is_null() && !rows[5].contains("unknown") {
        return Err("Unknown output extent was fabricated".into());
    }
    if sample["gpuPassSamples"].is_null() && !rows[1].contains("unknown") {
        return Err("Unknown GPU pass sample count was fabricated".into());
    }
    if rows
        .iter()
        .any(|row| row.contains("FPS") || row.contains("GPU frame") || row.contains("VRAM"))
    {
        return Err("Stats HUD claims an unobserved FPS/GPU-total/VRAM metric".into());
    }
    Ok(
        json!({"rows":rows,"observedSample":sample,"timingLabel":"informal (shared GPU)","noTimingThreshold":true}),
    )
}
