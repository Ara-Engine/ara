//! C# from the editor: Create > C# Behaviour scaffolds a hot-reloading module (the
//! project's first one, or the existing one) with a template behaviour and an IDE
//! project, then opens it in the IDE. Ara compiles modules itself; Play publishes
//! their types and saving a script while playing hot-reloads it (see Console).
use std::path::{Path, PathBuf};

use super::*;

/// The project's C# modules (folders with `ara.module.toml`), sorted.
pub(crate) fn modules(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if !path.is_dir() || name.starts_with('.') || matches!(name.as_str(), "bin" | "obj") {
                continue;
            }
            if path.join("ara.module.toml").is_file() {
                out.push(path);
            } else {
                walk(&path, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

/// "outpost sample" → "OutpostSample".
fn pascal(text: &str) -> String {
    let name: String = text
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect();
    match name.chars().next() {
        Some(c) if c.is_ascii_alphabetic() => name,
        _ => format!("Game{name}"),
    }
}

/// A template behaviour: every reflected member has a summary (ARA0703).
fn behaviour(module: &str, name: &str) -> String {
    format!(
        r#"namespace {module};

/// <summary>Spins its entity around the Y axis. Edit while playing: saving hot-reloads it.</summary>
public sealed partial class {name} : Behaviour
{{
    /// <summary>Turn speed.</summary>
    [Property, Unit(Units.Degrees)] public partial float Speed {{ get; set; }}

    /// <inheritdoc/>
    protected override void OnTick(in Tick t)
    {{
        ref var transform = ref Get<Transform>();
        transform.Rotation.Y += (Speed == 0 ? 90f : Speed) * t.Dt;
    }}
}}
"#
    )
}

/// An SDK project for IDEs only (completion, navigation); Ara builds the module.
fn ide_project(module: &str, runtime: Option<&Path>) -> String {
    let reference = runtime
        .map(|dir| {
            format!(
                "\n  <ItemGroup>\n    <Reference Include=\"Ara.Runtime\"><HintPath>{}</HintPath></Reference>\n  </ItemGroup>",
                dir.join("Ara.Runtime.dll").display()
            )
        })
        .unwrap_or_default();
    format!(
        "<!-- For IDEs only: Ara compiles and hot-reloads the {module} module itself. -->\n<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n    <Nullable>enable</Nullable>\n    <AssemblyName>{module}</AssemblyName>\n  </PropertyGroup>\n  <ItemGroup>\n    <Using Include=\"System\" />\n    <Using Include=\"System.Collections.Generic\" />\n    <Using Include=\"System.Linq\" />\n    <Using Include=\"System.Numerics\" />\n    <Using Include=\"Ara\" />\n  </ItemGroup>{reference}\n</Project>\n"
    )
}

/// Opens a folder and file in VS Code when it is installed, else the file in the OS
/// default editor.
pub(crate) fn open_in_ide(folder: &Path, file: &Path) -> ara_ui::Result<()> {
    let code = if cfg!(windows) { "code.cmd" } else { "code" };
    if std::process::Command::new(code)
        .arg(folder)
        .arg(file)
        .spawn()
        .is_ok()
    {
        return Ok(());
    }
    ops::open_external(file)
}

impl Shell {
    /// Create > C# Behaviour: scaffold (or reuse) the module, add a template behaviour
    /// and open it in the IDE.
    pub(crate) fn create_behaviour(&mut self, editor: &mut Editor) -> ara_ui::Result<()> {
        let root = editor.host.root().to_owned();
        let module_dir = match modules(&root).into_iter().next() {
            Some(dir) => dir,
            None => {
                let project = std::fs::read_to_string(root.join("ara.project.json"))
                    .ok()
                    .and_then(|t| serde_json::from_str::<Value>(&t).ok())
                    .and_then(|v| v["name"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| "Game".into());
                let name = pascal(&project);
                let dir = root.join("scripts").join(&name);
                std::fs::create_dir_all(&dir)?;
                std::fs::write(
                    dir.join("ara.module.toml"),
                    format!(
                        "# Gameplay scripts, compiled and hot-reloaded by Ara while playing.\n[module]\nname = \"{name}\"\nhot = true\n"
                    ),
                )?;
                dir
            }
        };
        let manifest = std::fs::read_to_string(module_dir.join("ara.module.toml"))?;
        let module = manifest
            .lines()
            .find_map(|l| l.trim().strip_prefix("name"))
            .and_then(|l| l.split('"').nth(1))
            .map(str::to_owned)
            .ok_or("ara.module.toml has no module name")?;
        let name = (1..)
            .map(|n| {
                if n == 1 {
                    "Spinner".to_owned()
                } else {
                    format!("Spinner{n}")
                }
            })
            .find(|n| !module_dir.join(format!("{n}.cs")).exists())
            .unwrap_or_else(|| "Spinner".into());
        let file = module_dir.join(format!("{name}.cs"));
        std::fs::write(&file, behaviour(&module, &name))?;
        let project = module_dir.join(format!("{module}.csproj"));
        if !project.exists() {
            let runtime = ara_play_host::ScriptPaths::locate(&root)
                .or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .and_then(|e| e.parent().map(Path::to_path_buf))
                        .and_then(|dir| ara_play_host::ScriptPaths::locate(&dir))
                })
                .map(|p| p.host_dir);
            std::fs::write(&project, ide_project(&module, runtime.as_deref()))?;
        }
        open_in_ide(&module_dir, &file)?;
        self.status_note = Some(format!(
            "Created {name}.cs: press Play to compile it, then add the {name} component"
        ));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_and_behaviour_names_are_csharp_identifiers() {
        assert_eq!(pascal("outpost sample"), "OutpostSample");
        assert_eq!(pascal("3d-demo"), "Game3dDemo");
        let source = behaviour("Outpost", "Spinner");
        assert!(source.contains("public sealed partial class Spinner : Behaviour"));
        assert!(source.contains("/// <summary>Turn speed.</summary>"));
    }
}
