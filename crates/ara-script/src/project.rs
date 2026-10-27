//! Shared project module/source inventory and derived-schema publication (D30).
//! Folder ownership is implemented once by the compile tool and reused by its
//! watcher, the runtime, export and migration. These functions perform I/O; call
//! them on a loading/preparation worker, never on a simulation tick.
use ara_diag::{Diagnostic, codes};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

/// One C# source, with both path forms made explicit.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceFile {
    /// Authored-root-relative reference path.
    pub path: String,
    /// Project-relative storage path.
    pub storage_path: String,
    /// Canonical module/assembly name.
    pub module: String,
    /// Folder-derived editor domain; never eligible for a player build.
    pub editor: bool,
}

/// A module's sources and explicit private build outputs.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectModule {
    /// Canonical module/assembly name.
    pub name: String,
    /// Project-relative source boundary directory.
    pub directory: String,
    /// Initial hot-reload setting.
    pub hot: bool,
    /// Canonically spelled dependencies; unknown names and cycles are diagnostics.
    pub references: Vec<String>,
    /// Folder-derived gameplay/editor domain.
    pub profile: crate::ModuleProfile,
    /// Project-relative private build-state directory.
    pub state_path: String,
    /// Project-relative generated-schema path; never an authored asset.
    pub schema_path: String,
    /// Project-relative compiled assembly path.
    pub assembly_path: String,
    /// Deterministically ordered, exclusive source membership.
    pub sources: Vec<SourceFile>,
    /// Generated schema from this module's compilation/publication.
    pub schema: Option<Value>,
    /// Compilation/publication result: unbuilt, compiled, cached or failed.
    pub result: String,
    /// Input/tool/dependency identity used to validate the private publication cache.
    pub fingerprint: Option<String>,
}

/// Inventory plus all coded configuration/compilation diagnostics. Invalid modules
/// remain visible here; callers must not silently drop errors and ship a partial graph.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Inventory {
    /// Modules ordered by canonical name.
    pub modules: Vec<ProjectModule>,
    /// Sources ordered by reference path, each assigned exactly once.
    pub sources: Vec<SourceFile>,
    /// Manifest, dependency and compiler diagnostics.
    pub diagnostics: Vec<Diagnostic>,
}

/// Private module input contract consumed by the runtime and compile watcher.
/// Source membership is refreshed through the authoritative project inventory.
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ModuleInputs {
    pub project: String,
    pub authoring_root: String,
    pub name: String,
    pub hot: bool,
    pub references: Vec<String>,
    pub profile: crate::ModuleProfile,
    pub assembly_path: String,
    #[serde(default)]
    pub sources: Vec<SourceFile>,
}

/// Installed SDK tool locations shared by project inventory and gameplay hosting.
#[derive(Debug, Clone)]
pub struct ToolPaths {
    /// Ara.Host output directory.
    pub host_dir: PathBuf,
    /// The out-of-process compiler/inventory tool.
    pub compile_server: PathBuf,
}

impl ToolPaths {
    /// Installed tools first, explicit artifact override next, checkout artifacts last.
    pub fn locate(project: &Path) -> Option<Self> {
        let exe = std::env::current_exe().ok();
        let installed = exe
            .as_deref()
            .and_then(Path::parent)
            .map(|p| p.join("dotnet/artifacts/bin"));
        let override_path = std::env::var_os("ARA_DOTNET_ARTIFACTS").map(PathBuf::from);
        let packaged = exe
            .as_deref()
            .and_then(Path::parent)
            .is_some_and(|dir| dir.join("engine.json").is_file());
        let checkout = project
            .ancestors()
            .chain(exe.as_deref().into_iter().flat_map(Path::ancestors))
            .filter(|_| !packaged)
            .map(|p| p.join("dotnet/artifacts/bin"));
        installed
            .into_iter()
            .chain(override_path)
            .chain(checkout)
            .find_map(|bin| {
                let bin = std::fs::canonicalize(bin).ok()?;
                let compile_server = bin.join("Ara.CompileServer/release/Ara.CompileServer.dll");
                let host_dir = bin.join("Ara.Host/release");
                (compile_server.is_file() && host_dir.join("Ara.Host.dll").is_file()).then_some(
                    Self {
                        host_dir,
                        compile_server,
                    },
                )
            })
    }
}

/// Return the authoritative module/source inventory under `ara_doc::AUTHORING_ROOT`.
/// No schemas are inferred by scanning `.ara`; every output path comes from its module.
pub fn inventory(root: &Path) -> Result<Inventory, Diagnostic> {
    let root = &std::fs::canonicalize(root).map_err(io)?;
    if !source_hint(&root.join(ara_doc::AUTHORING_ROOT), true)? {
        return Ok(Inventory::default());
    }
    invoke(root, "--inventory", None)
}

/// Compile gameplay (or editor plus its gameplay dependencies), returning explicit
/// schema results. Compiler errors stay in `diagnostics`, so an editor can display
/// them while keeping the project open; check/export must reject error results.
pub fn compile(root: &Path, domain: crate::ModuleProfile) -> Result<Inventory, Diagnostic> {
    let root = &std::fs::canonicalize(root).map_err(io)?;
    compile_canonical(root, domain)
}

fn compile_canonical(root: &Path, domain: crate::ModuleProfile) -> Result<Inventory, Diagnostic> {
    if !source_hint(&root.join(ara_doc::AUTHORING_ROOT), true)? {
        return Ok(Inventory::default());
    }
    invoke(root, "--compile-project", Some(domain))
}

/// Compile and canonically publish generated schemas into the module's private
/// output directory before scenes are validated. Returns paths and results even
/// when compilation has coded diagnostics; generated outputs never enter game/.
pub fn publish(root: &Path, domain: crate::ModuleProfile) -> Result<Inventory, Diagnostic> {
    let root = &std::fs::canonicalize(root).map_err(io)?;
    let mut inventory = compile_canonical(root, domain)?;
    for module in &inventory.modules {
        let Some(schema) = &module.schema else {
            continue;
        };
        let path = checked_output(root, &module.schema_path)?;
        let text = ara_json::to_canonical_string(schema);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io)?;
            }
            let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
            std::fs::write(&temporary, text).map_err(io)?;
            std::fs::rename(&temporary, &path).map_err(io)?;
        }
        if let Some(fingerprint) = &module.fingerprint {
            let assembly = checked_output(root, &module.assembly_path)?;
            let marker = format!(
                "{fingerprint}\n{}\n{}\n",
                ara_doc::file_state::FileState::bytes(&std::fs::read(&path).map_err(io)?).hash,
                ara_doc::file_state::FileState::bytes(&std::fs::read(assembly).map_err(io)?).hash
            );
            let marker_path =
                checked_output(root, &format!("{}/published.sha256", module.state_path))?;
            if std::fs::read_to_string(&marker_path).ok().as_deref() != Some(marker.as_str()) {
                std::fs::write(marker_path, marker).map_err(io)?;
            }
        }
        let inputs = ModuleInputs {
            project: root.to_string_lossy().into_owned(),
            authoring_root: ara_doc::AUTHORING_ROOT.into(),
            name: module.name.clone(),
            hot: module.hot,
            references: module.references.clone(),
            profile: module.profile,
            assembly_path: module.assembly_path.clone(),
            sources: module.sources.clone(),
        };
        let path = checked_output(root, &format!("{}/module.inputs.json", module.state_path))?;
        write_changed(
            &path,
            &ara_json::to_canonical_string(&serde_json::to_value(inputs).map_err(io)?),
        )?;
        let tools =
            ToolPaths::locate(root).ok_or_else(|| io("C# SDK disappeared during publication"))?;
        let xml = |s: &str| {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('"', "&quot;")
        };
        let sources = module
            .sources
            .iter()
            .map(|s| {
                format!(
                    "<Compile Include=\"{}\" />",
                    xml(&root.join(&s.storage_path).to_string_lossy())
                )
            })
            .collect::<String>();
        let references = module
            .references
            .iter()
            .map(|name| {
                format!(
                    "<ProjectReference Include=\"../{0}/{0}.csproj\" />",
                    xml(name)
                )
            })
            .collect::<String>();
        let runtime = tools.host_dir.join("Ara.Runtime.dll");
        let compiler_dir = tools
            .compile_server
            .parent()
            .ok_or_else(|| io("C# compiler directory missing"))?;
        let project = format!(
            "<Project Sdk=\"Microsoft.NET.Sdk\"><PropertyGroup><TargetFramework>net10.0</TargetFramework><LangVersion>14</LangVersion><AllowUnsafeBlocks>true</AllowUnsafeBlocks><Nullable>enable</Nullable><ImplicitUsings>enable</ImplicitUsings><EnableDefaultCompileItems>false</EnableDefaultCompileItems><AssemblyName>{}</AssemblyName><AraModuleProfile>{}</AraModuleProfile></PropertyGroup><ItemGroup>{sources}{references}<Using Include=\"Ara\" /><Using Include=\"System.Numerics\" /><Reference Include=\"Ara.Runtime\"><HintPath>{}</HintPath></Reference><Analyzer Include=\"{}\" /><Analyzer Include=\"{}\" /><CompilerVisibleProperty Include=\"AraModuleProfile\" /></ItemGroup></Project>\n",
            xml(&module.name),
            module.profile.as_str(),
            xml(&runtime.to_string_lossy()),
            xml(&compiler_dir.join("Ara.Generators.dll").to_string_lossy()),
            xml(&compiler_dir.join("Ara.Analyzers.dll").to_string_lossy())
        );
        write_changed(
            &checked_output(
                root,
                &format!("{}/{}.csproj", module.state_path, module.name),
            )?,
            &project,
        )?;
    }
    inventory.diagnostics.sort_by(|a, b| {
        (a.location.file.as_deref(), &a.message).cmp(&(b.location.file.as_deref(), &b.message))
    });
    Ok(inventory)
}

fn write_changed(path: &Path, text: &str) -> Result<(), Diagnostic> {
    if std::fs::read_to_string(path).ok().as_deref() != Some(text) {
        std::fs::write(path, text).map_err(io)?;
    }
    Ok(())
}

impl Inventory {
    /// Runtime module descriptors, excluding editor assemblies and refusing any
    /// error in the graph rather than returning a partially loadable player.
    pub fn gameplay_specs(&self, root: &Path) -> Result<Vec<crate::ModuleSpec>, Vec<Diagnostic>> {
        let root = &std::fs::canonicalize(root).map_err(|error| vec![io(error)])?;
        let errors: Vec<_> = self
            .diagnostics
            .iter()
            .filter(|d| d.severity == ara_diag::Severity::Error)
            .cloned()
            .collect();
        if !errors.is_empty() {
            return Err(errors);
        }
        self.modules
            .iter()
            .filter(|m| m.profile == crate::ModuleProfile::Gameplay)
            .map(|m| crate::ModuleSpec::from_dir(&root.join(&m.state_path)).map_err(|d| vec![d]))
            .collect()
    }
}

fn io(error: impl std::fmt::Display) -> Diagnostic {
    Diagnostic::error(codes::IO, error.to_string())
}

/// Cheap empty-project guard, not an ownership inventory. It lets non-C# projects
/// work from a fresh Rust checkout before the optional C# tool build exists.
fn source_hint(directory: &Path, root: bool) -> Result<bool, Diagnostic> {
    if !directory.is_dir()
        || std::fs::symlink_metadata(directory)
            .map_err(io)?
            .file_type()
            .is_symlink()
        || (!root && directory.join("ara.project.json").is_file())
    {
        return Ok(false);
    }
    for entry in std::fs::read_dir(directory).map_err(io)? {
        let entry = entry.map_err(io)?;
        let kind = entry.file_type().map_err(io)?;
        if kind.is_symlink() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if kind.is_dir() {
            if ![".ara", ".git"].contains(&name.as_str()) && source_hint(&entry.path(), false)? {
                return Ok(true);
            }
        } else if name.to_ascii_lowercase().ends_with(".cs")
            || name.to_ascii_lowercase().ends_with(".module.json")
            || name == "ara.module.toml"
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn checked_output(root: &Path, relative: &str) -> Result<PathBuf, Diagnostic> {
    if !relative.starts_with(".ara/scripts/")
        || Path::new(relative)
            .components()
            .any(|p| !matches!(p, std::path::Component::Normal(_)))
    {
        return Err(Diagnostic::error(
            codes::INVALID_PARAMS,
            "script output must stay under .ara/scripts",
        ));
    }
    Ok(root.join(relative))
}

fn invoke(
    root: &Path,
    mode: &str,
    domain: Option<crate::ModuleProfile>,
) -> Result<Inventory, Diagnostic> {
    let tools = ToolPaths::locate(root).ok_or_else(|| Diagnostic::error(crate::codes::COMPILE_SERVER, "the installed C# compile tools are missing; build dotnet/Ara.slnx or repair the engine SDK"))?;
    let mut command = Command::new("dotnet");
    command
        .args(["exec"])
        .arg(&tools.compile_server)
        .arg(mode)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("DOTNET_MODIFIABLE_ASSEMBLIES");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000 | 0x0000_4000); // no window, BelowNormal
    }
    let mut child = command.spawn().map_err(io)?;
    let request = json!({"project":root, "authoringRoot":ara_doc::AUTHORING_ROOT, "domain":domain.map(crate::ModuleProfile::as_str)});
    let bytes = serde_json::to_vec(&request).map_err(io)?;
    child
        .stdin
        .take()
        .ok_or_else(|| io("C# tool has no input pipe"))?
        .write_all(&bytes)
        .map_err(io)?;
    let output = child.wait_with_output().map_err(io)?;
    if !output.status.success() {
        return Err(Diagnostic::error(
            crate::codes::COMPILE_SERVER,
            String::from_utf8_lossy(&output.stderr).trim().to_owned(),
        ));
    }
    serde_json::from_slice(&output.stdout).map_err(|error| {
        Diagnostic::error(
            crate::codes::COMPILE_SERVER,
            format!("invalid module inventory: {error}"),
        )
    })
}
