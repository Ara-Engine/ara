//! `ara-hub --capture DIR`: every view, dark and light, rendered by the real GPU
//! path (a headless device and the window's UI adapter; no window opens) and read
//! back. The data is a fixed fixture fed through the model's own
//! reply handling (no service), so captures are reproducible; the service path is
//! covered by `tests/service.rs`.
use crate::{
    connection::Event,
    model::{AgentClient, Call, Hub, Intent, Layout, View},
    native::Offscreen,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

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

fn seed_with(hub: &mut Hub, with_projects: bool) {
    let root = checkout();
    let evidence = |sample: &str, file: &str| {
        root.as_ref()
            .map(|r| r.join("samples").join(sample).join("evidence").join(file))
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
         "setup":{"message":"Material and character showcase needs its third-party art: 212 of 212 pinned files are not downloaded","command":"ara-samples fetch showcase"}},
        {"id":"starter","name":"Starter","thumbnail":null}
    ]});
    let install = r"C:\Users\tester\AppData\Local\Programs\Ara";
    let engines = json!({"engines":[
        {"version":"0.0.1-alpha","executable":format!(r"{install}\0.0.1-alpha\ara.exe"),"editor":format!(r"{install}\0.0.1-alpha\ara-editor.exe")},
        {"version":"0.0.2-nightly","executable":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly\ara.exe","editor":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.2-nightly\ara-editor.exe","managed":true}
    ]});
    let activity = json!({"activity":[
        {"project":"project-1","participants":[
            {"id":"ses_ed01","kind":"human","name":"ara-editor","joinedMs":0},
            {"id":"ses_ag01","kind":"agent","name":"claude-code","joinedMs":0}],
         "presence":{
            "ses_ed01":{"doc":"levels/outpost.scene","selection":["e_campfire"]},
            "ses_ag01":{"doc":"levels/outpost.scene","selection":[],"activity":{"what":"entity.create, transform.set","why":"lining the fence with floodlights"}}}},
        {"project":"project-2","participants":[
            {"id":"ses_ag02","kind":"agent","name":"codex","joinedMs":0}],
         "presence":{"ses_ag02":{"doc":"levels/showcase.scene","selection":["e_plinth03","e_plinth04"]}}}
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
    hub.install.insert(
        AgentClient::Claude,
        "{\n  \"mcpServers\": {\n    \"ara\": {\n      \"args\": [\"mcp\"],\n      \"command\": \"ara\"\n    }\n  }\n}\n".into(),
    );
    hub.install.insert(
        AgentClient::Codex,
        "[mcp_servers.ara]\ncommand = \"ara\"\nargs = [\"mcp\"]\n".into(),
    );
    let release = |kind: &str, version: &str, channel: &str| {
        json!({
            "format":"ara.release/1","kind":kind,"version":version,"channel":channel,
            "minHubVersion":"0.0.1-alpha","platform":"windows-x64",
            "notes":"Improved editor tools, faster project startup and renderer fixes. Installs beside your current engine; existing project pins stay as they are.",
            "assets":[{"name":format!("ara-{kind}-{version}-windows-x64.zip"),"size":864_000_000_u64,"sha256":"00".repeat(32),"url":"https://github.com/Snowy7/ara/releases"}]
        })
    };
    for (method, value) in [
        (
            "hub.engines.available",
            json!({"releases":[release("engine","0.0.3-alpha","preview")],"status":{"state":"fresh"},"checking":false}),
        ),
        (
            "hub.updates",
            json!({"releases":[release("hub","0.1.0","stable")],"status":{"state":"fresh"},"checking":false,"channel":"preview","repo":"Snowy7/ara"}),
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
    ("agents", |hub| hub.view = View::Agents),
    ("learn", |hub| hub.view = View::Learn),
];

/// Seed, then capture every step in both themes under `directory/<w>x<h>/`.
pub fn run(
    host: &mut Offscreen,
    app: &mut crate::native::App,
    hub: &mut Hub,
    directory: &Path,
) -> anyhow::Result<()> {
    seed(hub);
    let physical = host.physical();
    let folder = directory.join(format!("{}x{}", physical[0], physical[1]));
    std::fs::create_dir_all(&folder)?;
    app.size = host.logical();
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
        settle(host, app, &mut fresh)?;
        let path = folder.join(format!("{theme}-first-run.png"));
        host.capture(app)?.save(&path)?;
        println!("{}", path.display());
        hub.prefs.light = light;
        for (name, step) in STEPS {
            step(hub);
            settle(host, app, hub)?;
            let path = folder.join(format!("{theme}-{name}.png"));
            host.capture(app)?.save(&path)?;
            println!("{}", path.display());
        }
    }
    Ok(())
}

/// Rebuild until every requested preview has arrived (or two seconds pass).
fn settle(host: &mut Offscreen, app: &mut crate::native::App, hub: &mut Hub) -> anyhow::Result<()> {
    let until = Instant::now() + Duration::from_secs(2);
    // Twice: the overflow menu anchors to the previous layout of its button.
    for _ in 0..2 {
        app.rebuild(hub, NOW).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    while app.thumbnails.pending() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(10));
        if host.upload(app) {
            app.rebuild(hub, NOW).map_err(|e| anyhow::anyhow!("{e}"))?;
        }
    }
    if host.upload(app) {
        app.rebuild(hub, NOW).map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    Ok(())
}
