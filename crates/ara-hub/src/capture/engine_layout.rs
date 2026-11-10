//! Capability consent is captured through native controls without any engine process.
use super::*;

pub(super) fn seed_dialog(hub: &mut Hub, available: bool) {
    hub.dialog = None;
    hub.status.remove("project-1");
    hub.intent(Intent::Open("project-1".into()));
    for call in hub.take_calls() {
        if matches!(call, Call::Open { .. }) {
            hub.event(Event::Reply {
                call,
                result: Err(ara_diag::Diagnostic::error(
                    ara_diag::codes::PROJECT_ENGINE_LAYOUT_UNSUPPORTED,
                    "This project uses the `content/` layout and needs Ara preview 4 or newer",
                )
                .into()),
            });
        }
    }
    for call in hub.take_calls() {
        if call.method() == "hub.project.currentEngine" {
            hub.event(Event::Reply{call,result:Ok(json!({"engineVersion":"0.0.1-alpha","available":available,"registered":available}))});
        }
    }
}

pub(super) fn run(cap: &mut Capture, folder: &Path, light: bool) -> anyhow::Result<()> {
    let theme = if light { "light" } else { "dark" };
    for (available, label) in [(true, "use-current"), (false, "install-current")] {
        let mut hub = Hub::new(
            crate::model::Prefs {
                light,
                ..Default::default()
            },
            "target/hub-flows".into(),
            None,
        );
        seed(&mut hub);
        seed_dialog(&mut hub, available);
        cap.save(
            &mut hub,
            &folder.join(format!("{theme}-engine-layout-{label}.png")),
        )?;
        if available {
            cap.click(&mut hub, "engine-layout-use")?;
            for call in hub.take_calls() {
                if call.method() == "hub.project.useCurrentEngine" {
                    hub.event(Event::Reply{call,result:Ok(json!({"id":"pin-fixture","project":"project-1","kind":"pin","state":"running","phase":"Saving engine choice and history","cancellable":false,"progress":0.,"result":null,"diagnostics":[]}))});
                }
            }
            cap.save(
                &mut hub,
                &folder.join(format!("{theme}-engine-layout-saving.png")),
            )?;
        } else {
            cap.click(&mut hub, "engine-layout-use")?;
            hub.status.clear();
            for call in hub.take_calls() {
                if call.method() == "hub.engine.preflight" {
                    hub.event(Event::Reply{call,result:Ok(json!({"destination":r"C:\Users\tester\AppData\Local\Ara\Engines\0.0.1-alpha","downloadBytes":178_257_920u64,"requiredBytes":713_031_680u64,"availableBytes":120_000_000_000u64,"spaceBasis":"estimate","channel":"preview"}))});
                }
            }
            cap.save(
                &mut hub,
                &folder.join(format!("{theme}-engine-layout-install-confirm.png")),
            )?;
            cap.click(&mut hub, "dialog-cancel")?;
            anyhow::ensure!(
                hub.engine_layout.is_none() && hub.take_calls().is_empty(),
                "Cancel does not pin or install an engine"
            );
        }
    }
    Ok(())
}
