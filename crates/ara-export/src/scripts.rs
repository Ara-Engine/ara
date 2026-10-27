//! NativeAOT shipping for real Ara gameplay modules (A12/A13), separate from S7's fixture ABI.
use crate::tools::{self, Toolchain};
use anyhow::{Context, Result, ensure};
use ara_script::{ModuleProfile, ModuleSpec, shipping::ScriptManifest};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

/// Module graph and generated build project. All author paths stay in the build job.
pub struct ShippingProject {
    /// Generated project linking every gameplay module in dependency order.
    pub project: PathBuf,
    /// Linked module inventory used to validate the exported image.
    pub modules: Vec<String>,
}

fn xml(value: &str) -> String {
    quick_xml::escape::escape(value).into_owned()
}
fn path_xml(path: &Path) -> String {
    xml(&path.to_string_lossy())
}

/// Finds module folders, excluding build outputs and editor-only modules.
pub fn modules(root: &Path) -> Result<Vec<ModuleSpec>> {
    let mut specs = ara_play_host::script_modules(root)
        .into_iter()
        .map(|path| ModuleSpec::from_dir(&path).map_err(|e| anyhow::anyhow!("{e}")))
        .collect::<Result<Vec<_>>>()?;
    specs.retain(|spec| spec.profile == ModuleProfile::Gameplay);
    let mut names = std::collections::BTreeSet::new();
    for spec in &specs {
        ensure!(
            names.insert(&spec.name),
            "ARA0920: duplicate gameplay module `{}`",
            spec.name
        );
        ensure!(
            !spec.name.is_empty()
                && spec
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_')),
            "ARA0920: invalid gameplay module name `{}`",
            spec.name
        );
    }
    let (order, errors) = ara_script::load_order(&specs);
    ensure!(
        errors.is_empty(),
        "{}",
        errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n")
    );
    Ok(order.into_iter().map(|i| specs[i].clone()).collect())
}

/// Generates isolated module projects and one NativeAOT entry project. The SDK's
/// Runtime, generator and analyzers are build inputs; none are shipped as tools.
pub fn prepare(
    project: &Path,
    assembly: &str,
    job: &Path,
    tools: &Toolchain,
) -> Result<Option<ShippingProject>> {
    let specs = modules(project)?;
    if specs.is_empty() {
        return Ok(None);
    }
    ensure!(
        !assembly.is_empty()
            && assembly
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "ARA0920: invalid shipping assembly stem"
    );
    let sdk = std::env::var_os("ARA_DOTNET_SDK")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dotnet"));
    let bin = std::env::var_os("ARA_DOTNET_ARTIFACTS")
        .map(PathBuf::from)
        .or_else(|| {
            let installed = std::env::current_exe()
                .ok()?
                .parent()?
                .join("dotnet/artifacts/bin");
            installed
                .join("Ara.Runtime/release/Ara.Runtime.dll")
                .is_file()
                .then_some(installed)
        })
        .unwrap_or_else(|| sdk.join("artifacts/bin"));
    let bin = std::path::absolute(bin)?;
    let artifacts = ["Ara.Runtime", "Ara.Generators", "Ara.Analyzers"];
    if artifacts.iter().any(|name| {
        !bin.join(name)
            .join("release")
            .join(format!("{name}.dll"))
            .is_file()
    }) {
        ensure!(
            sdk.join("Ara.slnx").is_file(),
            "ARA0922: shipping C# SDK is missing; run `ara doctor`, install the Ara .NET SDK or set ARA_DOTNET_SDK/ARA_DOTNET_ARTIFACTS"
        );
        tools::run(
            Command::new("dotnet")
                .args([
                    "build",
                    "Ara.slnx",
                    "-c",
                    "Release",
                    "--nologo",
                    "--disable-build-servers",
                ])
                .current_dir(&sdk)
                .env("PATH", tools.path()?),
            &job.join("logs/script-sdk.log"),
            Duration::from_secs(300),
        )?;
    }
    let generated = std::path::absolute(job.join("shipping-project"))?;
    std::fs::create_dir_all(&generated)?;
    // Do not inherit project/worktree build props: this job owns every output path.
    std::fs::write(generated.join("Directory.Build.props"), "<Project />\n")?;
    std::fs::write(generated.join("Directory.Build.targets"), "<Project />\n")?;
    let runtime = bin.join("Ara.Runtime/release/Ara.Runtime.dll");
    let generator = bin.join("Ara.Generators/release/Ara.Generators.dll");
    let analyzer = bin.join("Ara.Analyzers/release/Ara.Analyzers.dll");
    let common = format!(
        r#"<PropertyGroup><TargetFramework>net10.0</TargetFramework><LangVersion>14</LangVersion><AllowUnsafeBlocks>true</AllowUnsafeBlocks><Nullable>enable</Nullable><ImplicitUsings>enable</ImplicitUsings><EnableDefaultCompileItems>false</EnableDefaultCompileItems><TreatWarningsAsErrors>true</TreatWarningsAsErrors><IsAotCompatible>true</IsAotCompatible><Deterministic>true</Deterministic><AraModuleProfile>gameplay</AraModuleProfile></PropertyGroup><ItemGroup><Reference Include="Ara.Runtime"><HintPath>{}</HintPath></Reference><Analyzer Include="{}" /><Analyzer Include="{}" /><Using Include="Ara" /><Using Include="System.Numerics" /><CompilerVisibleProperty Include="AraModuleProfile" /></ItemGroup>"#,
        path_xml(&runtime),
        path_xml(&generator),
        path_xml(&analyzer)
    );
    let projects: BTreeMap<_, _> = specs
        .iter()
        .enumerate()
        .map(|(i, spec)| {
            (
                spec.name.as_str(),
                generated.join(format!("module-{i}/Module.csproj")),
            )
        })
        .collect();
    for spec in &specs {
        let csproj = &projects[spec.name.as_str()];
        std::fs::create_dir_all(csproj.parent().context("module project directory")?)?;
        let source = dunce::canonicalize(&spec.dir)?;
        let includes = source_items(&source, &spec.dir, &ara_play_host::script_modules(project));
        let refs = spec
            .references
            .iter()
            .map(|name| {
                format!(
                    r#"<ProjectReference Include="{}" />"#,
                    path_xml(&projects[name.as_str()])
                )
            })
            .collect::<String>();
        std::fs::write(
            csproj,
            format!(
                r#"<Project Sdk="Microsoft.NET.Sdk">{common}<PropertyGroup><AssemblyName>{}</AssemblyName></PropertyGroup><ItemGroup>{includes}{refs}</ItemGroup></Project>"#,
                xml(&spec.name)
            ),
        )?;
    }
    let native = generated.join("native/Shipping.csproj");
    std::fs::create_dir_all(native.parent().context("native project directory")?)?;
    let refs = specs
        .iter()
        .map(|spec| {
            format!(
                r#"<ProjectReference Include="{}" />"#,
                path_xml(&projects[spec.name.as_str()])
            )
        })
        .collect::<String>();
    std::fs::write(
        &native,
        format!(
            r#"<Project Sdk="Microsoft.NET.Sdk">{common}<PropertyGroup><AssemblyName>{}</AssemblyName><AraNativeEntry>true</AraNativeEntry><PublishAot>true</PublishAot><NativeLib>Shared</NativeLib><InvariantGlobalization>true</InvariantGlobalization></PropertyGroup><ItemGroup><CompilerVisibleProperty Include="AraNativeEntry" />{refs}</ItemGroup></Project>"#,
            xml(assembly)
        ),
    )?;
    Ok(Some(ShippingProject {
        project: native,
        modules: specs.into_iter().map(|m| m.name).collect(),
    }))
}

/// Writes the managed crash hook as MSBuild targets in `work`: attached only to the
/// linked NativeAOT entry when `entry_only` (runtime players), else to every project.
pub fn crash_targets(work: &Path, entry_only: bool) -> Result<PathBuf> {
    let source = work.join("AraCrash.g.cs");
    std::fs::write(&source, include_str!("managed-crash.cs"))?;
    let targets = work.join("AraCrash.targets");
    let condition = if entry_only {
        " Condition=\"'$(AraNativeEntry)' == 'true'\""
    } else {
        ""
    };
    let source = source
        .to_str()
        .context("non-UTF8 managed crash source path")?;
    std::fs::write(
        &targets,
        format!(
            "<Project><ItemGroup{condition}><Compile Include=\"{}\" /></ItemGroup></Project>",
            quick_xml::escape::escape(source)
        ),
    )?;
    Ok(targets)
}

/// `dotnet publish` of a script project for `rid` into `output` with the shipping
/// properties: a NativeAOT shared library, or a self-contained ReadyToRun build.
/// Symbols stay in the image (ELF/PE builds archive and strip them), except on macOS,
/// where they go to a `.dSYM` beside it. Callers add platform toolchain properties.
pub fn publish_command(
    project: &Path,
    rid: &str,
    output: &Path,
    native_aot: bool,
    crash_targets: &Path,
    tools: &Toolchain,
) -> Result<Command> {
    let mut cmd = Command::new("dotnet");
    cmd.arg("publish")
        .arg(project)
        .args([
            "-c",
            "Release",
            "-r",
            rid,
            "--nologo",
            "--disable-build-servers",
            "-o",
        ])
        .arg(output)
        .arg(format!(
            "-p:AraLane={}",
            if native_aot { "aot" } else { "r2r" }
        ))
        .arg("-p:Deterministic=true")
        .arg("-p:InvariantGlobalization=true")
        .arg("-p:UseSharedCompilation=false")
        .arg("-p:DebugSymbols=true")
        .arg(format!("-p:StripSymbols={}", rid.starts_with("osx")))
        .arg(format!(
            "-p:CustomBeforeMicrosoftCommonTargets={}",
            crash_targets.display()
        ))
        .env("PATH", tools.path()?);
    if native_aot {
        cmd.args(["-p:PublishAot=true", "-p:NativeLib=Shared"]);
    } else {
        cmd.args([
            "-p:PublishAot=false",
            "-p:OutputType=Exe",
            "-p:SelfContained=true",
            "-p:PublishReadyToRun=true",
            "-p:PublishTrimmed=false",
        ]);
    }
    Ok(cmd)
}

/// The runtime identifier of a desktop platform's one architecture.
pub(crate) fn desktop_rid(platform: crate::Platform) -> &'static str {
    match platform {
        crate::Platform::Windows => "win-x64",
        crate::Platform::Macos => "osx-arm64",
        _ => "linux-x64",
    }
}

/// File name of `assembly`'s NativeAOT shared library on `rid`.
pub fn library_name(assembly: &str, rid: &str) -> String {
    let extension = if rid.starts_with("win") {
        "dll"
    } else if rid.starts_with("osx") || rid.starts_with("ios") {
        "dylib"
    } else {
        "so"
    };
    format!("{assembly}.{extension}")
}

/// Builds a project's shipping NativeAOT image for `rid` and puts only the image and
/// its `scripts.json` in `output`; the job directory keeps generated projects, the
/// publish output (symbols included) and logs. Returns the
/// image, or `None` for a project without gameplay modules. This is the host-native
/// lane for platforms the export pipeline does not build yet (macOS players).
pub fn build(
    project: &Path,
    assembly: &str,
    rid: &str,
    output: &Path,
    job: &Path,
    tools: &Toolchain,
) -> Result<Option<PathBuf>> {
    let Some(shipping) = prepare(project, assembly, job, tools)? else {
        return Ok(None);
    };
    std::fs::create_dir_all(job.join("logs"))?;
    let targets = crash_targets(job, true)?;
    let published = job.join("publish").join(rid);
    let mut cmd = publish_command(&shipping.project, rid, &published, true, &targets, tools)?;
    tools::run(
        &mut cmd,
        &job.join(format!("logs/script-{rid}.log")),
        Duration::from_secs(600),
    )?;
    let library = library_name(assembly, rid);
    std::fs::create_dir_all(output)?;
    tools::copy(&published.join(&library), &output.join(&library))?;
    write_manifest(output, &library, &shipping.modules)?;
    Ok(Some(output.join(library)))
}

/// Writes portable metadata next to a published image. The native initializer also
/// checks the compiled binding identity, so stale SDK binaries cannot bypass it.
pub fn write_manifest(output: &Path, library: &str, modules: &[String]) -> Result<()> {
    let mut db = ara_schema::SchemaDb::new();
    ara_components::register(&mut db);
    let bindings = ara_script::engine_bindings();
    let layouts = ara_script::csgen::engine_layouts(&db, &bindings.names())
        .map_err(|d| anyhow::anyhow!("{d}"))?;
    let manifest = ScriptManifest::new(
        library.into(),
        modules.to_vec(),
        ara_script::abi::layout_hash(&layouts),
    );
    tools::write_json(
        &output.join("scripts.json"),
        &serde_json::to_value(manifest)?,
    )
}

/// The shipping compiler respects the same nested-module boundaries as Roslyn.
fn source_items(source: &Path, module: &Path, roots: &[PathBuf]) -> String {
    let mut excludes = [
        "bin",
        "obj",
        "target",
        "artifacts",
        "node_modules",
        "spikes",
        ".*",
    ]
    .into_iter()
    .map(|dir| format!("{}/**/{dir}/**", path_xml(source)))
    .collect::<Vec<_>>();
    excludes.extend(
        roots
            .iter()
            .filter(|root| root.as_path() != module)
            .filter_map(|root| root.strip_prefix(module).ok())
            .map(|relative| format!("{}/**", path_xml(&source.join(relative)))),
    );
    format!(
        r#"<Compile Include="{}/**/*.cs" Exclude="{}" />"#,
        path_xml(source),
        excludes.join(";")
    )
}

#[cfg(test)]
mod source_tests {
    use super::*;
    #[test]
    fn shipping_sources_exclude_nested_modules_and_generated_code() {
        let module = Path::new("/game");
        let roots = vec![module.to_owned(), module.join("Gameplay")];
        let xml = source_items(module, module, &roots);
        assert!(xml.contains(&format!("{}/**", path_xml(&module.join("Gameplay")))));
        assert!(xml.contains("/game/**/obj/**"));
        assert!(xml.contains("/game/**/.*/**"));
        assert!(!xml.contains(";/game/**;"));
    }
    #[test]
    fn script_images_take_each_platforms_shared_library_name() {
        assert_eq!(library_name("Outpost", "win-x64"), "Outpost.dll");
        assert_eq!(library_name("Outpost", "osx-arm64"), "Outpost.dylib");
        assert_eq!(library_name("Outpost", "linux-x64"), "Outpost.so");
        assert_eq!(library_name("Outpost", "android-arm64"), "Outpost.so");
    }
}
