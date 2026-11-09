//! `ara-hub --capture DIR`: every view, dark and light, rendered headlessly through
//! stillkit's software target (the same drawing a window shows; no window opens)
//! and saved as PNG. The data is a fixed fixture fed through the model's own
//! reply handling (no service), so captures are reproducible; the service path is
//! covered by `ara/tests/hub_ui.rs`.
mod engine_layout;
mod flows;
mod interactions;
mod migration;
use crate::{
    connection::Event,
    model::{AgentClient, AgentSetup, Call, Hub, Intent, Layout, View},
    ui::HubApp,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use stillkit::el::Runtime;

/// Fixed clock for relative times in captures (2026-10-23 12:00 UTC).
pub const NOW: u64 = 1_792_756_800_000;
const HOUR: u64 = 3_600_000;

/// The checkout this binary was built in, for real sample previews.
fn checkout() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .ancestors()
        .find(|dir| dir.join("samples/assets.lock.json").is_file())
        .map(Path::to_owned)
}

/// Fill `hub` as if a service had answered: five projects (one open in the
/// editor, one headless), templates with a sample needing art, two builds,
/// an agent at work, and both install snippets.
pub fn seed(hub: &mut Hub) {
    seed_with(hub, true);
}

/// A first run: the installed build and templates, no projects yet.
pub fn seed_first_run(hub: &mut Hub) {
    seed_with(hub, false);
}

/// A read-only migration decision for native tests and offscreen captures.
pub fn seed_migration(hub: &mut Hub) {
    migration::seed_dialog(hub);
}

/// The current-engine choice after a content-layout incompatibility.
pub fn seed_engine_layout(hub: &mut Hub, available: bool) {
    engine_layout::seed_dialog(hub, available);
}

fn seed_with(hub: &mut Hub, with_projects: bool) {
    let root = checkout();
    let evidence = |sample: &str, file: &str| {
        root.as_ref()
            .map(|r| {
                let migrated = r.join("evidence/samples").join(sample).join(file);
                if migrated.is_file() {
                    migrated
                } else {
                    r.join("samples").join(sample).join("evidence").join(file)
                }
            })
            .filter(|p| p.is_file())
    };
    let base = r"C:\Users\tester\Ara Projects";
    let project = |id: &str,
                   name: &str,
                   folder: &str,
                   opened: Option<u64>,
                   thumb: Option<PathBuf>,
                   engine: &str| {
        json!({"id":id,"name":name,"path":format!(r"{base}\{folder}"),"lastOpened":opened,"thumbnail":thumb,"engineVersion":engine})
    };
    let projects = vec![
        project(
            "project-1",
            "Outpost",
            "Outpost",
            Some(NOW - 2 * HOUR),
            evidence("outpost", "outpost-vista.png"),
            "0.0.1-alpha",
        ),
        project(
            "project-2",
            "Material showcase",
            "Showcase",
            Some(NOW - 26 * HOUR),
            evidence("showcase", "showcase-stage.png"),
            "0.0.1-alpha",
        ),
        project(
            "project-3",
            "Courier Run",
            "Courier Run",
            Some(NOW - 5 * 24 * HOUR),
            None,
            "0.0.1-alpha",
        ),
        project(
            "project-4",
            "Night Market",
            "Night Market",
            Some(NOW - 50 * 24 * HOUR),
            evidence("outpost", "hangar-bay.png"),
            "0.0.2-nightly",
        ),
        project(
            "project-5",
            "Prototype 7",
            "Prototype 7",
            None,
            None,
            "0.0.1-alpha",
        ),
    ];
    let status = |p: &Value, state: &str, headless: Option<bool>| {
        let open = state == "open";
        json!({"project":p,"state":state,"pid":open.then_some(4120),"port":open.then_some(52011),"headless":headless})
    };
    let statuses = vec![
        status(&projects[0], "open", Some(false)),
        status(&projects[1], "open", Some(true)),
        status(&projects[2], "closed", None),
        status(&projects[3], "closed", None),
        status(&projects[4], "closed", None),
    ];
    let templates = json!({"templates":[
        {"id":"blank","name":"Blank project","thumbnail":null,"description":"An empty scene, ready for your first entity."},
        {"id":"outpost","name":"Outpost","thumbnail":evidence("outpost","outpost-player.png")},
        {"id":"showcase","name":"Material and character showcase","thumbnail":evidence("showcase","showcase-player.png"),
         "setup":with_projects.then(|| json!({"message":"Material and character showcase needs its third-party art: 212 of 212 pinned files are not downloaded","command":"ara-samples fetch showcase"}))},
        {"id":"starter","name":"Starter","thumbnail":null}
    ]});
    let install = r"C:\Users\tester\AppData\Local\Programs\Ara";
    let mut engines = json!({"engines":[
        {"version":"0.0.1-alpha","executable":format!(r"{install}\0.0.1-alpha\ara.exe"),"editor":format!(r"{install}\0.0.1-alpha\ara-editor.exe")},
        {"version":"0.0.2-nightly","executable":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly\ara.exe","editor":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly\ara-editor.exe","managed":true}
    ]});
    if !with_projects && let Some(builds) = engines["engines"].as_array_mut() {
        builds.truncate(1);
    }
    let activity = json!({"activity":[
        {"project":"project-1","participants":[
            {"id":"ses_ed01","kind":"human","name":"ara-editor","joinedMs":0},
            {"id":"ses_ag01","kind":"agent","name":"claude-code","joinedMs":0}],
         "presence":{
            "ses_ed01":{"doc":format!("{}/levels/outpost.scene",ara_hub_service::AUTHORING_ROOT),"selection":["e_campfire"]},
            "ses_ag01":{"doc":format!("{}/levels/outpost.scene",ara_hub_service::AUTHORING_ROOT),"selection":[],"activity":{"what":"entity.create, transform.set","why":"lining the fence with floodlights"}}}},
        {"project":"project-2","participants":[
            {"id":"ses_ag02","kind":"agent","name":"codex","joinedMs":0}],
         "presence":{"ses_ag02":{"doc":format!("{}/levels/showcase.scene",ara_hub_service::AUTHORING_ROOT),"selection":["e_plinth03","e_plinth04"]}}}
    ]});
    let (projects, statuses, activity) = if with_projects {
        (projects, statuses, activity)
    } else {
        (Vec::new(), Vec::new(), json!({"activity":[]}))
    };
    hub.event(Event::Connected);
    for call in hub.take_calls() {
        let result = match &call {
            Call::Projects => json!({"projects":projects,"revision":7}),
            Call::Status => json!({"projects":statuses,"revision":7}),
            Call::Templates => templates.clone(),
            Call::Engines => engines.clone(),
            Call::Activity => activity.clone(),
            _ => continue,
        };
        hub.event(Event::Reply {
            call,
            result: Ok(result),
        });
    }
    // The form's dry run: accept, as a fresh destination would.
    for call in hub.take_calls() {
        hub.event(Event::Reply {
            call,
            result: Ok(json!({"valid":true})),
        });
    }
    hub.create.destination = base.into();
    let cli = if cfg!(windows) {
        r"C:\Ara Test\ara.exe"
    } else {
        "/opt/ara/bin/ara"
    };
    hub.cli_executable = Some(cli.into());
    hub.install.insert(
        AgentClient::Claude,
        ara_json::to_canonical_string(
            &json!({"mcpServers":{"ara":{"args":["mcp"],"command":cli}}}),
        ),
    );
    hub.install.insert(
        AgentClient::Codex,
        format!(
            "[mcp_servers.ara]\ncommand = {}\nargs = [\"mcp\"]\n",
            json!(cli)
        ),
    );
    hub.install.insert(
        AgentClient::OpenCode,
        ara_json::to_canonical_string(&json!({
            "$schema": "https://opencode.ai/config.json",
            "mcp": {"ara": {"type": "local", "command": [cli, "mcp"], "enabled": true}}
        })),
    );
    // Releases and incomplete samples are separate demonstration states, not
    // the default state of a newly installed current build.
    if !with_projects {
        hub.take_calls();
        return;
    }
    hub.releases.can_self_update = true;
    for version in ["0.0.1-alpha", "0.0.2-nightly", "0.0.3-alpha"] {
        hub.event(Event::Reply {
            call: Call::Release { method: "hub.engine.preflight", params: json!({"version":version}) },
            result: Ok(json!({"destination":format!(r"C:\Users\tester\AppData\Local\Ara\Engines\{version}"),"downloadBytes":864_000_000_u64,"requiredBytes":3_456_000_000_u64,"availableBytes":42_000_000_000_u64,"channel":"preview"})),
        });
    }
    let release = |kind: &str, version: &str, channel: &str| {
        json!({
            "format":"ara.release/1","kind":kind,"version":version,"channel":channel,
            "minHubVersion":"0.0.1-alpha","platform":"windows-x64",
            "notes":"Improved editor tools, faster project startup and renderer fixes. Installs beside your current engine; existing project pins stay as they are.",
            "assets":[{"name":format!("ara-{kind}-{version}-windows-x64.zip"),"size":864_000_000_u64,"sha256":"00".repeat(32),"url":"https://github.com/Ara-Engine/ara/releases"}]
        })
    };
    for (method, value) in [
        (
            "hub.engines.available",
            json!({"releases":[release("engine","0.0.3-alpha","preview")],"status":{"state":"fresh"},"checking":false,"installRoot":r"C:\Users\tester\AppData\Local\Ara\Engines"}),
        ),
        (
            "hub.updates",
            json!({"releases":[release("hub","0.1.0","stable")],"status":{"state":"fresh"},"checking":false,"channel":"preview","repo":"Ara-Engine/ara"}),
        ),
        (
            "hub.downloads",
            json!({"jobs":[
                {"id":"hub-0.1.0","kind":"hub","version":"0.1.0","state":"complete","progress":{"received":18_000_000,"total":18_000_000},"result":{"archive":"target/hub-fixture.zip","manifest":"fixture","signature":"fixture"}},
                {"id":"engine-0.0.3-alpha","kind":"engine","version":"0.0.3-alpha","state":"running","progress":{"received":328_320_000,"total":864_000_000}}
            ]}),
        ),
    ] {
        hub.event(Event::Reply {
            call: Call::Release {
                method,
                params: json!({}),
            },
            result: Ok(value),
        });
    }
    hub.take_calls();
}

/// One captured state: a name and how to reach it from the seeded model.
type Step = (&'static str, fn(&mut Hub));
const STEPS: &[Step] = &[
    ("projects", |hub| {
        hub.projects[2].engine_version = "0.0.1-alpha".into();
        hub.view = View::Projects;
        hub.prefs.layout = Layout::Grid;
        hub.menu = None;
    }),
    ("projects-menu", |hub| hub.menu = Some("project-3".into())),
    ("projects-list", |hub| {
        hub.menu = None;
        hub.prefs.layout = Layout::List;
    }),
    ("projects-missing-engine", |hub| {
        hub.prefs.layout = Layout::Grid;
        hub.projects[2].engine_version = "0.0.3-alpha".into();
    }),
    ("create", |hub| {
        hub.prefs.layout = Layout::Grid;
        hub.intent(Intent::Navigate(View::Create));
        for call in hub.take_calls() {
            let result = Ok(json!({"valid":true}));
            hub.event(Event::Reply { call, result });
        }
    }),
    ("installs", |hub| hub.view = View::Installs),
    ("update-banner", |hub| hub.view = View::Settings),
    ("agents", |hub| {
        hub.view = View::Agents;
        for (client, state) in [
            (AgentClient::Claude, AgentSetup::Available),
            (AgentClient::Codex, AgentSetup::Available),
            (AgentClient::Cursor, AgentSetup::Missing),
            (AgentClient::OpenCode, AgentSetup::Missing),
        ] {
            hub.agent_setup.insert(client, state);
        }
    }),
    ("agents-manual", |hub| {
        hub.intent(Intent::ManualAgent(Some(AgentClient::OpenCode)))
    }),
    ("agents-connected", |hub| {
        hub.intent(Intent::ManualAgent(None));
        hub.agent_setup
            .insert(AgentClient::Claude, AgentSetup::Connected);
        hub.agent_setup
            .insert(AgentClient::Codex, AgentSetup::Connected);
    }),
    ("learn", |hub| hub.view = View::Learn),
];

/// A headless Hub window for captures and tests: the real view over a stillkit runtime.
/// Fixture models are swapped in for each operation (`settle`, `save`, `click`), so a
/// script keeps driving its own [`Hub`] through intents and replies.
pub struct Capture {
    rt: Runtime<HubApp>,
    physical: [u32; 2],
    dpi: f32,
}

impl Capture {
    /// A `physical` px window at `dpi` device px per design px.
    pub fn new(physical: [u32; 2], dpi: f32) -> anyhow::Result<Self> {
        let width = physical[0] as f32 / dpi;
        let hub = Hub::new(
            crate::model::Prefs::default(),
            PathBuf::from("target/hub-captures"),
            None,
        );
        let mut app = HubApp::new(hub, width);
        app.now_ms = NOW;
        app.frame.controls = (!cfg!(target_os = "macos")).then_some(false);
        let theme = app.look.theme.clone();
        let mut rt = Runtime::new(app, crate::ui::view, theme, (physical[0], physical[1]), dpi)?;
        rt.set_reduced_motion(true);
        Ok(Self { rt, physical, dpi })
    }

    /// The window's size in device px.
    pub fn physical(&self) -> [u32; 2] {
        self.physical
    }

    /// The window's size in design px.
    pub fn logical(&self) -> [f32; 2] {
        self.physical.map(|v| v as f32 / self.dpi)
    }

    /// The runtime, for tests that drive input directly.
    pub fn runtime(&mut self) -> &mut Runtime<HubApp> {
        &mut self.rt
    }

    /// Runs `body` with `hub` as the window's model, then hands it back.
    fn with<T>(&mut self, hub: &mut Hub, body: impl FnOnce(&mut Runtime<HubApp>) -> T) -> T {
        self.rt.update(|s| std::mem::swap(&mut s.hub, hub));
        self.rt.update(|s| {
            if s.sync_look() {
                s.images.keep_scheme(s.look.dark);
            }
        });
        let theme = self.rt.state().look.theme.clone();
        self.rt.set_theme(theme);
        let out = body(&mut self.rt);
        self.rt.update(|s| std::mem::swap(&mut s.hub, hub));
        out
    }

    /// Lays out `hub` and loads every picture it shows, until nothing more arrives.
    pub fn settle(&mut self, hub: &mut Hub) {
        self.with(hub, |rt| {
            for _ in 0..6 {
                rt.frame(Duration::from_millis(16));
                let mut loaded = false;
                rt.update(|s| {
                    let look = s.look.clone();
                    loaded = s.images.load_now(&look);
                });
                if !loaded {
                    break;
                }
            }
            for _ in 0..30 {
                rt.frame(Duration::from_millis(16));
            }
        });
    }

    /// Settles `hub`, then writes the window as PNG to `path`.
    pub fn save(&mut self, hub: &mut Hub, path: &Path) -> anyhow::Result<()> {
        self.settle(hub);
        let png = self.with(hub, |rt| {
            rt.snap().and_then(|image| ara_kit::capture::png(&image))
        })?;
        std::fs::write(path, png)?;
        println!("{}", path.display());
        Ok(())
    }

    /// Clicks the element keyed `key` as a person would, with `hub` as the model.
    pub fn click(&mut self, hub: &mut Hub, key: &str) -> anyhow::Result<()> {
        let found = self.with(hub, |rt| {
            rt.frame(Duration::from_millis(16));
            let found = rt.find(key).is_some();
            if found {
                rt.click(key);
                rt.frame(Duration::from_millis(16));
            }
            found
        });
        anyhow::ensure!(found, "no control keyed {key}");
        Ok(())
    }

    /// Presses a key (a virtual-key code) with modifiers held, with `hub` as the model.
    pub fn key(&mut self, hub: &mut Hub, vk: u16, mods: stillkit::el::Modifiers) {
        self.with(hub, |rt| {
            rt.frame(Duration::from_millis(16));
            rt.key_with(vk, mods);
            rt.frame(Duration::from_millis(16));
        });
    }

    /// Types `text` into the focused field, with `hub` as the model.
    pub fn typing(&mut self, hub: &mut Hub, text: &str) {
        self.with(hub, |rt| {
            rt.type_text(text);
            rt.frame(Duration::from_millis(16));
        });
    }

    /// Moves keyboard focus to the element keyed `key`.
    pub fn focus(&mut self, hub: &mut Hub, key: &str) {
        self.with(hub, |rt| {
            rt.frame(Duration::from_millis(16));
            rt.focus(key);
            rt.frame(Duration::from_millis(16));
        });
    }

    /// Whether an element keyed `key` shows for `hub`.
    pub fn shows(&mut self, hub: &mut Hub, key: &str) -> bool {
        self.with(hub, |rt| {
            rt.frame(Duration::from_millis(16));
            rt.find(key).is_some()
        })
    }

    /// Runs `change` on the window's own state (the palette, a dropdown) with `hub` in it.
    pub fn window(&mut self, hub: &mut Hub, change: impl FnOnce(&mut HubApp)) {
        self.with(hub, |rt| rt.update(change));
    }
}

/// Seed, then capture every step in both themes under `directory/<w>x<h>/`.
pub fn run(cap: &mut Capture, hub: &mut Hub, directory: &Path) -> anyhow::Result<()> {
    seed(hub);
    let physical = cap.physical();
    let folder = directory.join(format!("{}x{}", physical[0], physical[1]));
    std::fs::create_dir_all(&folder)?;
    for light in [false, true] {
        let theme = if light { "light" } else { "dark" };
        // What a tester sees right after installing: no projects yet.
        let mut fresh = Hub::new(
            hub.prefs.clone(),
            hub.create.destination.clone().into(),
            None,
        );
        seed_first_run(&mut fresh);
        fresh.prefs.light = light;
        cap.save(&mut fresh, &folder.join(format!("{theme}-first-run.png")))?;
        hub.prefs.light = light;
        for (name, step) in STEPS {
            step(hub);
            cap.save(hub, &folder.join(format!("{theme}-{name}.png")))?;
        }
        interactions::run(cap, &folder, light)?;
        flows::run(cap, &folder, light)?;
        migration::run(cap, &folder, light)?;
        engine_layout::run(cap, &folder, light)?;
    }
    Ok(())
}
