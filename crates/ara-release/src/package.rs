use crate::{
    archive, notices,
    support::{self, copy, copy_tree, files, run},
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const BINARIES: &[(&str, &str)] = &[
    ("ara", "ara"),
    ("ara-editor", "ara-editor"),
    ("ara-player", "ara-player"),
    ("ara-import-worker", "ara-import-worker"),
    ("ara-nav", "ara-nav-worker"),
    ("ara-terrain", "ara-terrain-worker"),
    ("ara-shader-cook", "ara-shader-cook"),
    ("ara-release", "ara-setup"),
];
const RESOURCES: &[&str] = &[
    "ara-render",
    "ara-render-fidelity",
    "ara-runtime-render",
    "ara-render2d",
    "ara-landscape",
    "ara-vfx-gpu",
];

pub fn package(root: &Path, version: &str) -> Result<()> {
    ensure!(
        cfg!(all(windows, target_arch = "x86_64")),
        "engine packaging currently requires Windows x64"
    );
    let dist = support::dist(root);
    fs::create_dir_all(dist.join("logs"))?;
    let name = support::name(version);
    let output = dist.join(&name);
    ensure!(
        !output.exists(),
        "{} already exists; move the previous distribution before packaging again",
        output.display()
    );
    let sdk = sdk()?;
    let mut build = Command::new("cargo");
    build
        .current_dir(root)
        .env("CARGO_BUILD_JOBS", "2")
        .env("CARGO_PROFILE_RELEASE_DEBUG", "line-tables-only")
        .env("VULKAN_SDK", &sdk)
        .args(["build", "--release"]);
    for (package, binary) in BINARIES {
        build.args(["-p", package, "--bin", binary]);
    }
    // A Hub UI package can land independently; no fake Hub executable is shipped.
    let hub = root.join("crates/ara-hub/Cargo.toml").is_file();
    if hub {
        build.args(["-p", "ara-hub", "--bin", "ara-hub"]);
    }
    run(&mut build, &dist.join("logs/release-build.log"))?;
    let stage = dist.join(format!(".stage-{name}-{}", std::process::id()));
    fs::create_dir(&stage)?;
    let symbols = dist.join(format!("{name}-symbols"));
    fs::create_dir_all(&symbols)?;
    let mut identities = Vec::new();
    let binaries = BINARIES
        .iter()
        .map(|(_, binary)| *binary)
        .chain(hub.then_some("ara-hub"));
    for binary in binaries {
        let source = root.join("target/release").join(format!("{binary}.exe"));
        if binary != "ara-setup" {
            copy(&source, &stage.join(format!("{binary}.exe")))?;
        }
        let pdb = root
            .join("target/release")
            .join(format!("{}.pdb", binary.replace('-', "_")));
        ensure!(
            pdb.is_file(),
            "release PDB missing for {binary}; symbols must match this build"
        );
        copy(
            &pdb,
            &symbols.join(pdb.file_name().context("PDB filename")?),
        )?;
        copy(
            &source,
            &symbols.join(format!("{}.exe", binary.replace('-', "_"))),
        )?;
        identities.push(json!({"binary":format!("{binary}.exe"), "blake3":blake3::hash(&fs::read(&source)?).to_string(), "pdb":pdb.file_name().map(|name| name.to_string_lossy())}));
    }
    for entry in fs::read_dir(root.join("target/release"))? {
        let path = entry?.path();
        if path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
        {
            copy(
                &path,
                &stage.join(path.file_name().context("DLL filename")?),
            )?;
        }
    }
    for package in RESOURCES {
        copy_tree(
            &root.join("crates").join(package).join("cooked"),
            &stage.join("resources").join(package),
        )?;
    }
    copy_tree(&root.join("assets/brand"), &stage.join("assets/brand"))?;
    for file in [
        "install.ps1",
        "uninstall.ps1",
        "launch-hub.ps1",
        "launch-editor.ps1",
        "README-FIRST.txt",
        "LICENSE",
    ] {
        copy(&root.join("release").join(file), &stage.join(file))?;
    }
    bundle_tools(root, &stage, &sdk)?;
    bundle_dotnet(root, &stage, &symbols, &dist)?;
    templates(root, &stage, &dist)?;
    notices::generate(root, &stage, &dist)?;
    support::json(
        &stage.join("engine.json"),
        &json!({"format":"ara.engine/1","version":version,"platform":"windows-x64","hub":{"binary":"ara-hub.exe","available":hub},"sourceCommit":git(root, &["rev-parse","HEAD"])? .trim(),"crashes":"%LOCALAPPDATA%/Ara/crashes"}),
    )?;
    support::json(
        &symbols.join("builds.json"),
        &json!({"version":version,"binaries":identities}),
    )?;
    let inventory = files(&stage)?.iter().map(|path| Ok(json!({"path":path.strip_prefix(&stage)?.to_string_lossy().replace('\\',"/"),"bytes":fs::metadata(path)?.len(),"blake3":blake3::hash(&fs::read(path)?).to_string()}))).collect::<Result<Vec<_>>>()?;
    support::json(
        &stage.join("files.json"),
        &json!({"version":version,"files":inventory}),
    )?;
    fs::rename(&stage, &output)?;
    let portable = dist.join(format!("{name}.zip"));
    archive::zip(&output, &portable)?;
    let installer = dist.join(format!("{name}-setup.exe"));
    fs::write(
        &installer,
        ara_export_installer::append(
            &fs::read(root.join("target/release/ara-setup.exe"))?,
            &fs::read(&portable)?,
        )?,
    )?;
    sign(root, &installer, &dist)?;
    let symbols_zip = dist.join(format!("{name}-symbols.zip"));
    archive::zip(&symbols, &symbols_zip)?;
    let artifacts = [&portable, &installer, &symbols_zip].map(|path| -> Result<_> { Ok(json!({"path":path.file_name().map(|name| name.to_string_lossy()),"bytes":fs::metadata(path)?.len(),"blake3":blake3::hash(&fs::read(path)?).to_string()})) }).into_iter().collect::<Result<Vec<_>>>()?;
    support::json(
        &dist.join("artifacts.json"),
        &json!({"version":version,"artifacts":artifacts}),
    )?;
    notes(root, version)?;
    for path in [portable, installer, symbols_zip] {
        println!("{}: {} bytes", path.display(), fs::metadata(&path)?.len());
    }
    Ok(())
}

fn sdk() -> Result<PathBuf> {
    if let Some(sdk) = std::env::var_os("VULKAN_SDK") {
        return Ok(PathBuf::from(sdk));
    }
    let mut installed = fs::read_dir("C:/VulkanSDK")
        .context("Install the Vulkan SDK or set VULKAN_SDK")?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.join("Bin/slangc.exe").is_file())
        .collect::<Vec<_>>();
    installed.sort_by_cached_key(|path| {
        path.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .split('.')
            .map(|number| number.parse::<u32>().unwrap_or(0))
            .collect::<Vec<_>>()
    });
    installed
        .pop()
        .context("No shader tools found under C:/VulkanSDK; set VULKAN_SDK")
}
fn bundle_tools(root: &Path, stage: &Path, sdk: &Path) -> Result<()> {
    // Both SPIR-V and DXIL are emitted by the existing shader cooker.
    for file in [
        "slangc.exe",
        "slang.dll",
        "slang-compiler.dll",
        "slang-glslang.dll",
        "slang-glsl-module.dll",
        "slang-rt.dll",
        "dxcompiler.dll",
        "dxc.exe",
        "spirv-val.exe",
    ] {
        copy(&sdk.join("Bin").join(file), &stage.join("tools").join(file))?;
    }
    for entry in fs::read_dir(sdk.join("Bin"))? {
        let path = entry?.path();
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("slang-standard-module-"))
        {
            copy_tree(
                &path,
                &stage
                    .join("tools")
                    .join(path.file_name().context("Slang module directory")?),
            )?;
        }
    }
    for entry in fs::read_dir(root.join("release/licenses"))? {
        let path = entry?.path();
        if path.is_file() {
            copy(
                &path,
                &stage
                    .join("tools/licenses")
                    .join(path.file_name().context("tool license filename")?),
            )?;
        }
    }
    copy(
        &sdk.join("Licenses/LICENSE.txt"),
        &stage.join("tools/licenses/Vulkan-SDK.txt"),
    )?;
    Ok(())
}

fn bundle_dotnet(root: &Path, stage: &Path, symbols: &Path, dist: &Path) -> Result<()> {
    for project in ["Ara.Host", "Ara.CompileServer", "Ara.RunWorker"] {
        run(
            Command::new("dotnet").current_dir(root).args([
                "build",
                &format!("dotnet/src/{project}/{project}.csproj"),
                "-c",
                "Release",
                "-m:2",
            ]),
            &dist.join(format!("logs/{project}.log")),
        )?;
    }
    for project in [
        "Ara.Host",
        "Ara.Runtime",
        "Ara.CompileServer",
        "Ara.Analyzers",
        "Ara.Generators",
        "Ara.RunWorker",
        "Ara.Agent",
    ] {
        let source = root
            .join("dotnet/artifacts/bin")
            .join(project)
            .join("release");
        let target = stage
            .join("dotnet/artifacts/bin")
            .join(project)
            .join("release");
        for file in files(&source)? {
            let rel = file.strip_prefix(&source)?;
            if file.extension().is_some_and(|extension| extension == "pdb") {
                copy(&file, &symbols.join("dotnet").join(project).join(rel))?;
            } else {
                copy(&file, &target.join(rel))?;
            }
        }
    }
    Ok(())
}

fn templates(root: &Path, stage: &Path, dist: &Path) -> Result<()> {
    run(
        Command::new(stage.join("ara.exe"))
            .env("GIT_CEILING_DIRECTORIES", stage)
            .args(["init"])
            .arg(stage.join("templates/starter"))
            .args(["--name", "Starter"]),
        &dist.join("logs/starter.log"),
    )?;
    let samples = root.join("samples");
    if samples.is_dir() {
        for entry in fs::read_dir(&samples)? {
            let path = entry?.path();
            if path.join("ara.project.json").is_file() {
                for file in files(&path)? {
                    let rel = file.strip_prefix(&path)?;
                    if !rel.components().any(|c| {
                        [".git", ".ara", "target", "bin", "obj", "evidence"]
                            .iter()
                            .any(|name| c.as_os_str() == *name)
                    }) {
                        copy(
                            &file,
                            &stage
                                .join("templates")
                                .join(path.file_name().context("sample directory")?)
                                .join(rel),
                        )?;
                    }
                }
            }
        }
        if samples.join("assets.lock.json").is_file() {
            copy(
                &samples.join("assets.lock.json"),
                &stage.join("templates/assets.lock.json"),
            )?;
        }
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(output.status.success(), "git {args:?} failed");
    Ok(String::from_utf8(output.stdout)?)
}
pub fn notes(root: &Path, version: &str) -> Result<()> {
    let decisions = fs::read_to_string(root.join("docs/decisions.md"))?;
    let highlights = decisions
        .lines()
        .filter(|line| {
            ["D2", "D3", "D9", "D11", "D12", "D22", "D23", "D24", "D25"]
                .iter()
                .any(|id| line.starts_with(&format!("| {id} |")))
        })
        .collect::<Vec<_>>()
        .join("\n");
    let features = git(
        root,
        &[
            "log",
            "--first-parent",
            "--max-count=80",
            "--format=- %s (%h)",
        ],
    )?;
    let issues = fs::read_to_string(root.join("docs/KNOWN_ISSUES.md"))?;
    let notes = format!(
        "# Ara {version}\n\nEarly Windows alpha for testers. Unsigned; no automatic crash uploads.\n\n## System requirements\n\n- Windows 10/11 x64; 16 GB RAM.\n- Vulkan 1.3 GPU and current driver; ray tracing recommended (RTX 20-series or newer / RX 6000+).\n- x64 .NET 10 SDK and runtime: https://dotnet.microsoft.com/en-us/download/dotnet/10.0\n- Microsoft VC++ v14 Redistributable (x64), required by bundled SDK shader tools: https://learn.microsoft.com/en-us/cpp/windows/latest-supported-vc-redist\n- Slang, DXC and SPIR-V validation tools are bundled; no Vulkan SDK or Rust is needed to run the installed engine.\n\n## Installation\n\nRun the setup EXE, or extract the ZIP and run install.ps1. Versions install side by side under %LOCALAPPDATA%\\Programs\\Ara\\{version}. Connect agents with `ara hub install-mcp --client claude|codex`.\n\n## Decision highlights\n\n| ID | Decision | Date |\n|---|---|---|\n{highlights}\n\n## Merged features (first-parent history)\n\n{features}\n\n## Known issues (source document snapshot)\n\nThe following is copied from docs/KNOWN_ISSUES.md. Its installer/Hub build-order entries may precede this packaging; see the smoke report for actual installed-build results.\n\n{issues}\n"
    );
    let dist = support::dist(root);
    fs::create_dir_all(&dist)?;
    fs::write(dist.join("RELEASE_NOTES.md"), notes)?;
    println!("{}", dist.join("RELEASE_NOTES.md").display());
    Ok(())
}
fn sign(root: &Path, installer: &Path, dist: &Path) -> Result<()> {
    if let Some(thumbprint) = std::env::var_os("ARA_SIGN_THUMBPRINT") {
        run(
            Command::new("signtool.exe")
                .current_dir(root)
                .args(["sign", "/sha1"])
                .arg(thumbprint)
                .args([
                    "/fd",
                    "SHA256",
                    "/tr",
                    "http://timestamp.digicert.com",
                    "/td",
                    "SHA256",
                ])
                .arg(installer),
            &dist.join("logs/sign.log"),
        )?;
    } else {
        fs::write(
            dist.join("logs/sign.log"),
            "Unsigned alpha. Optional: put signtool.exe on PATH and set ARA_SIGN_THUMBPRINT to a certificate thumbprint. No certificate or signing tool is required.\n",
        )?;
        eprintln!("Unsigned alpha: ARA_SIGN_THUMBPRINT is unset (signtool hook skipped).");
    }
    Ok(())
}
pub fn symbols(root: &Path, directory: &Path) -> Result<()> {
    let directory = directory.canonicalize()?;
    run(
        Command::new("cargo")
            .current_dir(root)
            .env("CARGO_BUILD_JOBS", "2")
            .args([
                "build",
                "-p",
                "ara-crash",
                "--features",
                "symbols",
                "--bin",
                "ara-crash",
            ]),
        &support::dist(root).join("logs/symbol-tool.log"),
    )?;
    let dump_syms = directory.join("symbol-store");
    // ara-crash's public archive API is exposed through a small conversion CLI.
    for file in files(&directory)? {
        if file.extension().is_some_and(|extension| extension == "pdb")
            && file.with_extension("exe").is_file()
        {
            run(
                Command::new(root.join("target/debug/ara-crash.exe"))
                    .arg("archive")
                    .arg(&file)
                    .arg(&dump_syms),
                &support::dist(root).join(format!(
                    "logs/{}.log",
                    file.file_stem()
                        .context("symbol filename")?
                        .to_string_lossy()
                )),
            )?;
        }
    }
    Ok(())
}
