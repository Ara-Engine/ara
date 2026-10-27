//! Agent command dispatch, independent of the daemon transport loop.
use super::*;
impl Lab {
    pub(super) fn dispatch(&mut self, request: Request) -> Result<Value> {
        let args = request.arguments;
        let name = args["session"].as_str().unwrap_or("default");
        match request.tool.as_str() {
            "lab_mirror" => {
                ensure!(!cfg!(target_os = "macos"), "configure the mirror on the PC");
                let result = super::super::mirror::configure(&self.root, &self.base, &args)?;
                if let Some(mirror) = &self.mirror {
                    mirror.notify();
                }
                Ok(result)
            }
            "_lab_import" => {
                ensure!(
                    cfg!(target_os = "macos"),
                    "cooked imports are accepted only by the Mac peer"
                );
                if session::read(&self.base.join("mirror/imported.json"))
                    .is_some_and(|v| v["directory"] == args["directory"])
                {
                    return Ok(json!({"imported":true,"unchanged":true}));
                }
                self.pending_import = Some(args);
                self.activity
                    .shaders_latest
                    .store(false, std::sync::atomic::Ordering::Release);
                Ok(json!({"queued":true,"cook_host":"pc"}))
            }
            "lab_status" => self.status(),
            "lab_claim" => {
                let agent = args["agent"].as_str().context("agent required")?;
                let paths: Vec<String> = serde_json::from_value(args["paths"].clone())?;
                let warnings = self.claims.claim(agent, &paths)?;
                self.persist()?;
                if let Some(mirror) = &self.mirror {
                    mirror.notify();
                }
                Ok(json!({"agent":agent,"warnings":warnings}))
            }
            "lab_release" => {
                self.claims
                    .0
                    .remove(args["agent"].as_str().context("agent required")?);
                self.persist()?;
                if let Some(mirror) = &self.mirror {
                    mirror.notify();
                }
                Ok(json!({"released":true}))
            }
            "lab_rebuild" => {
                self.rebuild = true;
                self.activity
                    .rust_latest
                    .store(false, std::sync::atomic::Ordering::Release);
                Ok(json!({"queued":true,"strategy":self.strategy.name()}))
            }
            "lab_reload" => {
                let result = self
                    .sessions
                    .get_mut(name)
                    .context("open a track with lab_session first")?
                    .control(json!({"reload_shaders":true}), Duration::from_secs(60))?;
                Ok(
                    json!({"reload":result,"cooks_pending":self.cooks,"cook_errors":self.shader_results.values().filter(|result|result["success"]==false).collect::<Vec<_>>()}),
                )
            }
            "lab_session" => {
                session::name(name)?;
                if args["close"] == true {
                    self.preparing.remove(name);
                    self.preparation_errors.remove(name);
                    self.sessions.remove(name);
                    self.persist()?;
                    return Ok(json!({"closed":name}));
                }
                ensure!(
                    self.sessions.contains_key(name)
                        || self.sessions.len()
                            + self
                                .preparing
                                .keys()
                                .filter(|n| !self.sessions.contains_key(*n))
                                .count()
                            < self.max_sessions as usize,
                    "resident player limit reached; stop or reuse a track"
                );
                ensure!(
                    !self.preparing.contains_key(name),
                    "this track already has a pending scene; inspect lab_status"
                );
                let pending = super::super::prepare::Pending::start(
                    &self.root,
                    &self.base,
                    &args,
                    &self.binaries,
                )?;
                self.preparation_errors.remove(name);
                self.preparing.insert(name.into(), pending);
                Ok(
                    json!({"queued":true,"session":name,"last_good_retained":self.sessions.contains_key(name)}),
                )
            }
            "lab_variant" => {
                let variant = args["name"].as_str().context("variant name required")?;
                session::name(variant)?;
                if args["remove"] == true {
                    ensure!(
                        !self
                            .sessions
                            .values()
                            .any(|s| s.active_variant() == variant),
                        "select baseline or close tracks using {variant} first"
                    );
                    self.variants.remove(&self.base, variant)?;
                    self.cooks.retain(|(name, _)| name != variant);
                    self.promotions.retain(|(name, _)| name != variant);
                    self.pending_variants.retain(|_, name| name != variant);
                    self.shader_results
                        .retain(|_, value| value["variant"] != variant);
                    if self
                        .cook
                        .as_ref()
                        .is_some_and(|cook| cook.variant == variant)
                    {
                        self.cook.take();
                    }
                    session::write(&self.base.join("shader-results.json"), &self.shader_results)?;
                    return Ok(
                        json!({"removed":variant,"cached_artifacts_retained":super::super::variants::profile(&self.base,variant)}),
                    );
                }
                if !args["recipe"].is_null() {
                    ensure!(
                        !cfg!(target_os = "macos"),
                        "register recipes through the PC endpoint; the PC is the only shader cook host"
                    );
                    ensure!(
                        args["promote"] != true,
                        "cook and verify a recipe before promoting it"
                    );
                    let packages = self.variants.register(
                        &self.base,
                        variant,
                        args["recipe"].clone(),
                        &self.watch.manifests(),
                        &self.root,
                    )?;
                    for manifest in packages {
                        self.variants
                            .queued(variant, manifest.strip_prefix(&self.root)?);
                        self.cooks.insert((variant.into(), manifest));
                    }
                    self.variants.save(&self.base)?;
                    if args["_cook_only"] != true {
                        self.pending_variants.insert(name.into(), variant.into());
                    }
                    return Ok(json!({"queued":true,"variant":variant,"session":name}));
                }
                self.variants.ready(variant)?;
                if args["promote"] == true {
                    ensure!(
                        !cfg!(target_os = "macos"),
                        "promote variants on the PC cook host"
                    );
                    let packages = self.variants.packages(variant)?;
                    for relative in &packages {
                        self.variants.queued(variant, relative);
                        let job = (variant.to_owned(), self.root.join(relative));
                        self.promotions.insert(job.clone());
                        self.cooks.insert(job);
                    }
                    return Ok(
                        json!({"promotion_queued":true,"variant":variant,"baseline_manifests":packages}),
                    );
                }
                self.sessions
                    .get_mut(name)
                    .context("unknown track")?
                    .control(json!({"lab_variant":variant}), Duration::from_secs(60))
            }
            "lab_logs" => Ok(
                json!({"events":self.events.iter().rev().take(args["lines"].as_u64().unwrap_or(50).min(1000) as usize).collect::<Vec<_>>()}),
            ),
            "lab_stop" => {
                self.stopping = true;
                Ok(json!({"stopping":true}))
            }
            _ => bail!("unknown lab tool {}", request.tool),
        }
    }
}

impl Lab {
    pub(super) fn publish_import(&mut self) -> Result<()> {
        if self.pending_import.is_none() {
            return Ok(());
        }
        let activity = std::sync::Arc::clone(&self.activity);
        let Ok(_publication) = activity.gpu.try_write() else {
            return Ok(());
        };
        let args = self.pending_import.take().context("pending import")?;
        super::super::mirror::bundle::import(&self.base, &args)?;
        self.variants = super::super::variants::Variants::load(&self.base);
        self.claims = serde_json::from_value(args["claims"].clone())?;
        for session in self.sessions.values_mut() {
            let name = session.active_variant();
            super::super::variants::publish_tree(
                &super::super::variants::profile(&self.base, &name),
                session.view(),
            )?;
            session.shaders_changed();
            session.control(
                json!({"reload_shaders":true,"measure":false}),
                Duration::from_secs(60),
            )?;
        }
        session::write(&self.base.join("mirror/imported.json"), &args)?;
        self.activity.shaders_latest.store(
            args["shaders_current"] != false,
            std::sync::atomic::Ordering::Release,
        );
        self.event("mirror_imported", json!({"directory":args["directory"]}));
        Ok(())
    }
}
