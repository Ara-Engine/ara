//! Private cook lanes and transactional publication to baseline/variant views.
use super::*;
impl Lab {
    pub(super) fn tick_shaders(&mut self) -> Result<()> {
        let activity = std::sync::Arc::clone(&self.activity);
        let publication = activity.gpu.try_write();
        if publication.is_ok()
            && let Some(cook) = &mut self.cook
            && let Some(status) = cook.child.try_wait()?
        {
            let cook = self.cook.take().context("completed cook")?;
            let relative = cook.manifest.strip_prefix(&self.root)?;
            let error = if status.success() {
                None
            } else {
                fs::read_to_string(&cook.log).ok()
            };
            let mut owners = self
                .claims
                .owners(relative)
                .into_iter()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>();
            if let Some(error) = &error {
                let error = error.replace('\\', "/").replace("//", "/");
                for (owner, paths) in &self.claims.0 {
                    if paths
                        .iter()
                        .any(|path| error.contains(&path.to_string_lossy().replace('\\', "/")))
                    {
                        owners.insert(owner.clone());
                    }
                }
            }
            let value = json!({"variant":cook.variant,"success":status.success(),"seconds":cook.started.elapsed().as_secs_f64(),"owners":owners,"log":cook.log,"error":error});
            self.shader_results
                .insert(PathBuf::from(&cook.variant).join(relative), value.clone());
            session::write(&self.base.join("shader-results.json"), &self.shader_results)?;
            self.event("shader_cook", value);
            let changed = fs::read_to_string(cook.output.join(".ara-shader-reload"))
                .unwrap_or_default()
                != cook.previous;
            let baseline_output =
                super::super::watch::output(&self.root, &self.base, &cook.manifest)?;
            let relative_output = baseline_output.strip_prefix(self.base.join("shaders"))?;
            let mut published = None;
            if cook.variant == "baseline" && status.success() && changed {
                super::super::variants::publish_tree(&cook.output, &baseline_output)?;
                self.variants
                    .baseline_changed(&self.base, relative, relative_output)?;
                published = Some("baseline".to_owned());
            } else if cook.variant != "baseline"
                && self
                    .variants
                    .completed(&self.base, &cook.variant, relative, status.success())?
            {
                self.variants
                    .publish(&self.root, &self.base, &cook.variant)?;
                published = Some(cook.variant.clone());
            }
            if let Some(published) = published {
                for session in self.sessions.values_mut() {
                    let name = session.active_variant();
                    if name == published
                        || (published == "baseline"
                            && !self.variants.affected(relative).contains(&name))
                    {
                        super::super::variants::publish_tree(
                            &super::super::variants::profile(&self.base, &name),
                            session.view(),
                        )?;
                        session.shaders_changed();
                        session.control(
                            json!({"reload_shaders":true,"measure":false}),
                            Duration::from_secs(30),
                        )?;
                    }
                }
                let selected = self
                    .pending_variants
                    .iter()
                    .filter(|(_, name)| **name == published)
                    .map(|(track, _)| track.clone())
                    .collect::<Vec<_>>();
                for track in selected {
                    if let Some(session) = self.sessions.get_mut(&track) {
                        session
                            .control(json!({"lab_variant":published}), Duration::from_secs(60))?;
                    }
                    self.pending_variants.remove(&track);
                }
            }
        }
        drop(publication);
        let admission = activity.gpu.try_read();
        if self.cook.is_none()
            && admission.is_ok()
            && let Some((variant, manifest)) = self.cooks.pop_first()
        {
            if let Some(executable) = self.binaries.get("ara-shader-cook") {
                let cache = super::super::variants::cache(&self.base, &variant);
                let output = super::super::watch::output(&self.root, &cache, &manifest)?;
                if !output.exists() {
                    let baseline = super::super::watch::output(&self.root, &self.base, &manifest)?;
                    if baseline.is_dir() {
                        super::super::variants::publish_tree(&baseline, &output)?;
                    }
                }
                fs::create_dir_all(&output)?;
                let log = self
                    .base
                    .join("cooks")
                    .join(format!("{}.log", session::id()));
                fs::create_dir_all(log.parent().context("cook log parent")?)?;
                let mut command = Command::new(executable);
                command
                    .current_dir(&self.root)
                    .arg(&manifest)
                    .arg(&output)
                    .stdout(File::options().create(true).append(true).open(&log)?)
                    .stderr(File::options().append(true).open(&log)?);
                if variant != "baseline" {
                    command.arg("--variant").arg(self.variants.recipe_path(
                        &self.base,
                        &variant,
                        manifest.strip_prefix(&self.root)?,
                    )?);
                }
                if self.promotions.remove(&(variant.clone(), manifest.clone())) {
                    command.arg("--promote");
                }
                super::super::build::background(&mut command);
                self.cook = Some(Cook {
                    child: crate::process::Tree::spawn(&mut command)?,
                    manifest,
                    log,
                    started: Instant::now(),
                    previous: fs::read_to_string(output.join(".ara-shader-reload"))
                        .unwrap_or_default(),
                    output,
                    variant,
                });
                self.activity
                    .compiling
                    .store(true, std::sync::atomic::Ordering::Release);
            } else {
                self.cooks.insert((variant, manifest));
            }
        }
        self.activity.shaders_latest.store(
            !self.cook.as_ref().is_some_and(|c| c.variant == "baseline")
                && !self.cooks.iter().any(|(name, _)| name == "baseline")
                && !self
                    .shader_results
                    .values()
                    .any(|v| v["variant"] == "baseline" && v["success"] == false),
            std::sync::atomic::Ordering::Release,
        );
        Ok(())
    }
}
