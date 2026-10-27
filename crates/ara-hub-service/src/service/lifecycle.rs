//! Project process lifecycle and per-client engine routes.
use super::*;

impl Hub {
    pub(super) fn status_of(&self, project: &ProjectEntry) -> Value {
        let running = self.processes.running.get(&project.id).filter(|p| !p.watch.exited()).map(|running| {
            json!({"state":"open","pid":running.info.pid,"port":running.info.port,"headless":running.headless})
        });
        let mut value = running
            .unwrap_or_else(|| json!({"state":"closed","pid":null,"port":null,"headless":null}));
        value["project"] = json!(project);
        value
    }

    pub(super) fn open_project(
        &mut self,
        project: &ProjectEntry,
        headless: bool,
    ) -> Result<Value, RpcError> {
        self.discover_process(project)?;
        let mut status = self.status_of(project);
        if status["state"] == "open" && (headless || status["headless"] == false) {
            return Ok(status);
        }
        let marker: Value = serde_json::from_slice(
            &fs::read(project.path.join("ara.project.json")).map_err(io_error)?,
        )
        .map_err(io_error)?;
        let version = marker["engineVersion"]
            .as_str()
            .ok_or_else(|| invalid("project is missing its engineVersion pin"))?;
        if self.engine_removal_running(version) {
            return Err(invalid(
                "this engine is being uninstalled; cancel or finish the uninstall before opening the project",
            ));
        }
        let engine = self.registry.engine(version)?.clone();
        let executable = if headless {
            engine.executable
        } else {
            engine
                .editor
                .ok_or_else(|| invalid(format!("engine `{version}` has no installed editor")))?
        };
        // The current native editor owns the authority. Flush and hand off from the headless process.
        if status["state"] == "open" {
            self.close_project(project)?;
        }
        let log_dir = project.path.join(".ara");
        fs::create_dir_all(&log_dir).map_err(io_error)?;
        let mut command = Command::new(executable);
        if headless {
            command.arg("--root").arg(&project.path).arg("serve");
            hidden(&mut command);
        } else {
            command.arg(&project.path);
        }
        let mut child = command
            .current_dir(&project.path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(File::create(log_dir.join("hub-process.log")).map_err(io_error)?)
            .spawn()
            .map_err(io_error)?;
        let until = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(exit) = child.try_wait().map_err(io_error)? {
                return Err(io_error(format!(
                    "project process exited with {exit}; see .ara/hub-process.log"
                )));
            }
            if let Err(error) = self.discover_process(project) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
            status = self.status_of(project);
            if status["state"] == "open" {
                break;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io_error(
                    "project did not become ready within 20 seconds; see .ara/hub-process.log",
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        self.children.insert(project.id.clone(), child);
        if let Some(entry) = self
            .registry
            .projects
            .iter_mut()
            .find(|p| p.id == project.id)
        {
            entry.last_opened = Some(now());
            entry.engine_version = version.to_owned();
            status["project"] = json!(entry);
        }
        self.registry.save(&self.config.directory)?;
        self.changed("opened", Some(&project.id));
        Ok(status)
    }

    pub(super) fn close_project(&mut self, project: &ProjectEntry) -> Result<Value, RpcError> {
        self.discover_process(project)?;
        self.routes.retain(|(_, id), _| id != &project.id);
        self.activity_clients.remove(&project.id);
        if let Some(running) = self.processes.running.get(&project.id) {
            if !running.watch.exited() {
                let stopped = (|| {
                    let mut client = Client::connect(running.info.port, &running.info.token)
                        .map_err(io_error)?;
                    let session = client.call(
                        "session.begin",
                        json!({"client":"ara-hub","author":"human","label":"Hub lifecycle"}),
                    )?;
                    client.call("daemon.shutdown", json!({"session":session["session"]}))?;
                    Ok::<_, RpcError>(())
                })();
                if let Err(error) = stopped
                    && !running.watch.exited()
                {
                    return Err(error);
                }
            }
            #[cfg(not(windows))]
            if let Some(child) = self.children.get_mut(&project.id) {
                let deadline = Instant::now() + Duration::from_secs(10);
                while child.try_wait().map_err(io_error)?.is_none() {
                    if Instant::now() >= deadline {
                        return Err(io_error(
                            "project process did not exit within 10 seconds after shutdown; see .ara/hub-process.log",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            if !running
                .watch
                .wait(Duration::from_secs(10))
                .map_err(io_error)?
            {
                return Err(io_error(format!(
                    "project process {} did not exit within 10 seconds after shutdown; see .ara/hub-process.log",
                    running.info.pid
                )));
            }
        }
        if let Some(mut child) = self.children.remove(&project.id) {
            let _ = child.wait();
        }
        self.processes.forget(&project.id);
        self.changed("closed", Some(&project.id));
        Ok(self.status_of(project))
    }

    pub(super) fn proxy(&mut self, conn: ConnId, value: Value) -> Result<Value, RpcError> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Call {
            project: String,
            method: String,
            params: Value,
        }
        let p: Call = parse(value)?;
        let project = self.registry.resolve(&p.project)?.clone();
        self.open_project(&project, true)?;
        let key = (conn, project.id.clone());
        if !self.routes.contains_key(&key) {
            let client = Client::for_project(&project.path)
                .ok_or_else(|| io_error("project daemon is unreachable"))?;
            self.routes.insert(key.clone(), client);
        }
        let client = self
            .routes
            .get_mut(&key)
            .ok_or_else(|| io_error("project route disappeared"))?;
        let mut result = client.call(&p.method, p.params);
        if p.method == "session.begin"
            && let Ok(begun) = &result
        {
            client.call(
                "collab.join",
                json!({"session":begun["session"],"events":["collab.conflict"]}),
            )?;
        }
        let notices: Vec<_> = client
            .notifications()
            .into_iter()
            .filter(|n| n["method"] == "collab.conflict")
            .map(|n| n["params"].clone())
            .collect();
        if !notices.is_empty()
            && let Ok(value) = &mut result
            && value.is_object()
        {
            value["notices"] = json!(notices);
        }
        if let Err(error) = &result
            && error
                .diagnostics()
                .iter()
                .any(|d| d.code == ara_diag::codes::DAEMON_UNREACHABLE)
        {
            self.routes.remove(&key);
        }
        self.changed("activity", Some(&project.id));
        result
    }
}
