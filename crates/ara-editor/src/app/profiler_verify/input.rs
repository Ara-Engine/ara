//! All measured controls use raw SDL events and the visible retained targets.
use super::*;
impl ProfilerCheck {
    fn click(
        &mut self,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        id: &str,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<bool> {
        let Some(index) = shell.shown_node(id) else {
            return Ok(false);
        };
        let node = &shell.ui.nodes[index];
        let rect = node.rect.intersection(node.clip);
        if node.state.disabled
            || rect.w < 4.
            || rect.w < node.rect.w - 0.75
            || rect.h < node.rect.h - 0.75
        {
            return Ok(false);
        }
        let point = [rect.x + rect.w * 0.5, rect.y + rect.h * 0.5];
        let mut hit = shell.ui.hit_test(point[0], point[1]);
        while let Some(at) = hit {
            if at == index {
                break;
            }
            hit = shell.ui.nodes[at].element.parent;
        }
        if hit != Some(index) {
            return Ok(false);
        }
        if !scale.is_finite() || scale <= 0. {
            return Err("Profiler native scale invalid".into());
        }
        let window_rect = window.rect()?;
        let desktop = [
            window_rect[0] as f32 + point[0] / scale,
            window_rect[1] as f32 + point[1] / scale,
        ];
        self.pointer = Some(desktop);
        shell.desktop_pointer = Some(desktop);
        native::motion(window, point, scale, events)?;
        native::button(window, true, events)?;
        native::button(window, false, events)?;
        if self.trace.len() < 160 {
            self.trace.push(json!({"journey":CASES[self.index],"phase":self.phase,"control":id,"point":point,"desktop":desktop,"window":window.id(),"scale":scale}));
        }
        Ok(true)
    }
    fn choose_target(
        &self,
        window: &Window,
        target: &Value,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let rows = self.observed["rows"]
            .as_array()
            .ok_or("Actual player inventory missing")?;
        let index = rows
            .iter()
            .position(|row| row["target"] == *target)
            .ok_or("Requested incarnation left the actual selector inventory")?;
        native::tap(window, 74, events)?;
        for _ in 0..=index {
            native::tap(window, 81, events)?;
        }
        native::tap(window, 40, events)
    }
    fn idle(&self) -> bool {
        self.observed["pending"].is_null()
    }
    fn selected(&self, target: &Value) -> bool {
        self.observed["selected"] == *target
    }
    fn captured(&self, target: &Value) -> bool {
        self.selected(target)
            && self.observed["current"]["target"] == *target
            && self.observed["current"]["frames"]
                .as_u64()
                .is_some_and(|n| n >= 3)
    }
    pub(super) fn input(
        &mut self,
        editor: &mut Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
        scene_ready: bool,
    ) -> ara_ui::Result<()> {
        if self.started.elapsed() > PATIENCE {
            return Err(format!(
                "Native Profiler phase timed out: {} phase {}, report {}",
                CASES[self.index],
                self.phase,
                ara_json::to_canonical_string(&self.observed)
            )
            .into());
        }
        match self.index {
            0 => match self.phase {
                0 => {
                    if scene_ready && self.click(shell, window, scale, "play", events)? {
                        self.next();
                    }
                }
                1 => {
                    let Some(world) = editor.play_world.as_ref() else {
                        return Ok(());
                    };
                    let rows = proof::clients(&self.observed, world);
                    if rows.len() < 2 {
                        return Ok(());
                    }
                    self.world = Some(world.clone());
                    self.targets = rows
                        .into_iter()
                        .take(2)
                        .map(|row| row["target"].clone())
                        .collect();
                    native::tap(window, 41, events)?;
                    native::shortcut(window, 14, events)?;
                    self.next();
                }
                2 => {
                    if shell.shown_node("palette-search").is_some() {
                        native::shortcut(window, 4, events)?;
                        native::text(window, "window.profiler", events)?;
                        self.next();
                    }
                }
                3 => {
                    let command = shell
                        .command_bindings()
                        .into_iter()
                        .find(|(id, command)| {
                            id.starts_with("command-")
                                && command == "window.profiler"
                                && shell.shown_node(id).is_some()
                        })
                        .map(|(id, _)| id);
                    if let Some(id) = command
                        && self.click(shell, window, scale, &id, events)?
                    {
                        self.next();
                    }
                }
                4 => {
                    if shell.shown_node("profile-target").is_some() {
                        native::key(window, 225, true, events)?;
                        native::tap(window, 44, events)?;
                        native::key(window, 225, false, events)?;
                        self.next();
                    }
                }
                _ => {
                    if editor.state.maximized.as_deref() == Some("profiler")
                        && shell.shown_node("profile-target").is_some()
                        && self.observed["transport"] == true
                    {
                        self.done(json!({"targets":self.targets,"world":self.world,"actualRows":self.observed["rows"],"nativePanelMaximized":true}));
                    }
                }
            },
            1 => {
                let target = self.targets[0].clone();
                match self.phase {
                    0 => {
                        if self.click(shell, window, scale, "profile-target", events)? {
                            self.next();
                        }
                    }
                    1 => {
                        if shell.choice_open() {
                            self.choose_target(window, &target, events)?;
                            self.next();
                        }
                    }
                    2 => {
                        if self.selected(&target)
                            && self.idle()
                            && self.click(shell, window, scale, "profile-record", events)?
                        {
                            self.next();
                        }
                    }
                    _ => {
                        if self.idle()
                            && self.observed["recording"] == true
                            && self.captured(&target)
                        {
                            let evidence = proof::capture(shell, &self.observed, &target)?;
                            self.done(evidence);
                        }
                    }
                }
            }
            2 => {
                let target = self.targets[0].clone();
                if self.phase == 0 {
                    if self.idle() && self.captured(&target) {
                        self.before_serial = self.observed["current"]["serial"]
                            .as_u64()
                            .ok_or("Snapshot serial missing")?;
                        if self.click(shell, window, scale, "profile-snapshot", events)? {
                            self.next();
                        }
                    }
                } else if self.idle()
                    && self.observed["recording"] == true
                    && self.observed["current"]["serial"]
                        .as_u64()
                        .is_some_and(|serial| serial > self.before_serial)
                {
                    let mut evidence = proof::capture(shell, &self.observed, &target)?;
                    evidence["recordingRetained"] = json!(true);
                    evidence["explicitSnapshotButton"] = json!(true);
                    self.done(evidence);
                }
            }
            3 => {
                let target = self.targets[0].clone();
                match self.phase {
                    0 => {
                        if self.idle()
                            && self.click(shell, window, scale, "profile-freeze", events)?
                        {
                            self.next();
                        }
                    }
                    1 => {
                        if self.idle()
                            && self.observed["recording"] == false
                            && self.observed["current"]["frozen"] == true
                            && self.captured(&target)
                        {
                            proof::capture(shell, &self.observed, &target)?;
                            self.frozen = self.observed["current"].clone();
                            self.stability = Some(Instant::now());
                            self.next();
                        }
                    }
                    _ => {
                        if self
                            .stability
                            .is_some_and(|start| start.elapsed() >= Duration::from_secs(1))
                        {
                            if self.observed["current"] != self.frozen
                                || self.observed["recording"] != false
                            {
                                return Err(
                                    "Frozen client capture continued changing or recording".into(),
                                );
                            }
                            self.done(json!({"capture":self.frozen,"recordingStopped":true,"immutableAcrossOwnerTicks":true}));
                        }
                    }
                }
            }
            4 => {
                let target = self.targets[0].clone();
                if self.phase == 0 {
                    if self.idle()
                        && self.click(shell, window, scale, "profile-baseline", events)?
                    {
                        self.next();
                    }
                } else if self.idle()
                    && self.observed["baseline"]["target"] == target
                    && self.observed["baseline"]["frameDigest"] == self.frozen["frameDigest"]
                {
                    self.done(json!({"baseline":self.observed["baseline"],"current":self.observed["current"],"scopeLabel":proof::text(shell,"profile-baseline-scope")}));
                }
            }
            5 => {
                let target = self.targets[1].clone();
                match self.phase {
                    0 => {
                        if self.click(shell, window, scale, "profile-target", events)? {
                            self.next();
                        }
                    }
                    1 => {
                        if shell.choice_open() {
                            self.choose_target(window, &target, events)?;
                            self.next();
                        }
                    }
                    2 => {
                        if self.selected(&target)
                            && self.idle()
                            && self.click(shell, window, scale, "profile-record", events)?
                        {
                            self.next();
                        }
                    }
                    3 => {
                        if self.idle()
                            && self.observed["recording"] == true
                            && self.captured(&target)
                        {
                            proof::capture(shell, &self.observed, &target)?;
                            if self.click(shell, window, scale, "profile-freeze", events)? {
                                self.next();
                            }
                        }
                    }
                    _ => {
                        if self.idle()
                            && self.observed["recording"] == false
                            && self.observed["current"]["frozen"] == true
                            && self.captured(&target)
                        {
                            let mut evidence = proof::capture(shell, &self.observed, &target)?;
                            if self.observed["baseline"]["target"] != self.targets[0] {
                                return Err(
                                    "Switching clients retargeted the retained baseline".into()
                                );
                            }
                            evidence["separateBaseline"] = self.observed["baseline"].clone();
                            self.frozen = self.observed["current"].clone();
                            self.done(evidence);
                        }
                    }
                }
            }
            6 => {
                if self.phase == 0 {
                    if self.idle() && self.click(shell, window, scale, "profile-compare", events)? {
                        self.next();
                    }
                } else if self.observed["compare"] == true
                    && shell.shown_node("profile-compare-head").is_some()
                {
                    proof::comparison(shell, &self.observed, &self.targets[0], &self.targets[1])?;
                    self.done(json!({"current":self.observed["current"],"baseline":self.observed["baseline"],"currentLabel":proof::text(shell,"profile-current-scope"),"baselineLabel":proof::text(shell,"profile-baseline-scope"),"informalTimingOnly":true}));
                }
            }
            7 => self.stale(editor, shell, window, scale, events)?,
            _ => return Err("Native Profiler case index invalid".into()),
        }
        Ok(())
    }
    fn stale(
        &mut self,
        editor: &Editor,
        shell: &mut Shell,
        window: &Window,
        scale: f32,
        events: &mut Vec<(u32, Event)>,
    ) -> ara_ui::Result<()> {
        let target = self.targets[1].clone();
        match self.phase {
            0 => {
                if self.click(shell, window, scale, "stop", events)? {
                    self.next();
                }
            }
            1 => {
                if editor.play_world.is_none()
                    && self.idle()
                    && proof::unavailable(shell, &self.observed, &target).is_ok()
                {
                    self.trace.push(json!({"stage":"stopped-incarnation","report":self.observed,"visible":proof::visible(shell)}));
                    self.next();
                }
            }
            2 => {
                if self.click(shell, window, scale, "play", events)? {
                    self.next();
                }
            }
            3 => {
                let Some(world) = editor
                    .play_world
                    .as_ref()
                    .filter(|world| Some(world.as_str()) != self.world.as_deref())
                else {
                    return Ok(());
                };
                if proof::clients(&self.observed, world).len() < 2 {
                    return Ok(());
                }
                native::tap(window, 41, events)?;
                native::shortcut(window, 14, events)?;
                self.next();
            }
            4 => {
                if shell.shown_node("palette-search").is_some() {
                    native::shortcut(window, 4, events)?;
                    native::text(window, "window.profiler", events)?;
                    self.next();
                }
            }
            5 => {
                let command = shell
                    .command_bindings()
                    .into_iter()
                    .find(|(id, command)| {
                        id.starts_with("command-")
                            && command == "window.profiler"
                            && shell.shown_node(id).is_some()
                    })
                    .map(|(id, _)| id);
                if let Some(id) = command
                    && self.click(shell, window, scale, &id, events)?
                {
                    self.next();
                }
            }
            6 => {
                if shell.shown_node("profile-target").is_none() {
                    return Ok(());
                }
                proof::unavailable(shell, &self.observed, &target)?;
                if self.observed["current"] != self.frozen {
                    return Err(
                        "Replacement Play silently replaced the stale selected capture".into(),
                    );
                }
                let world = editor
                    .play_world
                    .as_ref()
                    .ok_or("Replacement world disappeared")?;
                let fresh = proof::clients(&self.observed, world);
                if fresh.iter().any(|row| row["target"] == target) {
                    return Err("Replacement client reused the stale scope".into());
                }
                self.trace.push(json!({"stage":"replacement-live-old-target-retained","report":self.observed,"visible":proof::visible(shell)}));
                if self.click(shell, window, scale, "stop", events)? {
                    self.next();
                }
            }
            _ => {
                if editor.play_world.is_none() && self.idle() {
                    proof::unavailable(shell, &self.observed, &target)?;
                    self.done(json!({"selected":target,"current":self.observed["current"],"replacementDidNotRetarget":true,"bothOwnedWorldsStopped":true,"availableRows":self.observed["rows"].as_array().map(|rows|rows.iter().filter(|row|row["available"]==true).count()),"visible":proof::visible(shell)}));
                }
            }
        }
        Ok(())
    }
}
