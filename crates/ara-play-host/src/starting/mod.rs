//! Owned Play preparation. The host polls progress; it never joins live work.
#[cfg(test)]
mod tests;
mod worker;
mod restart;
use super::*;
pub(crate) use worker::Reporter;
use worker::Worker;

pub(super) struct Pending {
    pub id: String,
    scene: String,
    began: Instant,
    preparation: Preparation,
    cancelled: bool,
}
enum Preparation {
    Start { number: u32, request: PlayStartParams, workspace: ara_doc::Workspace, revision: u64 },
    Restart { world: World, keep_state: bool, internal: bool },
}
impl Pending {
    fn restarting(&self) -> bool { matches!(self.preparation,Preparation::Restart{..}) }
    fn internal_restart(&self) -> bool { matches!(self.preparation,Preparation::Restart{internal:true,..}) }
}
pub(super) struct Active {
    pub id: String,
    scene: String,
    restarting: bool,
    internal_restart: bool,
    worker: Worker<Input, Outcome>,
}
struct Input {
    service: PlayService,
    pending: Pending,
}
struct Outcome {
    service: PlayService,
    result: Result<Value, Vec<Diagnostic>>,
    catalog: Option<Result<Value, Vec<Diagnostic>>>,
}

pub(crate) fn report(
    progress: Option<&Reporter>,
    phase: &'static str,
    completed: usize,
    total: usize,
) -> Result<(), Vec<Diagnostic>> {
    progress.map_or(Ok(()), |progress| {
        progress
            .report(phase, completed, total)
            .map_err(|message| fail(codes::PLAY, message))
    })
}
pub(crate) fn check(progress: Option<&Reporter>) -> Result<(), &'static str> {
    progress.map_or(Ok(()), Reporter::check)
}

fn run(input: Input, progress: &Reporter) -> Outcome {
    let Input {
        mut service,
        pending,
    } = input;
    let restarting = pending.restarting();
    let mut result = match pending.preparation {
        Preparation::Start { number, request, workspace, revision } => {
            service.next = number - 1;
            service.start_owned(&workspace,revision,Some(&pending.scene),request,Some(progress))
        }
        Preparation::Restart { world, keep_state, .. } => {
            service.worlds.insert(pending.id.clone(),world);
            service.run_restart(&pending.id,keep_state,progress)
        }
    };
    let catalog = if result.is_ok() && progress.check().is_ok() {
        let _ = progress.report("catalog", 0, 1);
        let catalog = service.call_on_checked(&pending.id, method::COMMANDS, json!({}), Some(0), Some(progress))
            .and_then(|catalog|if catalog.is_object(){Ok(catalog)}else{Err(fail(codes::PLAY,"C# command refresh returned a non-object catalog"))});
        if let Ok(catalog) = &catalog
            && let Some(world) = service.worlds.get_mut(&pending.id)
        {
            world.commands = catalog.clone();
        }
        Some(catalog)
    } else {
        None
    };
    if progress.check().is_err() {
        // No surface was announced to the editor. Kill/reap cancelled candidates
        // here, never by dropping their processes on the UI thread.
        service.worlds.clear();
        result = Err(fail(codes::PLAY, "Play start cancelled"));
    } else if restarting && result.is_err() {
        // A failed snapshot/replacement owns any remaining old player here.
        // Never hand a stale world or its blocking destruction back to the UI.
        service.worlds.clear();
    } else if let Some(pool) = &mut service.pool {
        // Windows CreateProcess is measurable; refill before returning ownership.
        pool.refill();
    }
    Outcome {
        service,
        result,
        catalog,
    }
}

impl PlayService {
    pub(super) fn start_background(
        &mut self,
        cx: &Cx<'_>,
        request: PlayStartParams,
    ) -> Result<Value, Vec<Diagnostic>> {
        let scene = request
            .scene
            .as_deref()
            .or_else(|| cx.main_scene())
            .ok_or_else(|| fail(codes::REQUEST, "no scene given and no project main scene"))?
            .to_owned();
        self.next += 1;
        let id = format!("play:{}", self.next);
        self.starting.push_back(Pending {
            id: id.clone(),
            scene: scene.clone(),
            began: Instant::now(),
            preparation: Preparation::Start {
                number:self.next, request,workspace:cx.store().workspace().clone(),revision:cx.store().revision(),
            },
            cancelled: false,
        });
        self.begin_starting();
        Ok(
            json!({"world":id,"scene":scene,"status":"starting","players":[],"progress":{"phase":"queued","completed":0,"total":0}}),
        )
    }

    fn begin_starting(&mut self) {
        if self.starter.is_some() {
            return;
        }
        let Some(pending) = self.starting.pop_front() else {
            return;
        };
        let id = pending.id.clone();
        let scene = pending.scene.clone();
        let restarting = pending.restarting();
        // Spawn before moving any live resources. A spawn/send failure retains
        // their ownership, instead of synchronously destroying children/assets.
        let mut worker = match Worker::spawn(pending.began, run) {
            Ok(worker) => worker,
            Err(error) => {
                self.restore_restart(pending);
                // Restoring a queued restart keeps its live source players owned.
                // Fresh starts have no live player ownership to restore.
                self.start_failures.insert(id.clone(), json!({"world":id,"scene":scene,"restart":restarting,"status":"failed","error":error.to_string()}));
                return;
            }
        };
        if pending.cancelled {
            worker.cancel();
        }
        let internal_restart = pending.internal_restart();
        let mut service = Self::new(&self.root, self.config.clone());
        service.token = self.token.clone();
        service.pool = self.pool.take();
        service.assets = self.assets.take();
        service.pool_init = self.pool_init.take();
        service.assets_init = self.assets_init.take();
        service.pool_retry = self.pool_retry.take();
        service.assets_retry = self.assets_retry.take();
        service.graphics_prewarm = self.graphics_prewarm;
        if let Err(input) = worker.send(Input { service, pending }) {
            self.pool = input.service.pool;
            self.assets = input.service.assets;
            self.pool_init = input.service.pool_init;
            self.assets_init = input.service.assets_init;
            self.pool_retry = input.service.pool_retry;
            self.assets_retry = input.service.assets_retry;
            self.restore_restart(input.pending);
            self.start_failures.insert(id.clone(), json!({"world":id,"scene":scene,"restart":restarting,"status":"failed","error":"startup worker stopped before accepting work"}));
            return;
        }
        self.starter = Some(Active { id, scene, restarting, internal_restart, worker });
    }

    pub(super) fn poll_starting(&mut self, cx: &mut Cx<'_>) {
        if let Some(active) = &mut self.starter {
            active.worker.poll();
        }
        if self
            .starter
            .as_ref()
            .is_some_and(|active| active.worker.finished())
        {
            let active = self
                .starter
                .take()
                .expect("completed startup remains owned");
            let cancelled = active.worker.cancelled();
            match active.worker.finish() {
                Ok(Some(mut outcome)) => {
                    self.pool = outcome.service.pool.take();
                    self.assets = outcome.service.assets.take();
                    self.pool_init = outcome.service.pool_init.take();
                    self.assets_init = outcome.service.assets_init.take();
                    self.pool_retry = outcome.service.pool_retry.take();
                    self.assets_retry = outcome.service.assets_retry.take();
                    self.worlds.append(&mut outcome.service.worlds);
                    if cancelled && self.worlds.contains_key(&active.id) {
                        if let Err(problems) = self.stop(&active.id) {
                            for problem in problems {
                                cx.log(problem);
                            }
                        }
                    } else if !cancelled {
                        match outcome.result {
                            Ok(result) => {
                                let _ = self.with_notes(cx, Ok(result));
                                self.announce(&active.id);
                                if active.internal_restart {
                                    cx.log(Diagnostic::info(codes::PLAY,format!("{}: engine bindings changed; restarted from the current tick",active.id)));
                                }
                                if let Some(catalog) = outcome.catalog {
                                    self.catalog_refresh.ready(&active.id,&catalog);
                                    match catalog {
                                        Ok(catalog) => cx.publish_commands("play", &catalog),
                                        Err(problems) => {
                                            for problem in problems {
                                                cx.log(problem);
                                            }
                                        }
                                    }
                                }
                            }
                            Err(problems) => {
                                self.start_failures.insert(active.id.clone(), json!({"world":active.id,"scene":active.scene,"restart":active.restarting,"status":"failed","diagnostics":problems}));
                                for problem in problems {
                                    cx.log(problem);
                                }
                            }
                        }
                    }
                }
                _ => {
                    self.start_failures.insert(active.id.clone(), json!({"world":active.id,"scene":active.scene,"restart":active.restarting,"status":"failed","error":"Play preparation worker panicked"}));
                }
            }
        }
        self.begin_starting();
    }

    pub(super) fn cancel_start(&mut self, id: &str) -> Option<Result<Value,Vec<Diagnostic>>> {
        if let Some(active) = &self.starter
            && active.id == id
        {
            active.worker.cancel();
            return Some(Ok(json!({"world":id,"status":"stopping"})));
        }
        if let Some(result) = self.cancel_queued_restart(id) { return Some(result); }
        if let Some(pending) = self.starting.iter_mut().find(|pending| pending.id == id) {
            pending.cancelled = true;
            return Some(Ok(json!({"world":id,"status":"stopping"})));
        }
        if self.worlds.contains_key(id) { self.start_failures.remove(id); return None; }
        self.start_failures
            .remove(id)
            .map(|_| Ok(json!({"world":id,"status":"stopped"})))
    }

    pub(super) fn starting_status(&mut self) -> Vec<Value> {
        let mut rows = Vec::new();
        if let Some(active) = &mut self.starter {
            active.worker.poll();
            let p = active.worker.progress;
            rows.push(json!({"world":active.id,"scene":active.scene,"status":if active.worker.cancelled(){"stopping"}else{"starting"},"restart":active.restarting,"players":[],
                "progress":{"phase":p.phase,"label":if active.restarting {"Restarting Play…"} else {"Starting Play…"},"completed":p.completed,"total":p.total,"elapsedMs":active.worker.began.elapsed().as_secs_f64()*1000.}}));
        }
        rows.extend(self.starting.iter().map(|pending| json!({"world":pending.id,"scene":pending.scene,"status":if pending.cancelled{"stopping"}else{"starting"},"restart":pending.restarting(),"players":[],
            "progress":{"phase":if pending.restarting(){"restarting"}else{"queued"},"label":if pending.restarting(){"Restarting Play…"}else{"Starting Play…"},"completed":0,"total":0,"elapsedMs":pending.began.elapsed().as_secs_f64()*1000.}})));
        rows.extend(self.start_failures.iter().filter(|(id,_)|!self.worlds.contains_key(*id)).map(|(_,failure)|failure.clone()));
        rows
    }
}
