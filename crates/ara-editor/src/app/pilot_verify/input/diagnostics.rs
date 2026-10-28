//! Real palette toggles, current native geometry, cache reuse and honest HUD labels.
use super::*;
impl PilotCheck {
    pub(super) fn diagnostics(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        let command = if self.phase <= 3 {
            "view.show.bounds"
        } else if self.phase <= 7 {
            "view.show.colliders"
        } else {
            "view.show.stats"
        };
        let offset = if self.phase <= 3 {
            0
        } else if self.phase <= 7 {
            4
        } else {
            8
        };
        if matches!(self.phase,1..=3|5..=7|9..=11) {
            self.phase -= offset;
            let result = self.palette(shell, window, scale, command, events);
            self.phase += offset;
            return result.map(|_| ());
        }
        if self.phase == 4 || self.phase == 8 {
            self.next();
            return Ok(());
        }
        if !scene_ready {
            return Ok(());
        }
        for key in ["viewport.bounds", "viewport.colliders", "viewport.stats"] {
            if editor.state.settings.get(key) != Some(&json!(true)) {
                return Err(format!("Native palette toggle did not enable {key}").into());
            }
        }
        let report = shell.layout_report()["viewportDiagnostics"].clone();
        let counts = &report["counts"];
        if report["nativeSourceCurrent"] != true
            || counts["meshBounds"] != 3
            || counts["colliders"] != 2
            || counts["sensors"] != 1
            || counts["unsupportedColliders"] != 1
            || counts["invalidColliders"] != 0
            || report["projectedSegments"].as_u64().is_none_or(|n| n == 0)
        {
            return Ok(());
        }
        let notice = shell
            .shown_node("scene-diagnostic-notice-text")
            .and_then(|i| shell.ui.nodes[i].text.as_ref())
            .map(|t| t.value.as_str());
        if !notice.is_some_and(|note| note.contains("1 cooked/model collider shapes unavailable")) {
            return Err(
                "Unavailable cooked collider was silently omitted instead of labeled".into(),
            );
        }
        let key = (
            report["geometryBuilds"]
                .as_u64()
                .ok_or("Diagnostic geometry build count missing")?,
            report["projectionBuilds"]
                .as_u64()
                .ok_or("Diagnostic projection build count missing")?,
        );
        if self.cache == Some(key) {
            self.stable += 1;
        } else {
            self.cache = Some(key);
            self.stable = 0;
            self.stats_started = Some(Instant::now());
            self.stats_before = report["stats"]["samples"].as_u64().unwrap_or(0);
        }
        if self.stable < 10
            || self
                .stats_started
                .is_none_or(|t| t.elapsed() < Duration::from_millis(300))
            || report["stats"]["samples"]
                .as_u64()
                .is_none_or(|n| n <= self.stats_before)
        {
            return Ok(());
        }
        let stats = verify::stats(shell, &report)?;
        verify::unchanged(editor, &self.before, self.revision)?;
        self.done(json!({"nativePaletteToggles":["view.show.bounds","view.show.colliders","view.show.stats"],
            "viewportDiagnostics":report,"unchangedCacheFrames":self.stable,"stats":stats,
            "unsupportedCookedNotice":notice,"authoringUnchanged":true,"ingress":"SDL palette commands and actual presented Scene/HUD capture"}));
        Ok(())
    }
}
