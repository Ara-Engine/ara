//! C# editor contributions use the native registry and ordinary attributed ops.
use ara_editor::{Editor, shell::Shell};
use ara_proto::ConnId;
use serde_json::{Value, json};
use std::{
    fs,
    time::{Duration, Instant},
};

#[derive(Default)]
struct MemoryClipboard(String);
impl ara_ui_text::Clipboard for MemoryClipboard {
    fn read(&mut self) -> ara_ui_text::Result<String> {
        Ok(self.0.clone())
    }
    fn write(&mut self, value: &str) -> ara_ui_text::Result<()> {
        self.0 = value.into();
        Ok(())
    }
}

#[test]
fn csharp_commands_share_native_menus_palette_history_and_agent_attribution() {
    let root = std::env::temp_dir().join(format!("ara-csharp-editor-{}", std::process::id()));
    fs::create_dir_all(root.join("levels")).expect("scenes");
    let sources = root.join(ara_doc::AUTHORING_ROOT).join("tools/editor");
    fs::create_dir_all(&sources).expect("editor sources");
    let write = |path: &str, value: Value| {
        fs::write(root.join(path), ara_json::to_canonical_string(&value)).expect("fixture")
    };
    write(
        "ara.project.json",
        json!({"name":"C# editor API","main":"levels/main.scene"}),
    );
    write(
        "levels/main.scene",
        json!({"$schema":"ara://schema/scene/1","id":"sc_editor01","records":{
            "e_root001":{"name":"Root","Transform":{}},
            "e_child01":{"name":"Child","parent":"e_root001","Transform":{}}
        }}),
    );
    fs::write(sources.join("Tools.cs"), r#"
        using Ara.Editor;
        public sealed class Tools : IEditorExtension {
            public void Register(EditorRegistry registry) {
                registry.RegisterCommand("tools.mark", "Mark selection", context => {
                    var child = context.Scene.Find("Root/Child")!;
                    if (context.Scene.Parent(child.Id)?.Name != "Root" || context.Scene.Children(context.Scene.Find("Root")!.Id).Count != 1)
                        throw new System.InvalidOperationException("scene query failed");
                    context.Commands.SetProperty(child.Id, "Transform.position", new System.Numerics.Vector3(1,2,3));
                    context.Commands.Reparent(child.Id, null);
                    var marker = context.Commands.Create("Marker", child.Id);
                    var temporary = context.Commands.Create("Temporary");
                    context.Commands.AddComponent(temporary, "Transform", new {});
                    context.Commands.RemoveComponent(temporary, "Transform");
                    context.Commands.Delete(temporary);
                    context.Selection.Set([marker]);
                }, "Mark the selected entity with reversible authoring commands.");
                registry.AddMenu("Tools/Mark selection", "tools.mark");
            }
        }
    "#).expect("C# fixture");
    let mut editor = Editor::open(&root).expect("open without a window");
    let deadline = Instant::now() + Duration::from_secs(90);
    let catalog = loop {
        editor.tick();
        let catalog = editor
            .action("editor.commands", json!({}))
            .expect("registry");
        if catalog["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|c| c["id"] == "tools.mark")
        {
            break catalog;
        }
        assert!(
            Instant::now() < deadline,
            "C# command was not registered ({} native entries)",
            catalog["commands"].as_array().expect("commands").len()
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        catalog["commands"]
            .as_array()
            .expect("commands")
            .iter()
            .any(|c| c["id"] == "tools.mark" && c["menu"] == "Tools")
    );
    let mut shell = Shell::new(&mut editor, [1440., 900.]).expect("native UI without a window");
    assert!(
        shell
            .palette_rows(&mut editor, "Mark selection")
            .iter()
            .any(|(_, title, id)| title == "Mark selection" && id.as_deref() == Some("tools.mark"))
    );
    assert!(
        shell
            .walk_menus(&mut editor)
            .expect("menus")
            .iter()
            .any(|(path, id)| path == "Tools" && id == "tools.mark")
    );
    if let Some(output) = std::env::var_os("ARA_CSHARP_UI_CAPTURE") {
        let mut clipboard = MemoryClipboard::default();
        let index = shell
            .ui
            .nodes
            .iter()
            .filter(|n| n.element.id.starts_with("menu-title-") && !n.element.id.ends_with("-text"))
            .find(|n| {
                n.element
                    .attrs
                    .get("label")
                    .is_some_and(|label| label == "Tools menu")
            })
            .expect("Tools menu title")
            .element
            .id
            .clone();
        shell
            .click(&index, &mut clipboard)
            .expect("open native Tools menu");
        shell.dispatch(&mut editor).expect("menu dispatch");
        shell.frame().expect("paint frame");
        let device = ara_editor::gpu::open_presentation(&[]).expect("offscreen device");
        // SAFETY: the device outlives the adapter and this thread owns its queue.
        let mut gpu = unsafe { ara_editor::gpu::NativeUi::new(&device).expect("UI adapter") };
        let mut renderer = ara_ui_render::gpu::Renderer::default();
        renderer
            .render(
                &mut gpu,
                &shell.ui.display,
                &shell.text.assets,
                [1440, 900],
                ara_ui_render::atlas::TextMode::Msdf,
            )
            .expect("offscreen UI render");
        gpu.pixels()
            .expect("readback")
            .save(output)
            .expect("capture");
    }
    let agent = editor
        .host
        .call(
            ConnId::LOCAL,
            "session.begin",
            json!({"client":"editor-api", "author":"agent:editor-api"}),
        )
        .expect("session")["session"]
        .as_str()
        .expect("id")
        .to_owned();
    let pending = editor
        .host
        .call(
            ConnId::LOCAL,
            "editor.command",
            json!({"session":agent,"id":"tools.mark"}),
        )
        .expect("invoke");
    let job = pending["job"].as_str().expect("worker job");
    let result = loop {
        editor.tick();
        let status = editor
            .host
            .call(
                ConnId::LOCAL,
                "jobs.status",
                json!({"session":agent,"job":job}),
            )
            .expect("job");
        if status["status"] == "done" || status["status"] == "failed" {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "callback did not finish: {status}"
        );
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(result["status"], "done", "{result}");
    let doc = editor
        .host
        .store()
        .workspace()
        .doc("sc_editor01")
        .expect("scene");
    assert_eq!(
        doc.record("e_child01").expect("child")["Transform"]["position"],
        json!([1, 2, 3])
    );
    assert!(
        doc.record("e_child01")
            .expect("child")
            .get("parent")
            .is_none()
    );
    assert!(
        doc.records()
            .values()
            .any(|r| r.get("name") == Some(&json!("Marker")))
    );
    assert!(
        editor.state.selection.is_empty(),
        "an agent's selection must not move the human's view"
    );
    let history = editor
        .host
        .call(
            ConnId::LOCAL,
            "history.list",
            json!({"session":agent,"author":"agent:editor-api"}),
        )
        .expect("history");
    assert!(history.to_string().contains("tools.mark"), "{history}");
    editor
        .host
        .call(ConnId::LOCAL, "history.undo", json!({"session":agent}))
        .expect("ordinary undo");
    let doc = editor
        .host
        .store()
        .workspace()
        .doc("sc_editor01")
        .expect("scene");
    assert_eq!(
        doc.record("e_child01").expect("child")["parent"],
        "e_root001"
    );
    assert!(
        !doc.records()
            .values()
            .any(|r| r.get("name") == Some(&json!("Marker")))
    );
    let pending = editor
        .host
        .call(
            ConnId::LOCAL,
            "editor.command",
            json!({"session":agent,"id":"tools.mark"}),
        )
        .expect("second callback");
    // Do not tick between snapshot capture and this competing authority edit.
    editor.host.call(ConnId::LOCAL, "cmd.apply", json!({"session":agent,"doc":"sc_editor01","why":"Concurrent scene edit", "commands":[{"cmd":"property.set","args":{"entity":"e_root001","path":"name","value":"Changed root"}}]})).expect("concurrent edit");
    loop {
        editor.tick();
        let status = editor
            .host
            .call(
                ConnId::LOCAL,
                "jobs.status",
                json!({"session":agent,"job":pending["job"]}),
            )
            .expect("stale job");
        if status["status"] == "failed" {
            assert!(
                status["diagnostics"].to_string().contains("ARA1501"),
                "{status}"
            );
            break;
        }
        assert_ne!(status["status"], "done", "stale callback committed");
        assert!(Instant::now() < deadline, "stale callback did not finish");
        std::thread::sleep(Duration::from_millis(5));
    }
    let doc = editor
        .host
        .store()
        .workspace()
        .doc("sc_editor01")
        .expect("scene");
    assert_eq!(
        doc.record("e_child01").expect("child")["parent"],
        "e_root001"
    );
    assert!(
        !doc.records()
            .values()
            .any(|r| r.get("name") == Some(&json!("Marker")))
    );
}
