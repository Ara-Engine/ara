//! Benchmark-only module wiring; the cold kernel and editable wrapper share ECS columns.
use super::{source, support};
use std::error::Error;

/// Loads either the original single module or a wrapper referencing a cold kernel.
pub fn start(
    workload: &str,
    mode: &str,
    hot: bool,
    kernel_module: bool,
) -> Result<support::Live, Box<dyn Error>> {
    let (root, names) = if kernel_module {
        let (kernel, game) = source::split_module(workload, mode);
        (
            support::Live::write(
                "ecs-kernel-module-bench",
                &[("Kernel", &kernel, &[]), ("Game", &game, &["Kernel"])],
            ),
            vec!["Kernel", "Game"],
        )
    } else {
        let game = source::module(workload, mode);
        (
            support::Live::write("ecs-kernel-bench", &[("Game", &game, &[])]),
            vec!["Game"],
        )
    };
    for name in &names {
        if *name == "Kernel" || !hot {
            let path = root.join(name).join("ara.module.toml");
            let contents = std::fs::read_to_string(&path)?;
            std::fs::write(path, contents.replace("hot = true", "hot = false"))?;
        }
    }
    let mut live = support::Live::host(root);
    let modules = names
        .iter()
        .map(|name| ara_script::ModuleSpec::from_dir(&live.root.join(name)))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|diagnostic| format!("module spec: {diagnostic:?}"))?;
    live.host
        .load_modules(&mut live.world, &modules)
        .map_err(|diagnostics| format!("module compilation: {diagnostics:?}"))?;
    for module in live.host.modules() {
        assert_eq!(
            module.hot,
            module.name == "Game" && hot,
            "actual module heat"
        );
        assert_eq!(
            module.counts,
            (usize::from(module.name == "Game"), 0, 0),
            "only Game owns one scheduled query"
        );
    }
    assert_eq!(live.host.modules().len(), names.len());
    Ok(live)
}
