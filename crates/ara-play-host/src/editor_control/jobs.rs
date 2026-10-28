use super::*;
enum Completion {
    Return,
    Keep(PlayKeepParams),
    Diff(Option<Vec<String>>),
}
pub(crate) struct Pending {
    world: String,
    player: usize,
    ticket: u64,
    session: String,
    began: Instant,
    completion: Completion,
}
impl PlayService {
    pub(crate) fn start_editor_job(
        &mut self,
        cx: &mut Cx<'_>,
        method: &str,
        params: Value,
    ) -> Result<Value, Vec<Diagnostic>> {
        if self.editor_jobs.len() >= 64 {
            return Err(fail(
                codes::REQUEST,
                "Too many editor Play jobs are pending",
            ));
        }
        let mut completion = match method {
            "play.inspect" => {
                let _: PlayInspectParams = parse(params.clone())?;
                Completion::Return
            }
            "play.read" => {
                let _: PlayReadParams = parse(params.clone())?;
                Completion::Return
            }
            "play.patch" => {
                let _: PlayPatchParams = parse(params.clone())?;
                Completion::Return
            }
            "play.view" => {
                let _: PlayViewParams = parse(params.clone())?;
                Completion::Return
            }
            "play.control" => {
                let _: PlayControlParams = parse(params.clone())?;
                Completion::Return
            }
            "play.keep" => Completion::Keep(parse::<PlayKeepParams>(params.clone())?),
            "play.diff" => Completion::Diff(parse::<PlayDiffParams>(params.clone())?.entities),
            _ => {
                return Err(fail(
                    codes::REQUEST,
                    "This Play operation does not support deferred completion",
                ));
            }
        };
        let world = params["world"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| self.latest())
            .ok_or_else(|| fail(codes::UNKNOWN_WORLD, "No Play world is running"))?;
        if let Completion::Keep(params) = &mut completion {
            params.world = Some(world.clone());
        }
        let requested_player = params["player"].as_u64().unwrap_or(0) as usize;
        let player = if matches!(method, "play.keep" | "play.patch" | "play.control") {
            0
        } else {
            requested_player
        };
        let session = cx
            .session()
            .map(|session| session.id.clone())
            .ok_or_else(|| fail(codes::REQUEST, "Editor Play jobs require a live session"))?;
        let mut forward = if matches!(method, "play.keep" | "play.diff") {
            json!({})
        } else {
            params
        };
        if method == "play.control" {
            let role = self
                .world(&world)?
                .players
                .get(requested_player)
                .map(|(role, _)| *role)
                .ok_or_else(|| {
                    fail(
                        codes::REQUEST,
                        "The requested control player is not running",
                    )
                })?;
            let owner = match role {
                Role::Local | Role::Host => 1,
                Role::Client => requested_player as u64 + 1,
                Role::Dedicated => {
                    return Err(fail(
                        codes::REQUEST,
                        "The dedicated authority has no player controls; choose a rendering client",
                    ));
                }
            };
            forward["owner"] = json!(owner);
        }
        if let Some(map) = forward.as_object_mut() {
            for key in ["session", "world", "player", "background"] {
                map.remove(key);
            }
        }
        let (_, target) = self
            .world(&world)?
            .players
            .get_mut(player)
            .ok_or_else(|| fail(codes::REQUEST, "The requested Play player is not running"))?;
        let ticket = target
            .request(
                if method == "play.keep" {
                    method::DIFF
                } else {
                    method
                },
                &forward,
            )
            .map_err(|problem| {
                fail(
                    codes::PLAY,
                    problem["message"]
                        .as_str()
                        .unwrap_or("Cannot queue the Play request"),
                )
            })?;
        let job = cx.start_job(method);
        cx.job_progress(&job, 0., "Waiting for the requested player boundary");
        self.editor_jobs.insert(
            job.clone(),
            Pending {
                world,
                player,
                ticket,
                session,
                began: Instant::now(),
                completion,
            },
        );
        Ok(json!({"job":job,"status":"running"}))
    }
    /// Player::check drains bounded link messages first. This method only inspects
    /// reply slots and performs a completed Keep's one typed batch; it never waits.
    pub(crate) fn poll_editor_jobs(&mut self, cx: &mut Cx<'_>) {
        for (job, pending) in std::mem::take(&mut self.editor_jobs) {
            let cancelled = cx.job_cancelled(&job);
            let timed_out = pending.began.elapsed() > Duration::from_secs(120);
            let world = self.worlds.get_mut(&pending.world);
            let Some(world) = world else {
                cx.finish_job(
                    &job,
                    Err(fail(
                        codes::UNKNOWN_WORLD,
                        "Play stopped before this editor operation completed",
                    )),
                );
                continue;
            };
            let Some((_, player)) = world.players.get_mut(pending.player) else {
                cx.finish_job(
                    &job,
                    Err(fail(
                        codes::REQUEST,
                        "The requested Play player was removed",
                    )),
                );
                continue;
            };
            if cancelled || timed_out || world.fault.is_some() {
                player.forget_request(pending.ticket);
                if !cancelled {
                    cx.finish_job(
                        &job,
                        Err(fail(
                            if world.fault.is_some() {
                                codes::PLAYER_FAULT
                            } else {
                                codes::PLAY
                            },
                            if world.fault.is_some() {
                                "The Play player faulted before the editor operation completed"
                            } else {
                                "The editor Play operation exceeded its response deadline"
                            },
                        )),
                    );
                }
                continue;
            }
            let Some(reply) = player.response(pending.ticket) else {
                self.editor_jobs.insert(job, pending);
                continue;
            };
            let result = match reply {
                Err(problem) => Err(fail(
                    codes::PLAY,
                    problem["message"]
                        .as_str()
                        .unwrap_or("The player rejected the editor operation"),
                )),
                Ok(value) => match pending.completion {
                    Completion::Return => Ok(value),
                    Completion::Diff(entities) => {
                        self.diff_for_keep(cx, &pending.world, entities.as_deref(), value)
                    }
                    Completion::Keep(params) => {
                        cx.job_progress(
                            &job,
                            0.9,
                            "Keeping the runtime diff in one scene transaction",
                        );
                        match cx.resume_session(&pending.session, |cx| {
                            self.keep_from_diff(cx, params, value)
                        }) {
                            Ok(result) => result,
                            Err(diagnostics) => Err(diagnostics),
                        }
                    }
                },
            };
            cx.finish_job(&job, result);
        }
    }
}
