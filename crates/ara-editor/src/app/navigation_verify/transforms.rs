//! Native J1 transforms: independent world-matrix expectations, transient preview and history.
use super::*;
use glam::{Mat4, Quat, Vec2};
mod input;
#[cfg(test)]
mod tests;

const PARENT: &str = "e_j1parent";
const FIRST: &str = "e_j1first";
const SECOND: &str = "e_j1second";
const SELECTED: [&str; 2] = [FIRST, SECOND];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Action { World, Local, Centre, Pivot, Precision, Cancel }
impl Action {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::World => "transform-world-axis-rotated-parent-one-undo",
            Self::Local => "transform-local-axis-rotated-parent-one-undo",
            Self::Centre => "transform-centre-rotation-15-degree-snap",
            Self::Pivot => "transform-individual-pivots-rotation-15-degree-snap",
            Self::Precision => "transform-quarter-metre-snap-one-undo",
            Self::Cancel => "transform-snapped-preview-escape-no-ops",
        }
    }
    fn rotate(self) -> bool { matches!(self, Self::Centre | Self::Pivot) }
    fn snap(self) -> bool { !matches!(self, Self::World | Self::Local) }
}
pub(super) struct Proof {
    action: Action,
    before: Value,
    stages: Value,
    expected: std::collections::BTreeMap<String, Mat4>,
    from: [f32; 2],
    to: [f32; 2],
    geometry: Value,
    checks: Value,
    native_events: usize,
}

fn snapshot(editor: &Editor) -> ara_ui::Result<Value> {
    let resolved = crate::world::resolve(editor.host.store(), &editor.state.doc)
        .map_err(|diagnostics| format!("{diagnostics:?}"))?;
    let worlds = crate::world::world_transforms(&resolved);
    let records = editor.host.store().workspace().doc(&editor.state.doc)
        .ok_or("Native J1 Scene disappeared")?.records();
    let canonical = ara_json::to_canonical_string(&json!(records));
    let matrices: std::collections::BTreeMap<_,_> = [PARENT,FIRST,SECOND].into_iter()
        .filter_map(|id| worlds.get(id).map(|matrix|(id,matrix.to_cols_array()))).collect();
    Ok(json!({"revision":editor.host.store().revision(),"documents":{"algorithm":"blake3","schema":"canonical-scene-records-utf8-v1","bytes":canonical.len(),"hash":blake3::hash(canonical.as_bytes()).to_hex().to_string()},"worlds":matrices}))
}
fn same_documents(a: &Value, b: &Value) -> bool {
    a["documents"].is_object() && a["documents"] == b["documents"]
}
fn close(a: Mat4, b: Mat4) -> bool {
    a.is_finite() && b.is_finite() && a.to_cols_array().into_iter().zip(b.to_cols_array())
        .all(|(a,b)|(a-b).abs()<0.002)
}
fn matrix(value: &Value) -> Option<Mat4> {
    let array = value.as_array()?;
    if array.len()!=16 { return None; }
    let mut values=[0.;16];
    for (out,value) in values.iter_mut().zip(array) { *out=value.as_f64()? as f32; }
    let matrix=Mat4::from_cols_array(&values);
    matrix.is_finite().then_some(matrix)
}

/// Expected transforms use geometric operations on the captured world poses,
/// independently of Gesture::update and the production command lowering.
fn expected(action: Action, worlds: &std::collections::BTreeMap<String,Mat4>) -> ara_ui::Result<std::collections::BTreeMap<String,Mat4>> {
    let first=*worlds.get(FIRST).ok_or("Native J1 first world pose missing")?;
    let second=*worlds.get(SECOND).ok_or("Native J1 second world pose missing")?;
    let centre=(first.transform_point3(Vec3::ZERO)+second.transform_point3(Vec3::ZERO))*0.5;
    let axis=if action==Action::Local {first.to_scale_rotation_translation().1*Vec3::X} else {Vec3::X};
    let movement=axis*if action.snap(){0.75}else{1.1};
    SELECTED.into_iter().map(|id| {
        let world=*worlds.get(id).ok_or("Native J1 selected world pose missing")?;
        let next=if action.rotate() {
            let pivot=if action==Action::Centre {centre} else {world.transform_point3(Vec3::ZERO)};
            Mat4::from_translation(pivot)*Mat4::from_quat(Quat::from_rotation_z(30f32.to_radians()))*Mat4::from_translation(-pivot)*world
        } else {Mat4::from_translation(movement)*world};
        Ok((id.to_owned(),next))
    }).collect()
}

impl Proof {
    pub(super) fn setup(action: Action, editor: &mut Editor, shell: &mut Shell) -> ara_ui::Result<Self> {
        shell.leave();
        shell.menu=None;
        shell.dialog=None;
        editor.action("editor.palette",json!({"open":false}))?;
        editor.action("editor.layout.reset",json!({}))?;
        editor.action("editor.focus",json!({"panel":"scene"}))?;
        let existing=crate::world::resolve(editor.host.store(), &editor.state.doc).map_err(|d|format!("{d:?}"))?;
        let mut calls=Vec::new();
        for (id,position,rotation) in [(PARENT,[0.,0.,0.],[0.,30.,0.]),(FIRST,[-2.,0.,0.],[0.,0.,30.]),(SECOND,[2.,0.,0.],[0.,0.,30.])] {
            if !existing.contains_key(id) {
                calls.push(json!({"cmd":"entity.create","args":{"id":id,"name":id,"components":{"Transform":{},"MeshRenderer":{"size":[0.5,0.5,0.5]}}}}));
            }
            calls.push(json!({"cmd":"entity.reparent","args":{"entity":id,"parent":if id==FIRST {Some(PARENT)}else{None}}}));
            calls.push(json!({"cmd":"transform.set","args":{"entities":[id],"position":position,"rotation":rotation,"scale":[1.,1.,1.]}}));
        }
        editor.apply(calls,"Set up rotated-parent multi-selection native transform fixture",None)?;
        editor.action("editor.select",json!({"entities":SELECTED}))?;
        editor.action("editor.tool",json!({"tool":"translate","local":false,"centre":false,"snapping":false,"moveSnap":0.25,"rotateSnap":15.}))?;
        editor.action("editor.view",json!({"gizmos":true,"gameView":false,"hideOverlays":false}))?;
        editor.action("editor.settings.set",json!({"key":"viewport.surfaceSnap","value":false}))?;
        editor.action("editor.settings.set",json!({"key":"viewport.vertexSnap","value":false}))?;
        editor.action("editor.camera",json!({"target":[0.,0.,0.5],"yaw":0.,"pitch":0.,"distance":20.,"fovYDegrees":50.,"projection":"perspective"}))?;
        let resolved=crate::world::resolve(editor.host.store(), &editor.state.doc).map_err(|d|format!("{d:?}"))?;
        let worlds=crate::world::world_transforms(&resolved);
        let expected=expected(action,&worlds)?;
        shell.rebuild(editor)?;
        shell.input.blur(&mut shell.ui);
        Ok(Self{action,before:snapshot(editor)?,stages:json!({}),expected,from:[0.;2],to:[0.;2],geometry:Value::Null,checks:json!({}),native_events:0})
    }
    pub(super) fn end(&mut self, age:u32, editor:&Editor, shell:&Shell, gpu:&mut NativeUi, output:&Path, index:usize) -> ara_ui::Result<Option<Value>> {
        let stage=match age {1=>Some("before"),10=>Some("preview"),13=>Some("after"),16 if self.action!=Action::Cancel=>Some("undo"),19=>Some(if self.action==Action::Cancel{"cancel-settled"}else{"redo"}),_=>None};
        if let Some(stage)=stage {
            gpu.select_target(0);
            gpu.pixels()?.save(output.join(format!("nav-{index:02}-{}-{stage}.png",self.action.label())))?;
            let now=snapshot(editor)?;
            self.stages[stage]=now.clone();
            if age==10 {
                let resolved=crate::world::resolve(editor.host.store(),&editor.state.doc).map_err(|d|format!("{d:?}"))?;
                let worlds=crate::world::world_transforms(&resolved);
                let gesture=editor.gesture.as_ref();
                let preview_matches=gesture.is_some_and(|gesture|self.expected.iter().all(|(id,expected)| {
                    let parent=resolved.get(id).and_then(|entity|entity.parent.as_ref()).and_then(|id|worlds.get(id)).copied().unwrap_or(Mat4::IDENTITY);
                    gesture.current.get(id).is_some_and(|local|close(parent*(*local),*expected))
                }));
                self.checks["transientPreviewMatchesIndependentWorldGeometry"]=json!(preview_matches);
                self.checks["previewHasNoAuthoringOps"]=json!(same_documents(&self.before,&now)&&now["revision"]==self.before["revision"]);
                self.stages["preview"]["provisionalLocals"]=json!(gesture.map(|gesture|gesture.current.iter().map(|(id,matrix)|(id,matrix.to_cols_array())).collect::<std::collections::BTreeMap<_,_>>()));
            }
        }
        if age<19 {return Ok(None);}
        let after=&self.stages["after"];
        let revision=self.before["revision"].as_u64().ok_or("Native J1 base revision missing")?;
        if self.action==Action::Cancel {
            self.checks["escapeLeavesAllRecordsAndRevisionUnchanged"]=json!(same_documents(&self.before,after)&&after["revision"]==revision&&same_documents(after,&self.stages["cancel-settled"])&&self.stages["cancel-settled"]["revision"]==revision);
        } else {
            self.checks["finalWorldMatricesMatchPreview"]=json!(self.expected.iter().all(|(id,expected)|matrix(&after["worlds"][id]).is_some_and(|actual|close(actual,*expected))));
            self.checks["releaseOneTransaction"]=json!(after["revision"]==revision+1);
            self.checks["oneNativeUndoRestoresAllSceneRecords"]=json!(same_documents(&self.before,&self.stages["undo"])&&self.stages["undo"]["revision"]==revision+2);
            self.checks["oneNativeRedoRestoresCompleteGesture"]=json!(same_documents(after,&self.stages["redo"])&&self.stages["redo"]["revision"]==revision+3);
        }
        self.checks["gestureCaptureReleased"]=json!(editor.gesture.is_none());
        self.checks["nativeToolAndSpaceReached"]=json!(editor.state.local==(self.action==Action::Local)&&editor.state.centre==(self.action==Action::Centre)&&editor.state.snapping==self.action.snap()&&editor.state.tool==if self.action.rotate(){crate::state::Tool::Rotate}else{crate::state::Tool::Translate});
        self.checks["unselectedParentWorldUnchanged"]=json!(self.before["worlds"][PARENT]==after["worlds"][PARENT]);
        let ok=self.checks.as_object().is_some_and(|checks|checks.values().all(|value|value==true));
        Ok(Some(json!({"ok":ok,"checks":self.checks,"geometry":self.geometry,"expectedWorlds":self.expected.iter().map(|(id,matrix)|(id,matrix.to_cols_array())).collect::<std::collections::BTreeMap<_,_>>(),"before":self.before,"stages":self.stages,"nativeEvents":self.native_events,"camera":editor.state.camera,"viewport":shell.viewport(),"ingress":"SDL toolbar pointer clicks and real gizmo press/motion/release or Escape; Ctrl-Z/Ctrl-Y through production router"})))
    }
}
