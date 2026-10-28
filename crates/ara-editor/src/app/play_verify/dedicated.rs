//! Native Dedicated + three rendering clients; probes below are read-only observations.
use super::super::input_verify::native;
use super::*;
mod input;
mod proof;
#[cfg(test)]
mod tests;

const ENTITIES: &[&str] = &[ACTOR, "e_player02", "e_player03", "e_player04"];

impl PlayCheck {
    fn dedicated_world(&mut self, editor: &mut Editor) -> ara_ui::Result<Option<Value>> {
        let status = editor.action("play.status", json!({}))?;
        Ok(status["worlds"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|row| row["world"] == editor.play_world.as_deref().unwrap_or_default())
            .cloned())
    }
    fn dedicated_read(&mut self, editor: &mut Editor, label: &str) -> ara_ui::Result<()> {
        let paths: Vec<_> = ENTITIES
            .iter()
            .flat_map(|entity| {
                [
                    format!("{entity}.Transform.position"),
                    format!("{entity}.PlayerControl.player"),
                ]
            })
            .collect();
        if let Some(value) = self.probe(editor, "play.read", json!({"paths":paths}))? {
            self.proofs.insert(label.into(), value);
        }
        Ok(())
    }
    pub(super) fn dedicated_input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        if !scale.is_finite() || scale <= 0. {
            return Err("Dedicated Play input scale is invalid".into());
        }
        let target = match self.age {
            2 | 13 => Some("game-options"),
            4 => Some("play-options-mode"),
            5 => Some("play-client-count"),
            6 => Some("play-process-mode"),
            7 => Some("play-network-1"),
            8 => Some("play-network-2"),
            9 => Some("play-network-3"),
            _ => None,
        };
        if let Some(id) = target
            && !self.dedicated_visible(shell, window, id, scale, events)?
        {
            return Ok(());
        }
        match self.age {
            0 => {
                self.began = Instant::now();
                native::focus_lost(window, events)?;
                self.setup(editor, shell)?;
            }
            2 => input::click(shell, window, "game-options", scale, events)?,
            4 => input::select(shell, window, "play-options-mode", 2, scale, events)?,
            5 => input::replace(shell, window, "play-client-count", "3", scale, events)?,
            6 => input::select(shell, window, "play-process-mode", 0, scale, events)?,
            // Previous stories intentionally left client1 shaped. Reset both
            // other clients through the visible controls, then shape only client3.
            7 => input::select(shell, window, "play-network-1", 0, scale, events)?,
            8 => input::select(shell, window, "play-network-2", 0, scale, events)?,
            9 => input::select(shell, window, "play-network-3", 4, scale, events)?,
            10 => {
                self.configured_baseline(editor)?;
                self.proofs.insert("probe-contract".into(),json!({"read":"play.read background job, authority player0 only","status":"play.status cached role/heartbeat/live-proxy observations","mutationIngress":"raw SDL selectors, field input, F5/ShiftF5, capture clicks, W/D keys and FocusLost; no runtime-control mutation calls"}));
                native::tap(window, 62, events)?;
            }
            11 => self.ready(shell, 3)?,
            12 => {
                let Some(world) = self.dedicated_world(editor)? else {
                    return self.wait();
                };
                let Some(topology) = proof::topology(&world) else {
                    self.proofs.insert("last-dedicated-status".into(), world);
                    return self.wait();
                };
                self.proofs.insert("processes".into(), world);
                self.proofs.insert("dedicated-topology".into(), topology);
            }
            13 => input::click(shell, window, "game-options", scale, events)?,
            14 => self.dedicated_read(editor, "dedicated-before")?,
            15 | 20 => {
                let player = proof::focused_client(&self.proofs)
                    .ok_or("Dedicated client2 identity missing")?;
                input::click(
                    shell,
                    window,
                    &format!("game-frame-{player}"),
                    scale,
                    events,
                )?;
            }
            16 => native::key(window, 26, true, events)?,
            17 => {
                if self.held(450)? {
                    return Ok(());
                }
                native::key(window, 26, false, events)?;
                native::tap(window, 41, events)?;
            }
            18 => {
                if self.held(250)? {
                    return Ok(());
                }
                self.dedicated_read(editor, "dedicated-client2")?;
            }
            19 => {
                if self.held(250)? {
                    return Ok(());
                }
                self.dedicated_read(editor, "dedicated-key-released")?;
            }
            21 => native::key(window, 7, true, events)?,
            22 => {
                if self.held(450)? {
                    return Ok(());
                }
                native::focus_lost(window, events)?;
            }
            23 => {
                if self.held(250)? {
                    return Ok(());
                }
                self.dedicated_read(editor, "dedicated-focuslost")?;
            }
            24 => {
                if self.held(250)? {
                    return Ok(());
                }
                self.dedicated_read(editor, "dedicated-focuslost-settled")?;
                if !self.waiting
                    && let Some(world) = self.dedicated_world(editor)?
                {
                    self.proofs.insert("applied-link-status".into(), world);
                }
            }
            25 => {
                native::key(window, 26, false, events)?;
                native::key(window, 7, false, events)?;
                native::tap(window, 41, events)?;
            }
            26 => {
                native::key(window, 225, true, events)?;
                native::tap(window, 62, events)?;
                native::key(window, 225, false, events)?;
            }
            27 => {
                if editor.play_world.is_some() || editor.state.playing.is_some() {
                    return self.wait();
                }
                let status = editor.action("play.status", json!({}))?;
                let world = &self.proofs.get("processes").unwrap_or(&Value::Null)["world"];
                if status["stopping"]
                    .as_array()
                    .is_some_and(|rows| rows.iter().any(|row| &row["world"] == world))
                {
                    return self.wait();
                }
                self.proofs.insert("stop-status".into(), status);
            }
            _ => {}
        }
        Ok(())
    }
    pub(super) fn dedicated_end(
        &mut self,
        editor: &Editor,
        shell: &Shell,
        gpu: &mut NativeUi,
    ) -> ara_ui::Result<()> {
        let stage = match self.age {
            3 => Some("before"),
            9 => Some("configured"),
            14 => Some("ready"),
            16 => Some("client2-captured"),
            18 => Some("client2-moved"),
            19 => Some("key-released"),
            21 => Some("client2-recaptured"),
            23 => Some("focuslost"),
            24 => Some("focuslost-settled"),
            30 => Some("after"),
            _ => None,
        };
        if let Some(stage) = stage {
            gpu.select_target(0);
            let pixels = gpu.pixels()?;
            let prefix = format!(
                "play-{:02}-{}-{stage}",
                self.index + 1,
                Act::Dedicated.label()
            );
            pixels.save(self.output.join(format!("{prefix}.png")))?;
            std::fs::write(
                self.output.join(format!("{prefix}.layout.json")),
                ara_json::to_canonical_string(&shell.layout_report()),
            )?;
            let tiles = proof::tiles(shell, &self.proofs, &pixels);
            self.trace.push(json!({"stage":stage,"revision":editor.host.store().revision(),"world":editor.play_world,"playing":editor.state.playing,"runtime":shell.play_runtime_report(),"tiles":tiles,
                "scene":configuration::scene(editor)?,"proofs":self.proofs}));
        }
        if self.age == Act::Dedicated.finish() {
            let checks = proof::checks(editor, self);
            let unmet: Vec<_> = checks
                .as_object()
                .into_iter()
                .flat_map(|map| map.iter())
                .filter(|(_, value)| **value != Value::Bool(true))
                .map(|(name, _)| name.as_str())
                .collect();
            let ok = unmet.is_empty();
            self.results.push(json!({"journey":Act::Dedicated.label(),"ok":ok,"checks":checks,"unmet":unmet,"trace":self.trace,"proofs":self.proofs,"configuration":self.configuration,"waits":self.waits.values(),
                "networkProof":"requires live authority NetHost link conditions; requested policy alone is never accepted"}));
            self.advance();
            self.write()?;
        } else {
            self.age += 1;
            self.step_began = Instant::now();
        }
        Ok(())
    }
}
