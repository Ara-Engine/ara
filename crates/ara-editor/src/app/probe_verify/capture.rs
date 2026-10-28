//! Observe real Host/renderer generations; no capture backend call stands in for UI.
use super::*;
impl ProbeCheck {
    pub(super) fn query(&mut self, editor:&mut Editor, shell:&Shell)->ara_ui::Result<()> {
        if Instant::now()<self.next_query {return Ok(())}
        self.next_query=Instant::now()+QUERY_CADENCE;
        let response=editor.action("render.probes.query",json!({"doc":SCENE,"entities":self.probe.iter().collect::<Vec<_>>()}))?;
        let snapshot:ProbeSnapshot=serde_json::from_value(response)?;
        if snapshot.probes.iter().any(|row|row.doc!=SCENE) {return Err("Probe query returned another Scene's rows".into());}
        self.snapshot=Some(snapshot);
        let report=shell.probe_report();
        if self.phase>0 && self.job.is_none() && (3..=5).contains(&self.index) {
            let jobs=report["jobs"].as_array().ok_or("Shell probe report has no owned job inventory")?;
            let fresh:Vec<_>=jobs.iter().filter(|row|row["doc"]==SCENE)
                .filter(|row|row["entities"].as_array().is_some_and(|entities|self.probe.as_ref().is_some_and(|entity|entities.iter().any(|value|value==entity))))
                .filter_map(|row|row["job"].as_str()).filter(|job|!self.job_baseline.contains(*job)).collect();
            if fresh.len()>1 {return Err("One native Recapture gesture started multiple Host jobs".into());}
            self.job=fresh.first().map(|job|(*job).to_owned());
        }
        if let Some(job)=&self.job {
            self.job_status=editor.action("jobs.status",json!({"job":job}))?;
        }
        let row=self.row().map(serde_json::to_value).transpose()?.unwrap_or(Value::Null);
        let sample=json!({"generation":row["generation"],"state":row["state"],"stage":row["stage"],
            "sourceHash":row["sourceHash"],"retainedPrevious":row["retainedPrevious"],"stale":row["stale"],
            "job":self.job,"jobStatus":self.job_status["status"],"cancellable":self.job_status["cancellable"]});
        let samples=self.observations.as_object_mut().ok_or("Probe observation object missing")?
            .entry("stateTransitions").or_insert_with(||json!([])).as_array_mut().ok_or("Probe transition list missing")?;
        if samples.last()!=Some(&sample) && samples.len()<64 {samples.push(sample);}
        Ok(())
    }
    pub(super) fn capture_input(&mut self,editor:&mut Editor,shell:&mut Shell,window:&Window,scale:f32,events:&mut Vec<(u32,Event)>)->ara_ui::Result<()> {
        if self.index==5 && self.phase==1 {
            if !self.reveal(shell,"ReflectionProbe-blendDistance","inspector-scroll",window,scale,events)? {return Ok(())}
            self.phase=2;
        }
        let request_phase=if self.index==5 {2}else{1};
        if self.phase==request_phase {
            if !self.reveal(shell,"lighting-probe-recapture","lighting-scroll",window,scale,events)? {return Ok(())}
            if self.click(shell,"lighting-probe-recapture",window,scale,events)? {
                self.phase=request_phase+1;
                self.next_query=Instant::now();
            }
            return Ok(());
        }
        if self.job.is_none() {return Ok(())}
        if self.index==3 {return self.completed_capture(editor)}
        let previous=self.previous_receipt["generation"].as_u64().ok_or("Cancel/stale proof has no prior actual receipt")?;
        let active=self.row().filter(|row|row.generation>previous && matches!(row.state,
            ara_runtime_render::probes::ProbeState::Queued|ara_runtime_render::probes::ProbeState::Capturing|ara_runtime_render::probes::ProbeState::Progress))
            .map(|row|serde_json::to_value(row).map(|value|(row.generation,value))).transpose()?;
        if self.active_generation.is_none() {
            if let Some((generation,row))=active {
                self.active_generation=Some(generation);
                self.observations["beforeInterruption"]=row;
            } else {
                if self.job_status["status"]!="running" && !self.job_status.is_null() {
                    return Err("Capture finished before an actual queued/active generation could be interrupted".into());
                }
                return Ok(());
            }
        }
        let interrupt_phase=request_phase+1;
        if self.phase==interrupt_phase {
            if self.job_status["status"]!="running" {
                return Err("Capture completed before the native interruption gesture reached its control".into());
            }
            if self.index==4 {
                let entity=self.probe.as_deref().ok_or("Cancel probe identity missing")?;
                let id=format!("lighting-probe-{:016x}-cancel",ara_ui::source::stable_id(&format!("{SCENE}#{entity}")));
                if !self.reveal(shell,&id,"lighting-scroll",window,scale,events)? {return Ok(())}
                if self.click(shell,&id,window,scale,events)? {self.phase+=1;self.next_query=Instant::now();}
            } else {
                let id="ReflectionProbe-blendDistance";
                if !self.reveal(shell,id,"inspector-scroll",window,scale,events)? {return Ok(())}
                if self.click(shell,id,window,scale,events)? {
                    native::shortcut(window,4,events)?;
                    native::text(window,"2 m",events)?;
                    native::tap(window,40,events)?;
                    self.phase+=1;
                    self.next_query=Instant::now();
                }
            }
            return Ok(());
        }
        self.interrupted_capture(editor)
    }
    fn completed_capture(&mut self,editor:&Editor)->ara_ui::Result<()> {
        match self.job_status["status"].as_str() {
            Some("running")|None=>return Ok(()),
            Some("done")=>{},
            _=>return Err(format!("Native recapture did not complete: {}",self.job_status).into()),
        }
        let Some(row)=self.row() else{return Ok(())};
        let Some(receipt)=&row.last_receipt else{return Ok(())};
        let receipt_json=serde_json::to_value(receipt)?;
        if !self.job_status["result"]["receipts"].as_array().is_some_and(|receipts|receipts.iter().any(|item|item==&receipt_json)) {
            return Err("Renderer receipt was not the native gesture's Host job receipt".into());
        }
        let cache=proof::published_receipt(editor.host.root(),row,receipt)?;
        proof::unchanged(&self.before,&fixture::snapshot(editor)?)?;
        self.complete=Some(json!({"nativeRecapture":true,"job":self.job_status,"row":row,"cache":cache,
            "captureDoesNotAuthor":true,"cacheHashScope":"CaptureHash identifies filtered capture data; file byte length/path are observed separately"}));
        Ok(())
    }
    fn interrupted_capture(&mut self,editor:&Editor)->ara_ui::Result<()> {
        let Some(row)=self.row() else{return Ok(())};
        if matches!(row.state,ara_runtime_render::probes::ProbeState::Queued|ara_runtime_render::probes::ProbeState::Capturing|ara_runtime_render::probes::ProbeState::Progress) {return Ok(())}
        let status=self.job_status["status"].as_str().unwrap_or("running");
        if status=="running" {return Ok(())}
        let retained=row.last_receipt.as_ref().map(serde_json::to_value).transpose()?.unwrap_or(Value::Null);
        if retained!=self.previous_receipt {return Err("Interrupted generation replaced or lost the previous published receipt".into());}
        let generation=self.active_generation.ok_or("Interrupted generation was not actually observed")?;
        if row.generation<generation {return Err("Renderer generation moved backwards during interruption".into());}
        if self.index==4 {
            if status!="cancelled" || row.state!=ara_runtime_render::probes::ProbeState::Cancelled {
                return Err(format!("Native Cancel did not retire the renderer generation: {} {:?}",status,row.state).into());
            }
            proof::unchanged(&self.before,&fixture::snapshot(editor)?)?;
        } else {
            if status!="failed" || row.state!=ara_runtime_render::probes::ProbeState::Failed {
                return Err("Edited-source capture did not explicitly reject its stale generation".into());
            }
            let prior=self.previous_receipt["sourceHash"].as_str().ok_or("Previous source hash missing")?;
            if row.source_hash.is_empty() || row.source_hash==prior || !row.stale {
                return Ok(());
            }
            let now=fixture::snapshot(editor)?;
            proof::one_transaction(&self.before,&now)?;
            if proof::probe(editor,self.probe.as_deref().ok_or("Probe missing")?)?["blendDistance"].as_f64()!=Some(2.) {
                return Err("Native source-edit field did not author the requested2m change".into());
            }
            if self.job_status["diagnostics"].as_array().is_none_or(|items|items.is_empty()) {
                return Err("Stale generation rejection has no authority diagnostic".into());
            }
        }
        self.complete=Some(json!({"interruptedGeneration":generation,"observedActiveBeforeGesture":true,
            "previousReceipt":self.previous_receipt,"finalRow":row,"job":self.job_status,
            "rendererRetirementObserved":true,"sourceEditIngress":if self.index==5 {json!("SDL Inspector field + native Enter")}else{Value::Null}}));
        Ok(())
    }
}
