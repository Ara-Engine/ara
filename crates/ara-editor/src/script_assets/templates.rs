//! Templates use explicit Ara imports, independent of IDE implicit-usings settings.
use std::path::Path;

/// The gameplay code pattern scaffolded by script.create.
#[derive(Clone, Copy, Default, ara_schema::Schema, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub(super) enum Template {
    /// A per-entity Behaviour with reflected ECS properties.
    #[default]
    Behaviour,
    /// A static System with a batched component query.
    System,
}

pub(super) fn identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && ![
            "class",
            "namespace",
            "public",
            "private",
            "static",
            "void",
            "int",
            "float",
            "string",
            "System",
            "Ara",
        ]
        .contains(&name)
}

pub(super) fn module_name(root: &Path, folder: &Path) -> String {
    let project = std::fs::read_to_string(root.join("ara.project.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v["name"].as_str().map(str::to_owned))
        .unwrap_or_else(|| "Game".into());
    let suffix = folder
        .strip_prefix(root)
        .unwrap_or(folder)
        .to_string_lossy();
    let name = format!("{project} {suffix}")
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|p| !p.is_empty())
        .map(|p| {
            let mut c = p.chars();
            c.next()
                .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                .unwrap_or_default()
        })
        .collect::<String>();
    if identifier(&name) {
        name
    } else {
        format!("Game{name}")
    }
}

pub(super) fn source(namespace: &str, name: &str, template: Template) -> String {
    let body = match template {
        Template::Behaviour => format!(
            "/// <summary>Gameplay behaviour for its entity. State lives in ECS properties.</summary>\npublic sealed partial class {name} : Behaviour\n{{\n    /// <summary>Turn speed in degrees per second.</summary>\n    [Property, Unit(Units.Degrees)] public partial float Speed {{ get; set; }}\n\n    /// <inheritdoc/>\n    protected override void OnTick(in Tick tick)\n    {{\n        ref var transform = ref Get<Transform>();\n        transform.Rotation.Y += Speed * tick.Dt;\n    }}\n}}\n"
        ),
        Template::System => format!(
            "/// <summary>Batched gameplay logic over matching ECS components.</summary>\npublic static partial class {name}\n{{\n    /// <summary>Update each matching entity on the fixed simulation tick.</summary>\n    [Query]\n    static void Update(ref Transform transform)\n    {{\n        // Read and write component state here.\n    }}\n}}\n"
        ),
    };
    format!("using Ara;\n\nnamespace {namespace};\n\n{body}")
}

pub(super) fn ide_project(namespace: &str, runtime: Option<&Path>) -> String {
    let escape = |s: String| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let reference = runtime.map(|dir| {
        let path = escape(dir.join("Ara.Runtime.dll").display().to_string());
        format!("<ItemGroup><Reference Include=\"Ara.Runtime\"><HintPath>{path}</HintPath></Reference></ItemGroup>")
    }).unwrap_or_default();
    let generator = runtime
        .and_then(Path::parent)
        .and_then(Path::parent)
        .map(|dir| dir.join("Ara.CompileServer/release/Ara.Generators.dll"))
        .filter(|path| path.is_file())
        .map(|path| {
            let path = escape(path.display().to_string());
            format!("<ItemGroup><Analyzer Include=\"{path}\" /></ItemGroup>")
        })
        .unwrap_or_default();
    format!(
        "<!-- Ara compiles this module; this project provides IDE completion. -->\n<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework><Nullable>enable</Nullable><AllowUnsafeBlocks>true</AllowUnsafeBlocks><AssemblyName>{namespace}</AssemblyName></PropertyGroup>{reference}{generator}</Project>\n"
    )
}
