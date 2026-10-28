//! Persistent virtual desktop coordinates accompany every actual SDL pointer event.
use super::*;
use ara_ui::paint::Box2;
fn rect(shell: &Shell,id:&str)->Option<Box2> {
    let node=&shell.ui.nodes[shell.shown_node(id)?];
    let visible=node.rect.intersection(node.clip);
    (visible.w>4. && visible.h>=node.rect.h*0.8 && !node.state.disabled).then_some(visible)
}
impl ProbeCheck {
    fn motion(&mut self,shell:&mut Shell,window:&Window,scale:f32,point:[f32;2],events:&mut Vec<(u32,Event)>)->ara_ui::Result<()> {
        if !scale.is_finite() || scale<=0. || !point.iter().all(|v|v.is_finite()) { return Err("Invalid native probe pointer mapping".into()); }
        let bounds=window.rect()?;
        let desktop=[bounds[0] as f32+point[0]/scale,bounds[1] as f32+point[1]/scale];
        self.pointer=Some(desktop);
        shell.desktop_pointer=Some(desktop);
        native::motion(window,point,scale,events)?;
        if self.trace.len()<256 {self.trace.push(json!({"case":CASES.get(self.index),"phase":self.phase,"point":point,"desktop":desktop,"scale":scale,"window":window.id()}));}
        Ok(())
    }
    pub(in super::super) fn click(&mut self,shell:&mut Shell,id:&str,window:&Window,scale:f32,events:&mut Vec<(u32,Event)>)->ara_ui::Result<bool> {
        let Some(rect)=rect(shell,id) else {return Ok(false)};
        let point=[rect.x+rect.w*0.4,rect.y+rect.h*0.5];
        if shell.ui.hit_test(point[0],point[1]).is_none_or(|i|shell.ui.nodes[i].element.id!=id) {return Ok(false)}
        self.motion(shell,window,scale,point,events)?;
        native::button(window,true,events)?;
        native::button(window,false,events)?;
        Ok(true)
    }
    pub(in super::super) fn reveal(&mut self,shell:&mut Shell,id:&str,scroll:&str,window:&Window,scale:f32,events:&mut Vec<(u32,Event)>)->ara_ui::Result<bool> {
        if rect(shell,id).is_some() {return Ok(true)}
        let Some(container)=rect(shell,scroll) else {return Ok(false)};
        let direction=shell.ui.find(id).map_or(-2.,|i|if shell.ui.nodes[i].rect.y<container.y {2.}else{-2.});
        self.motion(shell,window,scale,[container.x+container.w*0.5,container.y+container.h*0.5],events)?;
        native::wheel(window,direction,events)?;
        Ok(false)
    }
    pub(in super::super) fn blur_scene(&mut self,shell:&mut Shell,window:&Window,scale:f32,events:&mut Vec<(u32,Event)>)->ara_ui::Result<()> {
        let rect=shell.viewport().ok_or("Native probe history requires visible Scene")?;
        let point=[rect.x+rect.w*0.95,rect.y+rect.h*0.95];
        if shell.ui.hit_test(point[0],point[1]).is_some() {return Err("Native probe Scene focus-release point is covered".into());}
        self.motion(shell,window,scale,point,events)?;
        native::button(window,true,events)?;
        native::button(window,false,events)
    }
    pub(in super::super) fn palette(&mut self,shell:&mut Shell,window:&Window,scale:f32,command:&str,events:&mut Vec<(u32,Event)>)->ara_ui::Result<()> {
        match self.phase {
            1=>{native::shortcut(window,14,events)?;self.phase=2;}
            2=>{
                if rect(shell,"palette-search").is_none() {return Ok(())}
                native::shortcut(window,4,events)?;
                native::text(window,command,events)?;
                self.phase=3;
            }
            _=>{
                let id=shell.command_bindings().into_iter().find(|(id,binding)|id.starts_with("command-") && binding==command && rect(shell,id).is_some()).map(|(id,_)|id);
                if let Some(id)=id && self.click(shell,&id,window,scale,events)? {self.phase=4;}
            }
        }
        Ok(())
    }
}
