//! The Hub view-model against a real user Hub service in a scratch config
//! directory: create from a template (with inline validation), open headless,
//! the status badge, close, remove from the list. Same model and connection the
//! window uses; only the window is absent.
#[path = "../../ara-hub-service/tests/support/mod.rs"]
mod support;
use ara_hub::{
    connection::Connection,
    model::{Hub, Intent, Prefs, State, View},
};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn scratch() -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/hub-ui-tests")
        .join(format!("{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).expect("scratch");
    dir.canonicalize().expect("canonical scratch")
}

struct Driver {
    hub: Hub,
    connection: Connection,
    config: PathBuf,
}
impl Driver {
    /// Apply intents, then pump calls and events until `done` holds and nothing is
    /// in flight.
    fn run(&mut self, what: &str, intents: Vec<Intent>, done: impl Fn(&Hub) -> bool) {
        for intent in intents {
            self.hub.intent(intent);
        }
        let until = Instant::now() + Duration::from_secs(90);
        loop {
            self.connection.send(self.hub.take_calls());
            if let Some(event) = self.connection.wait(Duration::from_millis(50)) {
                self.hub.event(event);
            }
            for event in self.connection.poll() {
                self.hub.event(event);
            }
            self.connection.send(self.hub.take_calls());
            if done(&self.hub) && self.hub.idle() {
                return;
            }
            assert!(
                Instant::now() < until,
                "{what}: timed out; link {:?}, notices {:#?}",
                self.hub.link,
                self.hub.notices
            );
        }
    }
}
impl Drop for Driver {
    fn drop(&mut self) {
        self.connection.stop();
        if let Err(error) = support::shutdown(&self.config) {
            if std::thread::panicking() {
                eprintln!("{error}");
            } else {
                panic!("{error}");
            }
        }
    }
}

#[test]
fn create_open_headless_close_and_remove_through_the_view_model() {
    let root = scratch();
    let config = root.join("config");
    let mut driver = Driver {
        hub: Hub::new(Prefs::default(), root.clone(), None),
        connection: Connection::start(config.clone(), PathBuf::from(env!("CARGO_BIN_EXE_ara"))),
        config,
    };
    driver.run("initial load", Vec::new(), Hub::ready);
    let hub = &driver.hub;
    assert!(hub.projects.is_empty(), "scratch registry starts empty");
    assert_eq!(hub.templates[0].id, "blank");
    assert_eq!(hub.engines.len(), 1, "the service registers its own build");

    // Samples from this checkout are templates; without their art, the service's
    // dry run rejects them with a field-tagged diagnostic.
    if let Some(sample) = driver
        .hub
        .templates
        .iter()
        .find(|t| t.id == "outpost" && t.setup.is_some())
        .cloned()
    {
        driver.run(
            "incomplete sample",
            vec![
                Intent::Navigate(View::Create),
                Intent::Template(sample.id.clone()),
            ],
            |hub| !hub.create.waiting(),
        );
        let problem = driver
            .hub
            .create
            .problems_for("template")
            .next()
            .expect("template problem");
        assert_eq!(problem.code, ara_diag::codes::TEMPLATE_INCOMPLETE);
        assert!(problem.message.contains("ara-samples fetch outpost"));
    }

    // Inline validation comes from the service's diagnostics.
    driver.run(
        "empty name",
        vec![
            Intent::Navigate(View::Create),
            Intent::Template("blank".into()),
            Intent::CreateDestination(root.display().to_string()),
            Intent::CreateName(String::new()),
        ],
        |hub| !hub.create.waiting(),
    );
    let name = driver
        .hub
        .create
        .problems_for("name")
        .next()
        .expect("name problem");
    assert_eq!(name.message, "Give the project a name");
    assert!(!driver.hub.create.can_submit());

    fs::create_dir(root.join("Taken")).expect("existing folder");
    driver.run(
        "existing folder",
        vec![Intent::CreateName("Taken".into())],
        |hub| !hub.create.waiting(),
    );
    let path = driver
        .hub
        .create
        .problems_for("path")
        .next()
        .expect("path problem");
    assert!(path.message.contains("already exists"), "{}", path.message);

    // Create from the blank template.
    driver.run(
        "dry run",
        vec![Intent::CreateName("Hub Test".into())],
        |hub| !hub.create.waiting(),
    );
    assert!(
        driver.hub.create.problems.is_empty(),
        "{:?}",
        driver.hub.create.problems
    );
    driver.run("create", vec![Intent::CreateSubmit], |hub| {
        hub.projects.iter().any(|p| p.name == "Hub Test")
    });
    assert_eq!(driver.hub.view, View::Projects);
    let project = driver
        .hub
        .projects
        .iter()
        .find(|p| p.name == "Hub Test")
        .cloned()
        .expect("created card");
    assert!(project.path.join("ara.project.json").is_file());
    assert_eq!(driver.hub.highlight.as_deref(), Some(project.id.as_str()));
    let id = project.id.clone();

    // Open headless: busy while starting, then the status badge from hub.status.
    driver.hub.intent(Intent::OpenHeadless(id.clone()));
    assert!(driver.hub.busy.contains_key(&id), "progress while starting");
    driver.run("open headless", Vec::new(), |hub| {
        hub.state(&id) == State::Headless
    });
    assert!(
        driver.hub.notices.iter().all(|n| !n.is_error()),
        "{:#?}",
        driver.hub.notices
    );
    let opened = driver.hub.project(&id).expect("card");
    assert!(opened.last_opened.is_some(), "open stamps last opened");

    // Close.
    driver.run("close", vec![Intent::Close(id.clone())], |hub| {
        hub.state(&id) == State::Closed
    });

    // Remove from the list keeps files and offers Undo.
    driver.run("remove", vec![Intent::Remove(id.clone())], |hub| {
        hub.project(&id).is_none()
    });
    assert!(
        project.path.join("ara.project.json").is_file(),
        "files kept"
    );
    let undo = driver
        .hub
        .notices
        .iter()
        .find(|n| n.undo.is_some())
        .map(|n| n.id)
        .expect("undo notice");
    driver.run(
        "undo remove",
        vec![Intent::Fix {
            notice: undo,
            fix: usize::MAX,
        }],
        |hub| hub.projects.iter().any(|p| p.name == "Hub Test"),
    );
}
