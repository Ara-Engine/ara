//! Owned Play preparation. The host polls progress; it never joins live work.
#[cfg(test)]
mod tests;
mod worker;
use super::*;
pub(crate) use worker::Reporter;
use worker::Worker;

pub(super) struct Pending {
    pub id: String,
    scene: String,
    number: u32,
    began: Instant,
    request: PlayStartParams,
    workspace: ara_doc::Workspace,
    revision: u64,
    cancelled: bool,
}
pub(super) struct Active {
    pub id: String,
    scene: String,
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
    service.next = pending.number - 1;
    let mut result = service.start_owned(
        &pending.workspace,
        pending.revision,
        Some(&pending.scene),
        pending.request,
        Some(progress),
    );
    let catalog = if result.is_ok() && progress.check().is_ok() {
        let _ = progress.report("catalog", 0, 1);
        let catalog = service.call(&pending.id, method::COMMANDS, json!({}), false);
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
            number: self.next,
            began: Instant::now(),
            request,
            workspace: cx.store().workspace().clone(),
            revision: cx.store().revision(),
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
        // Spawn before moving any live resources. A spawn/send failure retains
        // their ownership, instead of synchronously destroying children/assets.
        let mut worker = match Worker::spawn(pending.began, run) {
            Ok(worker) => worker,
            Err(error) => {
                self.start_failures.insert(pending.id.clone(), json!({"world":pending.id,"scene":pending.scene,"status":"failed","error":error.to_string()}));
                return;
            }
        };
        if pending.cancelled {
            worker.cancel();
        }
        let id = pending.id.clone();
        let scene = pending.scene.clone();
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
            self.start_failures.insert(id.clone(), json!({"world":id,"scene":scene,"status":"failed","error":"startup worker stopped before accepting work"}));
            return;
        }
        self.starter = Some(Active { id, scene, worker });
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
                                if let Some(catalog) = outcome.catalog {
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
                                self.start_failures.insert(active.id.clone(), json!({"world":active.id,"scene":active.scene,"status":"failed","diagnostics":problems}));
                                for problem in problems {
                                    cx.log(problem);
                                }
                            }
                        }
                    }
                }
                _ => {
                    self.start_failures.insert(active.id.clone(), json!({"world":active.id,"scene":active.scene,"status":"failed","error":"Play preparation worker panicked"}));
                }
            }
        }
        self.begin_starting();
    }

    pub(super) fn cancel_start(&mut self, id: &str) -> Option<Value> {
        if let Some(active) = &self.starter
            && active.id == id
        {
            active.worker.cancel();
            return Some(json!({"world":id,"status":"stopping"}));
        }
        if let Some(pending) = self.starting.iter_mut().find(|pending| pending.id == id) {
            pending.cancelled = true;
            return Some(json!({"world":id,"status":"stopping"}));
        }
        self.start_failures
            .remove(id)
            .map(|_| json!({"world":id,"status":"stopped"}))
    }

    pub(super) fn starting_status(&mut self) -> Vec<Value> {
        let mut rows = Vec::new();
        if let Some(active) = &mut self.starter {
            active.worker.poll();
            let p = active.worker.progress;
            rows.push(json!({"world":active.id,"scene":active.scene,"status":if active.worker.cancelled(){"stopping"}else{"starting"},"players":[],
                "progress":{"phase":p.phase,"completed":p.completed,"total":p.total,"elapsedMs":active.worker.began.elapsed().as_secs_f64()*1000.}}));
        }
        rows.extend(self.starting.iter().map(|pending| json!({"world":pending.id,"scene":pending.scene,"status":if pending.cancelled{"stopping"}else{"starting"},"players":[],
            "progress":{"phase":"queued","completed":0,"total":0,"elapsedMs":pending.began.elapsed().as_secs_f64()*1000.}})));
        rows.extend(self.start_failures.values().cloned());
        rows
    }
}
