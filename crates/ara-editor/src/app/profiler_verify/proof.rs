//! Assertions over real scoped captures and the UI's visible values, never injected statuses.
use super::*;
pub(super) fn scene(editor: &Editor) -> ara_ui::Result<String> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(&editor.state.doc)
        .ok_or("Profiler Scene missing")?;
    Ok(
        blake3::hash(ara_doc::write(doc, editor.host.store().schema()).as_bytes())
            .to_hex()
            .to_string(),
    )
}
pub(super) fn text(shell: &Shell, id: &str) -> Option<String> {
    let index = shell.shown_node(id)?;
    let node = &shell.ui.nodes[index];
    let visible = node.rect.intersection(node.clip);
    (visible.w > 0. && visible.h >= node.rect.h - 0.75)
        .then(|| node.text.as_ref().map(|text| text.value.clone()))
        .flatten()
}
pub(super) fn visible(shell: &Shell) -> Value {
    let labels = [
        "profile-current-scope",
        "profile-baseline-scope",
        "profile-cpu",
        "profile-gpu-total",
        "profiler-status-state",
        "profiler-status-stats",
        "profile-target-process",
        "profile-target-unavailable",
        "profile-target-pending",
        "profile-target-error",
        "profile-target-refresh-error",
    ];
    json!({"labels":labels.into_iter().map(|id|(id,text(shell,id))).collect::<std::collections::BTreeMap<_,_>>(),"recordDisabled":shell.shown_node("profile-record").map(|index|shell.ui.nodes[index].state.disabled),"recordLabel":text(shell,"profile-record-label"),"compareVisible":shell.shown_node("profile-compare-head").is_some()})
}
pub(super) fn clients(report: &Value, world: &str) -> Vec<Value> {
    report["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|row| {
            row["available"] == true
                && row["target"]["world"] == world
                && row["name"]
                    .as_str()
                    .is_some_and(|name| name.contains("Client"))
                && row["processId"]
                    .as_u64()
                    .is_some_and(|pid| pid != u64::from(std::process::id()))
        })
        .cloned()
        .collect()
}
fn labelled(label: Option<String>, scope: &Value) -> bool {
    label.is_some_and(|label| {
        scope["world"]
            .as_str()
            .is_some_and(|world| label.contains(world))
            && scope["player"]
                .as_u64()
                .is_some_and(|player| label.contains(&format!("player {player}")))
            && scope["source"]
                .as_str()
                .is_some_and(|source| label.contains(source))
            && scope["binding"]
                .as_u64()
                .is_some_and(|binding| label.contains(&format!("binding {binding}")))
    })
}
pub(super) fn validate_capture(report: &Value, target: &Value) -> ara_ui::Result<()> {
    let capture = &report["current"];
    if report["selected"] != *target
        || capture["target"] != *target
        || target["world"].as_str().is_none_or(str::is_empty)
        || target["source"].as_str().is_none_or(str::is_empty)
        || target["binding"]
            .as_u64()
            .is_none_or(|binding| binding == 0)
    {
        return Err(
            "Profiler data does not belong to the exact selected player incarnation".into(),
        );
    }
    if capture["frames"].as_u64().is_none_or(|n| n == 0 || n > 120)
        || capture["closedFrames"] != capture["frames"]
        || capture["minDurationNs"].as_u64().is_none_or(|n| n == 0)
        || capture["cpuSpanCount"].as_u64().is_none_or(|n| n == 0)
        || capture["hasPlayerFrameScope"] != true
        || capture["frameDigest"]
            .as_str()
            .is_none_or(|hash| hash.len() != 64)
    {
        return Err("Player capture lacks real bounded completed CPU frames/scopes".into());
    }
    Ok(())
}
pub(super) fn capture(shell: &Shell, report: &Value, target: &Value) -> ara_ui::Result<Value> {
    validate_capture(report, target)?;
    if !labelled(text(shell, "profile-current-scope"), target) {
        return Err(
            "Visible Profiler scope does not identify its actual world/player/source/binding"
                .into(),
        );
    }
    let cpu = text(shell, "profile-cpu").ok_or("Actual CPU frame value is not visible")?;
    let ns = report["current"]["displayFrame"]["durationNs"]
        .as_u64()
        .ok_or("Actual selected completed frame missing")?;
    let expected = if ns >= 1_000_000 {
        format!("CPU {:.2} ms", ns as f64 / 1e6)
    } else {
        format!("CPU {} \u{b5}s", (ns as f64 / 1e3).round())
    };
    if cpu != expected {
        return Err("Visible CPU label differs from its actual selected frame".into());
    }
    if report["current"]["displayFrame"]["gpuTimings"].is_null()
        && text(shell, "profile-gpu-total").as_deref() != Some("GPU \u{2014}")
    {
        return Err("Absent player GPU samples were not shown as unavailable".into());
    }
    Ok(
        json!({"capture":report["current"],"recording":report["recording"],"visible":visible(shell),"cpuLabelMatchesActualFrame":true,"backend":"real PlayProfilingHandle","noFallback":true}),
    )
}
pub(super) fn comparison(
    shell: &Shell,
    report: &Value,
    baseline: &Value,
    current: &Value,
) -> ara_ui::Result<()> {
    validate_capture(report, current)?;
    if report["compare"] != true
        || report["baseline"]["target"] != *baseline
        || baseline == current
        || !labelled(text(shell, "profile-current-scope"), current)
        || !labelled(text(shell, "profile-baseline-scope"), baseline)
    {
        return Err("Profiler comparison lost distinct explicit target scopes".into());
    }
    Ok(())
}
pub(super) fn unavailable(shell: &Shell, report: &Value, target: &Value) -> ara_ui::Result<()> {
    if report["selected"] != *target
        || report["current"]["target"] != *target
        || report["recording"] != false
    {
        return Err("Unavailable target silently fell back or kept recording".into());
    }
    if report["rows"].as_array().is_some_and(|rows| {
        rows.iter()
            .any(|row| row["target"] == *target && row["available"] == true)
    }) {
        return Err("Exited selected incarnation is still reported available".into());
    }
    if text(shell, "profile-target-unavailable").is_none()
        || !labelled(text(shell, "profile-current-scope"), target)
        || shell
            .shown_node("profile-record")
            .is_none_or(|index| !shell.ui.nodes[index].state.disabled)
    {
        return Err("Stale target has no truthful visible unavailable state".into());
    }
    Ok(())
}
