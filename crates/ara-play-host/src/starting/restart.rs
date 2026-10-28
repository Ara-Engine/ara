//! Restart owns the old players on the existing preparation worker, never the UI.
use super::*;

impl PlayService {
    pub(crate) fn restart_background(&mut self, id: &str, keep_state: bool, internal: bool) -> Result<Value,Vec<Diagnostic>> {
        if let Some(active)=&self.starter
            && active.id == id
        {
            return Ok(json!({"world":id,"status":if active.worker.cancelled(){"stopping"}else{"starting"},
                "restart":active.restarting,"coalesced":true,"players":[]}));
        }
        if let Some(pending)=self.starting.iter().find(|pending|pending.id==id) {
            return Ok(json!({"world":id,"status":if pending.cancelled{"stopping"}else{"starting"},
                "restart":pending.restarting(),"coalesced":true,"players":[]}));
        }
        let mut world=self.worlds.remove(id).ok_or_else(||fail(codes::UNKNOWN_WORLD,format!("no play world `{id}`")))?;
        self.catalog_refresh.invalidate(id,&mut world);
        let scene=world.scene.clone();
        self.start_failures.remove(id);
        self.starting.push_back(Pending {id:id.into(),scene:scene.clone(),began:Instant::now(),
            preparation:Preparation::Restart {world,keep_state,internal},cancelled:false});
        self.close(id);
        self.begin_starting();
        Ok(json!({"world":id,"scene":scene,"status":"starting","restart":true,"players":[],
            "progress":{"phase":"restarting","label":"Restarting Play…","completed":0,"total":1}}))
    }

    pub(super) fn run_restart(&mut self, id: &str, keep_state: bool, progress: &Reporter) -> Result<Value,Vec<Diagnostic>> {
        report(Some(progress),"restarting",0,1)?;
        let resume=if keep_state {
            report(Some(progress),"snapshot",0,1)?;
            let snapshot=self.call_on_checked(id,method::SNAPSHOT,json!({}),Some(0),Some(progress))?;
            Some(snapshot["snapshot"].as_str().ok_or_else(||fail(codes::PLAY,"player returned no snapshot"))?.to_owned())
        } else {None};
        self.restart_checked(id,resume.as_deref(),Some(progress))
    }

    pub(super) fn restore_restart(&mut self, pending: Pending) {
        if let Preparation::Restart {world,..}=pending.preparation {
            self.worlds.insert(pending.id.clone(),world);
            self.announce(&pending.id);
        }
    }

    pub(super) fn cancel_queued_restart(&mut self, id: &str) -> Option<Result<Value,Vec<Diagnostic>>> {
        let index=self.starting.iter().position(|pending|pending.id==id && pending.restarting())?;
        let (task,sender)=match lifecycle::Retiring::new(id) {Ok(task)=>task,Err(problems)=>return Some(Err(problems))};
        let pending=self.starting.remove(index).expect("located queued restart remains owned");
        let Preparation::Restart {world,keep_state,internal}=pending.preparation else {unreachable!("only restart work is removed")};
        if let Err(error)=sender.send(world) {
            self.starting.insert(index,Pending {preparation:Preparation::Restart {world:error.0,keep_state,internal},..pending});
            return Some(Err(fail(codes::PLAY,"shutdown worker stopped before accepting the queued restart")));
        }
        self.retiring.push(task);
        Some(Ok(json!({"world":id,"status":"stopping"})))
    }
}

#[cfg(test)]
mod tests;
