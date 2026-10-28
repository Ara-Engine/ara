//! Reproduce Float → next-frame Move Right → already queued native float Resize.
use super::*;
use sdl3_sys::{events::*, video::SDL_WindowID};
use std::{collections::BTreeSet,path::Path,time::{Duration,Instant}};

#[derive(Clone,Copy,Debug)]
pub(super) struct Act {pub panel:&'static str,pub scale:f64}
const CYCLES:u32=3;
const LIMIT:Duration=Duration::from_secs(10);

fn in_right(node:&DockNode,panel:&str) -> bool {
    match node {
        DockNode::Area{id,tabs,..}=>id=="right" && tabs.iter().any(|tab|tab==panel),
        DockNode::Split{first,second,..}=>in_right(first,panel) || in_right(second,panel),
    }
}

#[derive(Default)]
pub(super) struct Check {
    preferences:Option<(Value,Value)>,
    camera:Option<crate::state::Camera>,
    selection:Vec<String>,
    doc:String,
    revision:u64,
    pending:Option<(Instant,&'static str,u32)>,
    injected:Vec<Value>,
    observed:Vec<Value>,
    failures:Vec<String>,
    captures:usize,
    restored:bool,
}

fn chord(window:u32,scan:i32,events:&mut Vec<(u32,Event)>) -> ara_ui::Result<()> {
    let primary=if cfg!(target_os="macos"){227}else {224};
    for (scancode,pressed) in [(primary,true),(225,true),(scan,true),(scan,false),(225,false),(primary,false)] {
        scene::native(window,Event::Key{scancode,pressed,repeat:false},events)?;
    }
    Ok(())
}
fn pixel_resize(window:u32,events:&mut Vec<(u32,Event)>) -> ara_ui::Result<()> {
    let mut event=SDL_Event {window:SDL_WindowEvent {
        r#type:SDL_EVENT_WINDOW_PIXEL_SIZE_CHANGED,windowID:SDL_WindowID(window),..Default::default()
    }};
    // SAFETY: the window event member matches its tag and names a live owned SDL window.
    if !unsafe {SDL_PushEvent(&mut event)} {return Err("Rapid docking: SDL rejected the queued native resize".into());}
    events.extend(Window::poll_tagged()?);
    Ok(())
}

impl Check {
    #[allow(clippy::too_many_arguments)]
    pub fn inject(&mut self,act:Act,index:usize,age:u32,editor:&mut Editor,shell:&mut Shell,window:&Window,floats:&windows::Floats,events:&mut Vec<(u32,Event)>,output:&Path) -> ara_ui::Result<()> {
        if !shell.offscreen || !floats.hidden {return Err("Rapid docking requires the isolated hidden-window verifier".into());}
        if age==0 {
            self.restore(editor,shell)?;
            *self=Self::default();
            self.preferences=Some((editor.state.settings.get("input.shortcuts").cloned().unwrap_or_else(||json!({})),editor.state.settings.get("appearance.uiScale").cloned().unwrap_or_else(||json!(1.))));
            self.camera=Some(editor.state.camera);
            self.selection.clone_from(&editor.state.selection);
            self.doc.clone_from(&editor.state.doc);
            self.revision=editor.host.store().revision();
            let mut shortcuts=self.preferences.as_ref().map(|(keys,_)|keys.clone()).ok_or("Rapid docking preferences missing")?;
            shortcuts["window.float"]=json!("Ctrl+Shift+F11");
            shortcuts["window.move.right"]=json!("Ctrl+Shift+F12");
            shell.leave();
            shell.menu=None;
            shell.dialog=None;
            editor.action("editor.layout.reset",json!({}))?;
            editor.action("editor.layout.show",json!({"panel":act.panel}))?;
            editor.action("editor.focus",json!({"panel":act.panel}))?;
            editor.action("editor.settings.set",json!({"key":"input.shortcuts","value":shortcuts}))?;
            editor.action("editor.settings.set",json!({"key":"appearance.uiScale","value":act.scale}))?;
            shell.rebuild(editor)?;
            shell.input.blur(&mut shell.ui);
        }
        if age==10 {self.restore(editor,shell)?;}
        let float=(1..=CYCLES).find(|cycle|age==1+3*(cycle-1));
        let redock=(1..=CYCLES).find(|cycle|age==2+3*(cycle-1));
        if let Some(cycle)=float {
            shell.leave();
            let mut keys=Vec::new();
            let started=Instant::now();
            chord(window.id(),68,&mut keys)?;
            self.pending=Some((started,"float",cycle));
            self.injected.push(json!({"age":age,"cycle":cycle,"action":"Float","ingress":"raw SDL key -> poll_tagged -> production route","events":keys.iter().map(|(id,event)|json!({"window":id,"event":format!("{event:?}")})).collect::<Vec<_>>()}));
            events.extend(keys);
        } else if let Some(cycle)=redock {
            let target=floats.windows.iter().find(|float|editor.state.floating.iter().any(|layout|layout.id==float.id && area(&layout.root,act.panel).is_some())).ok_or("Native Float did not create its actual owned window on the preceding frame")?;
            let mut queued=std::mem::take(events);
            let points=[target.rect[2].max(320) as u32+53+cycle,target.rect[3].max(240) as u32+31+cycle];
            target.window.set_size(points)?;
            pixel_resize(target.sdl,&mut queued)?;
            let mut keys=Vec::new();
            let started=Instant::now();
            chord(window.id(),69,&mut keys)?;
            let mut first=Vec::new();
            for event in keys {
                if event.0==window.id() && matches!(event.1,Event::Key{..}) {first.push(event);} else {queued.push(event);}
            }
            let resizes=queued.iter().filter(|(id,event)|*id==target.sdl && matches!(event,Event::Resize(size) if !size.contains(&0))).count();
            if resizes==0 {return Err("Rapid docking did not retain the real tagged nonzero SDL Resize".into());}
            self.pending=Some((started,"redock-before-resize",cycle));
            self.injected.push(json!({"age":age,"cycle":cycle,"action":"Move Right before queued Resize","ingress":"raw SDL key and SDL pixel-size event -> poll_tagged -> production route","resizeWindow":target.sdl,"requestedWindowPoints":points,"preservedResizeEvents":resizes,"prependedKeyEvents":first.len(),"prependedEvents":first.iter().map(|(id,event)|json!({"window":id,"event":format!("{event:?}")})).collect::<Vec<_>>(),"queuedEvents":queued.iter().map(|(id,event)|json!({"window":id,"event":format!("{event:?}")})).collect::<Vec<_>>()}));
            first.extend(queued);
            *events=first;
        }
        self.write(output,index,act,age)
    }

    pub fn restore(&mut self,editor:&mut Editor,shell:&mut Shell) -> ara_ui::Result<()> {
        if let Some((shortcuts,scale))=self.preferences.take() {
            editor.action("editor.settings.set",json!({"key":"input.shortcuts","value":shortcuts}))?;
            editor.action("editor.settings.set",json!({"key":"appearance.uiScale","value":scale}))?;
            shell.rebuild(editor)?;
            self.restored=true;
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn end(&mut self,act:Act,index:usize,age:u32,editor:&Editor,shell:&Shell,gpu:&mut NativeUi,floats:&windows::Floats,output:&Path) -> ara_ui::Result<()> {
        let Some((started,phase,cycle))=self.pending.take() else {return Ok(());};
        let wanted:BTreeSet<_>=editor.state.floating.iter().map(|float|float.id.as_str()).collect();
        let native:BTreeSet<_>=floats.windows.iter().map(|float|float.id.as_str()).collect();
        let retained:BTreeSet<_>=shell.floats.iter().map(|float|float.id.as_str()).collect();
        let floating=editor.state.floating.iter().any(|float|area(&float.root,act.panel).is_some());
        let right=in_right(&editor.state.layout,act.panel);
        let body=format!("body-{}",act.panel);
        let mounted=if phase=="float" {
            editor.state.floating.iter().find(|float|area(&float.root,act.panel).is_some())
                .and_then(|float|shell.surface(&float.id)).is_some_and(|surface|surface.ui.find(&body).is_some_and(|index|surface.ui.shown(index)))
        } else {shell.shown_node(&body).is_some()};
        let scene=shell.scene_host();
        let preserved=self.camera==Some(editor.state.camera) && self.selection==editor.state.selection && self.doc==editor.state.doc && self.revision==editor.host.store().revision();
        let stage=if phase=="float" {"floating"}else {"redocked"};
        let prefix=format!("dock-{index:02}-rapid-{cycle}-{stage}");
        gpu.select_target(0);
        let image=gpu.pixels()?;
        image.save(output.join(format!("{prefix}.png")))?;
        self.captures+=1;
        for float in &floats.windows {
            gpu.select_target(float.key);
            gpu.pixels()?.save(output.join(format!("{prefix}-{}.png",float.id)))?;
            self.captures+=1;
        }
        gpu.select_target(0);
        let elapsed=started.elapsed();
        let scene_registered=shell.ui.display.commands.iter().any(|draw|matches!(draw,ara_ui::paint::Draw::Image{image,..} if *image==overlay::viewport_image()));
        let ok=wanted==native && wanted==retained && wanted.len()==usize::from(phase=="float") && floating==(phase=="float") && (phase=="float" || right) && mounted && preserved && scene.is_some_and(|(host,rect)|host.is_none() && rect.w>16. && rect.h>16.) && scene_registered && !shell.ui.display.commands.is_empty() && elapsed<=LIMIT;
        if !ok {self.failures.push(format!("Rapid {stage} cycle{cycle} failed surface/state/latency invariants"));}
        self.observed.push(json!({"age":age,"cycle":cycle,"stage":stage,"ok":ok,"capturePrefix":prefix,"keyToCompletedCaptureMilliseconds":elapsed.as_secs_f64()*1000.,"latencyLimitMilliseconds":LIMIT.as_millis(),"wanted":wanted,"native":native,"retained":retained,"panelFloating":floating,"panelInRightColumn":right,"panelMountedAndVisible":mounted,"scene":scene.map(|(surface,viewport)|json!({"surface":surface,"viewport":viewport})),"sceneImageRegistered":scene_registered,"nativeCaptureSize":image.dimensions(),"camera":editor.state.camera,"selection":editor.state.selection,"document":editor.state.doc,"revision":editor.host.store().revision(),"authoringCameraSelectionPreserved":preserved,"uiCommands":shell.ui.display.commands.len()}));
        self.write(output,index,act,age)
    }
    pub fn ok(&self) -> bool {self.failures.is_empty() && self.observed.len()==CYCLES as usize*2 && self.restored && self.captures>=CYCLES as usize*2}
    pub fn report(&self) -> Value {json!({"cycles":CYCLES,"expectedNativeTransitions":CYCLES*2,"observedTransitions":self.observed.len(),"pendingRoute":self.pending.map(|(_,phase,cycle)|json!({"phase":phase,"cycle":cycle,"boundary":"native route through completed capture"})),"bound":"A completed capture over10s fails; permanently blocked UI routing requires the outer native-run timeout and leaves this pending artifact","events":self.injected,"observations":self.observed,"failures":self.failures,"captures":self.captures,"temporaryBindingsRestored":self.restored})}
    fn write(&self,output:&Path,index:usize,act:Act,age:u32) -> ara_ui::Result<()> {
        std::fs::create_dir_all(output)?;
        std::fs::write(output.join("rapid-docking-live.json"),ara_json::to_canonical_string(&json!({"schema":"ara.editor.rapid-docking/1","case":index,"action":format!("{act:?}"),"age":age,"report":self.report()})))?;
        Ok(())
    }
}
