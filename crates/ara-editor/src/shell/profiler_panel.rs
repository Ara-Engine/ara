//! Frame timeline, flame graph, queue timings and complete two-capture comparisons.
use super::*;
impl Shell {
    pub(super) fn profiler_panel(
        &mut self,
        editor: &mut Editor,
        out: &mut String,
    ) -> ara_ui::Result<()> {
        let recording = editor.host.profiler().is_recording();
        out.push_str("<panel id='profiler-toolbar' class='toolbar'>");
        out.push_str(&self.bind_button(
            "profile-record",
            if recording {
                "Stop recording"
            } else {
                "Record"
            },
            Binding::Protocol(
                if recording {
                    "profile.stop"
                } else {
                    "profile.start"
                },
                json!({}),
            ),
            if recording { "small selected" } else { "small" },
        ));
        out.push_str(&self.bind_button(
            "profile-freeze",
            "Freeze capture",
            Binding::Protocol("editor.profile.freeze", json!({})),
            "small quiet",
        ));
        out.push_str(&self.bind_button(
            "profile-baseline",
            "Set baseline",
            Binding::Protocol("editor.profile.freeze", json!({"baseline":true})),
            "small quiet",
        ));
        out.push_str(&self.bind_button(
            "profile-compare",
            "Compare",
            Binding::Protocol(
                "editor.workspace",
                json!({"compare":!editor.state.profiler_compare}),
            ),
            "small quiet",
        ));
        out.push_str("<panel id='profile-toolbar-fill' style='flex-grow:1' /><text id='profiler-recording' class='badge' text='CPU active frame / GPU queue durations' /></panel>");
        let current = if let Some(profile) = &editor.profile_current {
            profile.as_ref()
        } else {
            editor.host.profiler().profile()
        };
        out.push_str("<scroll id='profiler-body' class='body' style='gap:12px'>");
        if editor.state.profiler_compare {
            if let Some(baseline) = &editor.profile_baseline {
                let budget = ara_profile::SummaryBudget {
                    top_n: 1024,
                    hitches: 64,
                    max_bytes: 1_048_576,
                };
                let before = baseline.summary(budget).map_err(String::from)?;
                let after = current.summary(budget).map_err(String::from)?;
                let comparison =
                    ara_profile::compare(&before, &after, 0.05, 1000).map_err(String::from)?;
                out.push_str(&format!("<panel id='profile-compare-heading' class='section'><text id='profile-compare-title' style='font-size:17px' text='Capture comparison' /><text id='profile-compare-meta' class='muted' text='Baseline: {} frames / Current: {} frames / p95' /><text id='profile-compare-verdict' class='{}' text='{}' /></panel>",before.frames,after.frames,if comparison.passed{"success"}else{"warning"},if comparison.passed{"Within regression budget"}else{"Review changed metrics"}));
                out.push_str("<panel id='profile-compare-columns' class='row'><text id='profile-metric-heading' class='field-label' text='Metric' style='flex-grow:1' /><text id='profile-baseline-heading' class='field-label' text='Baseline' style='width:100px' /><text id='profile-current-heading' class='field-label' text='Current' style='width:100px' /><text id='profile-delta-heading' class='field-label' text='Change' style='width:80px' /></panel>");
                for (n, delta) in comparison.deltas.iter().take(64).enumerate() {
                    out.push_str(&format!("<panel id='profile-delta-{n}' class='profile-delta'><text id='profile-delta-key-{n}' text='{}' style='flex-grow:1;flex-basis:0px;overflow:hidden' /><text id='profile-delta-before-{n}' class='number' text='{}' style='width:100px' /><text id='profile-delta-after-{n}' class='number' text='{}' style='width:100px' /><text id='profile-delta-change-{n}' class='{} number' text='{}' style='width:80px' /></panel>",escape(&delta.key),ms(delta.baseline_ns),ms(delta.current_ns),if delta.regression{"warning"}else{"success"},delta.change.map(|v|format!("{:+.1}%",v*100.)).unwrap_or_else(||"New".into())));
                }
            } else {
                out.push_str("<text id='profile-baseline-empty' class='muted' text='Set a baseline, record another capture, then freeze it to compare.' style='white-space:normal' />");
            }
            out.push_str("</scroll>");
            return Ok(());
        }
        if current.frames.is_empty() {
            out.push_str("<panel id='profile-empty' class='section'><text id='profile-empty-title' style='font-size:18px' text='Inspect a frame' /><text id='profile-empty-help' class='muted' text='Record while you work, then freeze a capture to inspect CPU scopes, GPU passes and hitches.' style='white-space:normal' /></panel></scroll>");
            return Ok(());
        }
        let selected = editor
            .state
            .profiler_frame
            .and_then(|id| current.frames.iter().find(|f| f.id == id))
            .or_else(|| current.frames.iter().rev().find(|f| f.duration_ns > 0))
            .ok_or("profile frame missing")?;
        let width = self
            .ui
            .find("profile-timeline")
            .map(|i| self.ui.nodes[i].rect.w)
            .unwrap_or(self.ui.size[0] * 0.58 - 24.);
        let visible = current.frames.len().min(120);
        let step = width / visible.max(1) as f32;
        let maximum = current
            .frames
            .iter()
            .map(|f| f.duration_ns)
            .max()
            .unwrap_or(1)
            .max(current.budget_ns);
        out.push_str(&format!("<panel id='profile-timeline-card' class='section' style='padding:8px;gap:8px'><panel id='profile-timeline-heading' class='row'><text id='profile-timeline-title' class='heading' text='Frame timeline' /><panel id='profile-timeline-fill' style='flex-grow:1' /><text id='profile-timeline-budget' class='badge' text='{} frames / {:.2} ms budget' /></panel><panel id='profile-timeline' style='height:76px;flex-shrink:0;overflow:hidden;background:var(--background);border-radius:var(--radius-sm)'>",current.frames.len(),current.budget_ns as f64/1e6));
        for (n, frame) in current
            .frames
            .iter()
            .skip(current.frames.len().saturating_sub(visible))
            .enumerate()
        {
            let height = (frame.duration_ns as f32 / maximum as f32 * 64.).clamp(2., 64.);
            let id = format!("profile-frame-{}", frame.id);
            self.bindings.insert(
                ara_ui::source::stable_id(&id),
                Binding::Protocol("editor.workspace", json!({"frame":frame.id})),
            );
            out.push_str(&format!("<button id='{id}' style='position:absolute;left:{}px;top:{}px;width:{}px;height:{height}px;padding:0px;border-width:0px;border-radius:2px;background:var(--{})' label='Frame {}: {:.3} ms' description='Select frame {}' action='Dispatch' />",n as f32*step,72.-height,(step-2.).max(1.),if frame.id==selected.id{"primary"}else if frame.duration_ns>current.budget_ns{"destructive-foreground"}else{"muted-foreground"},frame.id,frame.duration_ns as f64/1e6,frame.id));
        }
        out.push_str("</panel></panel>");
        out.push_str(&format!("<panel id='profile-frame-heading' class='row'><text id='profile-frame-title' style='font-size:16px' text='Frame {}' /><text id='profile-frame-duration' class='number' text='{:.3} ms' /><panel id='profile-frame-fill' style='flex-grow:1' /><text id='profile-frame-hitch' class='{}' text='{}' /></panel>",selected.id,selected.duration_ns as f64/1e6,if selected.duration_ns>current.budget_ns{"warning"}else{"success"},if selected.duration_ns>current.budget_ns{"Hitch"}else{"Within budget"}));
        out.push_str("<panel id='profile-flame-card' class='section' style='padding:8px;gap:8px'><text id='profile-flame-title' class='heading' text='CPU flame graph' /><panel id='profile-flame' style='height:160px;flex-shrink:0;overflow:hidden;background:var(--background)'>");
        let width = (width - 16.).max(100.);
        let spans = &selected.spans;
        let by_id = spans
            .iter()
            .map(|s| ((s.thread, s.id), s))
            .collect::<BTreeMap<_, _>>();
        for (n, span) in spans.iter().take(256).enumerate() {
            let mut parent = span.parent;
            let mut depth = 0;
            while parent != 0 && depth < 7 {
                depth += 1;
                parent = by_id
                    .get(&(span.thread, parent))
                    .map(|p| p.parent)
                    .unwrap_or(0);
            }
            let x = span.start_ns.saturating_sub(selected.start_ns) as f32
                / selected.duration_ns.max(1) as f32
                * width;
            let w = (span.duration_ns as f32 / selected.duration_ns.max(1) as f32 * width).max(1.);
            let label = current
                .labels
                .get(span.label as usize)
                .map(String::as_str)
                .unwrap_or("scope");
            out.push_str(&format!("<panel id='profile-span-{n}' style='position:absolute;left:{x}px;top:{}px;width:{w}px;height:20px;padding:2px;background:var(--{});border-color:var(--border);border-width:1px;border-radius:3px;overflow:hidden' description='{} / self {:.3} ms / total {:.3} ms'><text id='profile-span-label-{n}' style='font-size:10px' text='{}' /></panel>",depth*22,if depth%2==0{"accent-surface"}else{"secondary"},escape(label),span.self_ns as f64/1e6,span.duration_ns as f64/1e6,escape(label)));
        }
        out.push_str("</panel></panel><panel id='profile-detail-columns' style='flex-direction:row;gap:12px;flex-shrink:0'><panel id='profile-gpu' class='section' style='flex-grow:1;flex-basis:0px'><text id='profile-gpu-title' class='heading' text='GPU passes' />");
        if selected.gpu.is_empty() {
            out.push_str("<text id='profile-gpu-empty' class='badge' text='No GPU timestamps in this frame' />");
        }
        for (n, pass) in selected.gpu.iter().enumerate() {
            out.push_str(&format!("<panel id='profile-gpu-{n}' class='row'><text id='profile-gpu-name-{n}' class='muted' text='{}' style='flex-grow:1;flex-basis:0px;overflow:hidden' /><text id='profile-gpu-time-{n}' class='number' text='{:.3} ms' /></panel>",escape(current.labels.get(pass.label as usize).map(String::as_str).unwrap_or("pass")),pass.duration_ns as f64/1e6));
        }
        out.push_str("</panel><panel id='profile-counters' class='section' style='flex-grow:1;flex-basis:0px'><text id='profile-counters-title' class='heading' text='Counters' />");
        for (label, value) in [
            ("Entities", selected.counters.entities),
            ("UI draws", selected.counters.draws),
            ("UI triangles", selected.counters.triangles),
            ("Allocations", selected.memory.allocations),
        ] {
            out.push_str(&format!("<panel id='counter-{label}' class='row'><text id='counter-label-{label}' class='muted' text='{label}' style='flex-grow:1' /><text id='counter-value-{label}' class='number' text='{value}' /></panel>"));
        }
        out.push_str(&format!("<text id='profile-dropped' class='badge' text='{} dropped records' /></panel></panel></scroll>",selected.dropped));
        Ok(())
    }
}
fn ms(value: Option<u64>) -> String {
    value
        .map(|v| format!("{:.3} ms", v as f64 / 1e6))
        .unwrap_or_else(|| "Missing".into())
}
