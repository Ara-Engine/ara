//! Published watched modules must keep dependency edits and last-good code working.
mod support;

use ara_script::{
    EditOrigin, ModuleProfile,
    abi::{RELOAD_ERROR, RELOAD_INITIAL, RELOAD_L1, RELOAD_L3},
    project,
};
use serde_json::json;
use std::path::Path;

const CORE: &str = r#"
/// <summary>An observed gameplay value.</summary>
[Component] public partial struct Result {
    /// <summary>The value written by Game.</summary>
    public int Value;
}
public static class Policy { public static int Step() => 1; }
"#;
const GAME: &str = r#"
public static partial class Update {
    [Query] static void Run(ref Result result) { result.Value = Policy.Step() + 10; }
}
"#;

fn write(path: &Path, text: &str) {
    std::fs::write(path, text).expect("scratch project source");
}

fn value(live: &support::Live, entity: bevy_ecs::entity::Entity) -> i64 {
    live.host
        .read_component(&live.world, entity, "Result")
        .expect("result component")["value"]
        .as_i64()
        .expect("integer value")
}

fn is_module_error(diagnostic: &ara_diag::Diagnostic, module: &str) -> bool {
    diagnostic.is_error()
        && diagnostic
            .data
            .as_ref()
            .and_then(|data| data["module"].as_str())
            == Some(module)
}

fn wait(
    live: &mut support::Live,
    entity: bevy_ecs::entity::Entity,
    expected: i64,
    check: impl Fn(&support::Live),
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        live.tick();
        check(live);
        if value(live, entity) == expected {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "expected {expected}, got {}; reports={:?}; errors={:?}",
            value(live, entity),
            live.reports,
            live.diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.is_error())
                .map(|diagnostic| &diagnostic.message)
                .collect::<Vec<_>>()
        );
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn wait_error(live: &mut support::Live, module: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while !live
        .diagnostics
        .iter()
        .any(|diagnostic| is_module_error(diagnostic, module))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "missing error for {module}"
        );
        live.tick();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

fn completion<'a>(live: &'a support::Live, module: &str) -> &'a serde_json::Value {
    live.diagnostics
        .iter()
        .rev()
        .find_map(|diagnostic| {
            let data = diagnostic.data.as_ref()?;
            (data["module"].as_str() == Some(module)
                && data["compile"]["phase"].as_str() == Some("completed"))
            .then_some(&data["compile"])
        })
        .expect("module has a completed compilation scope")
}

fn edit(
    live: &mut support::Live,
    entity: bevy_ecs::entity::Entity,
    path: &Path,
    module: &str,
    source: &str,
    expected: i64,
    kind: u32,
) {
    let before = live.host.modules();
    let mut after = before.clone();
    let members: Vec<_> = before
        .iter()
        .filter(|member| {
            member.name == module
                || (kind == RELOAD_L3 && module == "Core" && member.name == "Game")
        })
        .map(|member| member.slot)
        .collect();
    if kind == RELOAD_L3 {
        for member in &mut after {
            if members.contains(&member.slot) {
                member.generation += 1;
            }
            if member.name == module {
                member.hot = true;
            }
        }
    }
    let cursor = live.reports.len();
    write(path, source);
    assert!(
        live.host
            .request_compile(module, &[path.to_path_buf()], EditOrigin::Human)
    );
    wait(live, entity, expected, |live| {
        let reports = live.reports[cursor..]
            .iter()
            .filter(|report| report.kind != RELOAD_ERROR)
            .collect::<Vec<_>>();
        assert!(
            reports.is_empty() || reports.len() == members.len(),
            "one atomic applying boundary: {reports:?}"
        );
        assert_eq!(
            live.host.modules().as_slice(),
            if reports.is_empty() {
                before.as_slice()
            } else {
                after.as_slice()
            }
        );
        if !reports.is_empty() {
            assert_eq!(
                reports
                    .iter()
                    .map(|report| report.module)
                    .collect::<Vec<_>>(),
                members
            );
            assert!(
                reports
                    .iter()
                    .all(|report| report.kind == kind && report.cascade as usize == members.len())
            );
        }
    });
    assert_eq!(
        live.reports[cursor..]
            .iter()
            .filter(|report| report.kind != RELOAD_ERROR)
            .count(),
        members.len()
    );
    assert_eq!(completion(live, module)["result"], "success");
    assert_eq!(completion(live, module)["errorCount"], 0);
}

fn edits(root_first: bool, core_watched: bool, valid_core_source: bool) {
    let _clr = support::clr_lock();
    support::build();
    for hot in [false, true] {
        let root = support::dotnet()
            .parent()
            .expect("repository")
            .join("target/prebuilt-watch")
            .join(format!(
                "{}-{root_first}-{hot}-{core_watched}-{valid_core_source}",
                std::process::id()
            ));
        let content = root.join(ara_doc::AUTHORING_ROOT);
        std::fs::create_dir_all(content.join("Core")).expect("Core directory");
        std::fs::create_dir_all(content.join("Game")).expect("Game directory");
        std::fs::create_dir_all(content.join("Unrelated")).expect("independent directory");
        write(
            &root.join("ara.project.json"),
            &ara_json::to_canonical_string(
                &json!({"layoutVersion": ara_doc::PROJECT_LAYOUT_VERSION}),
            ),
        );
        for (name, references) in [
            ("Core", vec![]),
            ("Game", vec!["Core"]),
            ("Unrelated", vec![]),
        ] {
            write(
                &content
                    .join(name)
                    .join(format!("{}.module.json", name.to_lowercase())),
                &ara_json::to_canonical_string(
                    &json!({"name":name,"hot":hot,"references":references}),
                ),
            );
        }
        let core = content.join("Core/Code.cs");
        let game = content.join("Game/Code.cs");
        write(&core, CORE);
        write(&game, GAME);
        let unrelated = content.join("Unrelated/Code.cs");
        write(
            &unrelated,
            "public static class Auxiliary { public static int Value => 7; }",
        );
        let publication = project::publish(&root, ModuleProfile::Gameplay).expect("publication");
        let mut specs = publication
            .gameplay_specs(&root)
            .expect("published module specs");
        assert_eq!(specs.len(), 3);
        for spec in &mut specs {
            spec.watch = spec.name != "Core" || core_watched;
            assert!(spec.assembly.as_ref().is_some_and(|path| path.is_file()));
        }
        if !valid_core_source {
            assert!(!core_watched);
            write(
                &core,
                "the explicitly supplied Core image must remain authoritative",
            );
        }
        let mut live = support::Live::host(root);
        live.host
            .load_modules(&mut live.world, &specs)
            .expect("published watched modules load");
        let entity = live.world.spawn_empty().id();
        live.host
            .insert_component(&mut live.world, entity, "Result", &json!({"value":0}))
            .expect("result");
        wait(&mut live, entity, 11, |_| {});
        assert!(
            live.diagnostics
                .iter()
                .all(|diagnostic| !diagnostic.is_error()),
            "initial publication is valid: {:?}",
            live.diagnostics
        );
        assert_eq!(live.reports.len(), 3);
        assert!(
            live.reports
                .iter()
                .all(|report| report.kind == RELOAD_INITIAL
                    && report.generation == 0
                    && ((!valid_core_source && report.module == 1)
                        || (report.parse_us == 0
                            && report.generate_us == 0
                            && report.emit_us == 0))),
            "published startup must not repeat Roslyn compilation: {:?}",
            live.reports
        );
        assert!(
            live.host
                .modules()
                .iter()
                .all(|module| module.generation == 0 && module.hot == hot)
        );
        live.reports.clear();
        let native_id = live.host.component_id("Result").expect("native component");
        write(&unrelated, "this is not valid C#");
        assert!(
            live.host
                .request_compile("Unrelated", &[unrelated], EditOrigin::Human)
        );
        wait_error(&mut live, "Unrelated");
        assert_eq!(value(&live, entity), 11);
        let (path, module, first, second, repaired, expected) = if root_first {
            (
                &core,
                "Core",
                CORE.replace("=> 1", "=> 2"),
                CORE.replace("=> 1", "=> 3"),
                CORE.replace("=> 1", "=> 4"),
                [12, 13, 14],
            )
        } else {
            (
                &game,
                "Game",
                GAME.replace("+ 10", "+ 20"),
                GAME.replace("+ 10", "+ 30"),
                GAME.replace("+ 10", "+ 40"),
                [21, 31, 41],
            )
        };
        edit(
            &mut live,
            entity,
            path,
            module,
            &first,
            expected[0],
            RELOAD_L3,
        );
        edit(
            &mut live,
            entity,
            path,
            module,
            &second,
            expected[1],
            RELOAD_L1,
        );
        let good = live.host.modules();
        let cursor = live.reports.len();
        write(path, "this is not valid C#");
        assert!(
            live.host
                .request_compile(module, std::slice::from_ref(path), EditOrigin::Human)
        );
        wait_error(&mut live, module);
        assert_eq!(
            live.host.modules(),
            good,
            "compile failure preserves loaded generations"
        );
        assert_eq!(
            live.reports[cursor..]
                .iter()
                .filter(|report| report.kind != RELOAD_ERROR)
                .count(),
            0,
            "failed code is never applied; diagnostic reports remain retained"
        );
        assert_eq!(value(&live, entity), expected[1]);
        live.diagnostics
            .retain(|diagnostic| !is_module_error(diagnostic, module));
        edit(
            &mut live,
            entity,
            path,
            module,
            &repaired,
            expected[2],
            RELOAD_L1,
        );
        assert!(
            live.diagnostics
                .iter()
                .any(|diagnostic| is_module_error(diagnostic, "Unrelated")),
            "an unrelated saved error stays visible throughout edits and recovery"
        );
        assert_eq!(completion(&live, "Unrelated")["result"], "errors");
        assert!(
            completion(&live, "Unrelated")["errorCount"]
                .as_u64()
                .expect("error count")
                > 0
        );
        assert_eq!(live.host.component_id("Result"), Some(native_id));
        assert!(
            live.world.get_entity(entity).is_ok(),
            "native entity survives"
        );
        assert_eq!(live.finish(), 0, "published generations unload");
    }
}

#[test]
fn first_leaf_edit_of_published_watched_modules_uses_its_dependency() {
    edits(false, true, true);
}

#[test]
fn first_root_edit_of_published_watched_modules_updates_its_dependent() {
    edits(true, true, true);
}

#[test]
fn watched_leaf_can_edit_against_an_unwatched_published_dependency() {
    edits(false, false, true);
}

#[test]
fn unwatched_dependency_uses_its_supplied_image_even_when_sources_are_broken() {
    edits(false, false, false);
}
