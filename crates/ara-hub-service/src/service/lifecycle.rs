//! Project process lifecycle and per-client engine routes.
use super::*;

impl Hub {
    pub(super) fn status_of(&self, project: &ProjectEntry) -> Value {
        let running = DaemonInfo::read(&project.path).and_then(|info| {
            let mut client = Client::connect(info.port, &info.token).ok()?;
            let catalog = client.call("rpc.discover", json!({})).ok()?;
            let editor = catalog["methods"]
                .as_array()
                .is_some_and(|ms| ms.iter().any(|m| m["name"] == "editor.state"));
            Some(json!({"state":"open","pid":info.pid,"port":info.port,"headless":!editor}))
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
        self.routes.retain(|(_, id), _| id != &project.id);
        if let Some(mut client) = Client::for_project(&project.path) {
            let session = client.call(
                "session.begin",
                json!({"client":"ara-hub","author":"human","label":"Hub lifecycle"}),
            )?;
            client.call("daemon.shutdown", json!({"session":session["session"]}))?;
            let until = Instant::now() + Duration::from_secs(10);
            while Client::for_project(&project.path).is_some() {
                if Instant::now() >= until {
                    return Err(io_error("project did not stop within 10 seconds"));
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
        if let Some(mut child) = self.children.remove(&project.id) {
            let _ = child.wait();
        }
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
