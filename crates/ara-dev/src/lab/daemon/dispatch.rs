//! Agent command dispatch, independent of the daemon transport loop.
use super::*;
impl Lab {
    pub(super) fn dispatch(&mut self, request: Request) -> Result<Value> {
        let args = request.arguments;
        let name = args["session"].as_str().unwrap_or("default");
        match request.tool.as_str() {
            "lab_status" => self.status(),
            "lab_claim" => {
                let agent = args["agent"].as_str().context("agent required")?;
                let paths: Vec<String> = serde_json::from_value(args["paths"].clone())?;
                let warnings = self.claims.claim(agent, &paths)?;
                self.persist()?;
                Ok(json!({"agent":agent,"warnings":warnings}))
            }
            "lab_release" => {
                self.claims
                    .0
                    .remove(args["agent"].as_str().context("agent required")?);
                self.persist()?;
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
                    self.sessions.remove(name);
                    self.persist()?;
                    return Ok(json!({"closed":name}));
                }
                ensure!(
                    self.sessions.contains_key(name)
                        || self.sessions.len() < self.max_sessions as usize,
                    "resident player limit reached; stop or reuse a track"
                );
                vram()?;
                let selection =
                    super::super::scenes::select(&self.root, &self.base, &args, &self.binaries)?;
                let player = Session::launch(
                    &self.root,
                    &self.base,
                    super::super::session::Session::executable(&self.binaries)?,
                    name,
                    selection,
                    std::sync::Arc::clone(&self.activity),
                )?;
                self.sessions.insert(name.into(), player);
                self.persist()?;
                self.sessions.get_mut(name).context("new session")?.status()
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
                    self.pending_variants.insert(name.into(), variant.into());
                    return Ok(json!({"queued":true,"variant":variant,"session":name}));
                }
                self.variants.ready(variant)?;
                if args["promote"] == true {
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
