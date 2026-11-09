//! Only isolated fixture/navigation setup uses protocol; measured gestures use SDL.
use super::*;

pub(super) fn create(editor: &mut Editor) -> ara_ui::Result<Value> {
    let original = json!({"doc":editor.state.doc,"selection":editor.state.selection,
        "camera":editor.state.camera,"viewMode":editor.state.view_mode,
        "gameView":editor.state.game_view,"hideOverlays":editor.state.hide_overlays,"gizmos":!editor.state.hide_gizmos,
        "quality":editor.state.settings.get("viewport.quality"),
        "bounds":editor.state.settings.get("viewport.bounds"),"colliders":editor.state.settings.get("viewport.colliders"),
        "stats":editor.state.settings.get("viewport.stats")});
    editor.apply(vec![json!({"cmd":"doc.create","args":{"id":SCENE,"path":"levels/native-probes.scene","kind":"scene"}})],"Native J7 probe fixture document (typed setup)",None)?;
    editor.action("editor.scene.open", json!({"scene":SCENE}))?;
    editor.apply(vec![
        json!({"cmd":"entity.create","args":{"id":CUBE,"name":"Probe reflective cube","components":{"Transform":{},"MeshRenderer":{"size":[2,2,2]}}}}),
        json!({"cmd":"entity.create","args":{"id":SUN,"name":"Probe sun","components":{"Transform":{"rotation":[-40,25,0]},"Light":{"kind":"directional","intensity":3}}}}),
        json!({"cmd":"entity.create","args":{"id":"e_nativeprobefloor","name":"Probe floor","components":{"Transform":{"position":[0,-1.5,0]},"MeshRenderer":{"size":[12,0.25,12]}}}}),
    ],"Native J7 geometry/light fixture (typed setup)",None)?;
    for (key, value) in [
        ("viewport.quality", json!("high")),
        ("viewport.bounds", json!(false)),
        ("viewport.colliders", json!(false)),
        ("viewport.stats", json!(false)),
    ] {
        editor.action("editor.settings.set", json!({"key":key,"value":value}))?;
    }
    editor.action(
        "editor.view",
        json!({"viewMode":"lit","gameView":false,"gizmos":true,"hideOverlays":false}),
    )?;
    editor.action(
        "editor.camera",
        json!({"target":[0,0,0],"yaw":0.45,"pitch":0.3,"distance":22.,"projection":"perspective"}),
    )?;
    editor.action("editor.layout.show", json!({"panel":"scene"}))?;
    editor.action("editor.layout.show", json!({"panel":"inspector"}))?;
    editor.action("editor.layout.show", json!({"panel":"lighting"}))?;
    editor.action(
        "editor.layout.move",
        json!({"panel":"lighting","slot":"left"}),
    )?;
    Ok(original)
}
pub(super) fn records(editor: &Editor) -> ara_ui::Result<Value> {
    let doc = editor
        .host
        .store()
        .workspace()
        .doc(SCENE)
        .ok_or("Native probe fixture document missing")?;
    Ok(serde_json::to_value(doc.records())?)
}
pub(super) fn snapshot(editor: &Editor) -> ara_ui::Result<Value> {
    Ok(
        json!({"revision":editor.host.store().revision(),"records":records(editor)?,"selection":editor.state.selection,"viewMode":editor.state.view_mode}),
    )
}
impl ProbeCheck {
    pub(super) fn prepare(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        if self.index == 1 {
            let probe = self
                .probe
                .as_deref()
                .ok_or("Native-created probe identity missing")?;
            editor.apply(vec![json!({"cmd":"property.set","args":{"entity":probe,"path":"ReflectionProbe.captureResolution","value":64}})],"Probe resolution64 fixture baseline (typed setup)",None)?;
        }
        if self.index == 2 {
            let probe = self
                .probe
                .as_deref()
                .ok_or("Probe cage fixture identity missing")?;
            editor.apply(vec![
                json!({"cmd":"property.set","args":{"entity":probe,"path":"ReflectionProbe.bounds","value":[6,4,8]}}),
                json!({"cmd":"property.set","args":{"entity":probe,"path":"ReflectionProbe.projectionBounds","value":[10,6,12]}}),
                json!({"cmd":"property.set","args":{"entity":probe,"path":"ReflectionProbe.projectionOffset","value":[1,0,0]}}),
                json!({"cmd":"property.set","args":{"entity":probe,"path":"ReflectionProbe.captureOffset","value":[0.5,0.25,0]}}),
            ],"Distinct influence/projection/capture cage fixture (typed setup)",None)?;
        }
        editor.action(
            "editor.select",
            json!({"entities":self.probe.iter().collect::<Vec<_>>()}),
        )?;
        editor.action("editor.focus", json!({"panel":"lighting"}))?;
        shell.rebuild(editor)?;
        self.before = snapshot(editor)?;
        self.committed = Value::Null;
        self.observations = json!({"fixtureSetup":"Typed commands/navigation before the measured baseline; no capture backend called as a gesture"});
        self.previous_receipt = self
            .row()
            .and_then(|row| row.last_receipt.as_ref())
            .map(serde_json::to_value)
            .transpose()?
            .unwrap_or(Value::Null);
        self.job_baseline = shell.probe_report()["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|row| row["job"].as_str().map(str::to_owned))
            .collect();
        self.job = None;
        self.job_status = Value::Null;
        self.active_generation = None;
        self.renderer = Value::Null;
        self.cache_publication = None;
        self.next_query = Instant::now();
        self.pending_capture = Some("before");
        self.phase = 1;
        self.case_started = Instant::now();
        Ok(())
    }
    pub(super) fn restore(&mut self, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<()> {
        editor.action("editor.scene.open", json!({"scene":self.original["doc"]}))?;
        let camera = &self.original["camera"];
        editor.action("editor.camera",json!({"target":camera["target"],"yaw":camera["yaw"],"pitch":camera["pitch"],"distance":camera["distance"],"projection":camera["projection"],"fovYDegrees":camera["fovYDegrees"],"orthographicHeight":camera["orthographicHeight"]}))?;
        editor.action(
            "editor.select",
            json!({"entities":self.original["selection"]}),
        )?;
        editor.action("editor.view",json!({"viewMode":self.original["viewMode"],"gameView":self.original["gameView"],"hideOverlays":self.original["hideOverlays"],"gizmos":self.original["gizmos"]}))?;
        for (field, key) in [
            ("quality", "viewport.quality"),
            ("bounds", "viewport.bounds"),
            ("colliders", "viewport.colliders"),
            ("stats", "viewport.stats"),
        ] {
            if self.original[field].is_null() {
                editor.action("editor.settings.set", json!({"key":key,"reset":true}))?;
            } else {
                editor.action(
                    "editor.settings.set",
                    json!({"key":key,"value":self.original[field]}),
                )?;
            }
        }
        shell.desktop_pointer = None;
        self.pointer = None;
        shell.rebuild(editor)?;
        self.restored = editor.state.doc == self.original["doc"];
        self.write()
    }
}
