use crate::{
    archive, notices, references,
    support::{self, copy, copy_tree, files, run},
};
use anyhow::{Context, Result, ensure};
use serde_json::json;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};
mod readme;
mod symbols_payload;

const BINARIES: &[(&str, &str)] = &[
    ("ara", "ara"),
    ("ara-editor", "ara-editor"),
    ("ara-player", "ara-player"),
    ("ara-import-worker", "ara-import-worker"),
    ("ara-nav", "ara-nav-worker"),
    ("ara-terrain", "ara-terrain-worker"),
    ("ara-shader-cook", "ara-shader-cook"),
    ("ara-setup", "ara-setup"),
];
const RESOURCES: &[&str] = &[
    "ara-texture-gpu",
    "ara-render",
    "ara-render-fidelity",
    "ara-runtime-render",
    "ara-render2d",
    "ara-landscape",
    "ara-vfx-gpu",
];

pub fn package(root: &Path, version: &str) -> Result<()> {
    ensure!(
        !ara_update::trusted_keys().is_empty(),
        "embed a release public key before packaging an installable engine/Hub"
    );
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
    // Packaging tool only (never shipped): writes the samples' generated kits.
    build.args(["-p", "ara-samples", "--bin", "ara-samples"]);
    // A Hub UI package can land independently; no fake Hub executable is shipped.
    let hub = root.join("crates/ara-hub/Cargo.toml").is_file();
    if hub {
        build.args(["-p", "ara-hub", "--bin", "ara-hub"]);
        build.args(["--bin", "ara-hub-updater"]);
        if cfg!(feature = "rehearsal") {
            build.args(["--features", "ara-hub/rehearsal"]);
        }
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
        .chain(hub.then_some("ara-hub"))
        .chain(hub.then_some("ara-hub-updater"));
    for binary in binaries {
        let source = support::profile(root, "release").join(format!("{binary}.exe"));
        if binary != "ara-setup" {
            copy(&source, &stage.join(format!("{binary}.exe")))?;
        }
        let pdb =
            support::profile(root, "release").join(format!("{}.pdb", binary.replace('-', "_")));
        ensure!(
            pdb.is_file(),
            "release PDB missing for {binary}; symbols must match this build"
        );
        symbols_payload::stage(binary, &pdb, &stage, &symbols)?;
        copy(
            &source,
            &symbols.join(format!("{}.exe", binary.replace('-', "_"))),
        )?;
        identities.push(json!({"binary":format!("{binary}.exe"), "blake3":blake3::hash(&fs::read(&source)?).to_string(), "pdb":pdb.file_name().map(|name| name.to_string_lossy())}));
    }
    for entry in fs::read_dir(support::profile(root, "release"))? {
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
        "paths.ps1",
        "hub-installation.ps1",
        "uninstall.ps1",
        "launch-hub.ps1",
        "launch-editor.ps1",
        "LICENSE",
    ] {
        copy(&root.join("release").join(file), &stage.join(file))?;
    }
    fs::write(stage.join("README-FIRST.txt"), readme::render(version))?;
    bundle_tools(root, &stage, &sdk)?;
    if hub {
        let script = stage.join("install.ps1");
        crate::hub::verify_binary(root, &stage, version, &dist)?;
        let mut install = fs::read_to_string(&script)?;
        install.push_str(include_str!("../engine-hub.ps1"));
        fs::write(script, install)?;
        support::json(
            &stage.join("hub-launcher.json"),
            &crate::hub::marker(root, version)?,
        )?;
        support::json(
            &stage.join("hub-service.json"),
            &json!({"version":version,"build":ara_update::sha256_file(&stage.join("ara.exe"))?}),
        )?;
        fs::write(
            stage.join("standalone-hub-install.ps1"),
            include_str!("../hub-install.ps1"),
        )?;
    }
    bundle_dotnet(root, &stage, &symbols, &dist)?;
    templates(root, &stage, &dist)?;
    // Generate with the packaged CLI, so docs describe the exact shipped schema
    // and command registry. The scratch project lives only in the staging tree.
    let docs_project = stage.join(".docs-project");
    fs::create_dir(&docs_project)?;
    fs::write(
        docs_project.join("ara.project.json"),
        "{\"name\":\"Ara engine reference\"}",
    )?;
    fs::create_dir_all(dist.join("docs-profile"))?;
    run(
        Command::new(stage.join("ara.exe"))
            .current_dir(&stage)
            .env("ARA_CONFIG_DIR", dist.join("docs-profile"))
            .args(["--root"])
            .arg(&docs_project)
            .args(["docs", "--llms", "--out"])
            .arg(stage.join("docs")),
        &dist.join("logs/docs.log"),
    )?;
    copy(&stage.join("docs/docs.json"), &stage.join("docs.json"))?;
    ensure!(
        docs_project
            .canonicalize()?
            .starts_with(stage.canonicalize()?),
        "docs scratch escaped the package stage"
    );
    fs::remove_dir_all(&docs_project)?;
    notices::generate(root, &stage, &dist)?;
    support::json(
        &stage.join("engine.json"),
        &json!({"format":"ara.engine/1","version":version,"rehearsal":cfg!(feature="rehearsal"),"platform":"windows-x64","hub":{"binary":"ara-hub.exe","available":hub},"sourceCommit":git(root, &["rev-parse","HEAD"])? .trim(),"crashes":"%LOCALAPPDATA%/Ara/crashes"}),
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
            &fs::read(support::profile(root, "release").join("ara-setup.exe"))?,
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
        "Ara.Editor",
        "Ara.CompileServer",
        "Ara.Analyzers",
        "Ara.Analyzers.CodeFixes",
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
            .env("ARA_CONFIG_DIR", dist.join("package-config"))
            .env("GIT_CEILING_DIRECTORIES", stage)
            .args(["init"])
            .arg(stage.join("templates/starter"))
            .args(["--name", "Starter"]),
        &dist.join("logs/starter.log"),
    )?;
    let samples = root.join("samples");
    let templates = stage.join("templates");
    // An actual headless capture of ara init's empty scene, also embedded in
    // the Hub so Starter has a preview before an image finishes loading.
    copy(
        &root.join("release/starter.png"),
        &templates.join("starter/thumbnail.png"),
    )?;
    if samples.is_dir() {
        if samples.join("assets.lock.json").is_file() {
            copy(
                &samples.join("assets.lock.json"),
                &templates.join("assets.lock.json"),
            )?;
        }
        for entry in fs::read_dir(&samples)? {
            let path = entry?.path();
            if path.join("ara.project.json").is_file() {
                let name = path.file_name().context("sample directory")?;
                for file in files(&path)? {
                    let rel = file.strip_prefix(&path)?;
                    if !rel.components().any(|c| {
                        [".git", ".ara", "target", "bin", "obj", "evidence"]
                            .iter()
                            .any(|name| c.as_os_str() == *name)
                    }) {
                        copy(&file, &templates.join(name).join(rel))?;
                    }
                }
                sample_thumbnail(&path, &templates)?;
                // Kits are generated and git-ignored: write them into the staged copy
                // (no document is authored, so the checkout's scenes never drift).
                run(
                    Command::new(support::profile(root, "release").join("ara-samples.exe"))
                        .arg("--samples")
                        .arg(&templates)
                        .arg("kits")
                        .arg(name),
                    &dist.join(format!("logs/kits-{}.log", name.to_string_lossy())),
                )?;
            }
        }
    }
    for entry in fs::read_dir(&templates)? {
        let template = entry?.path();
        if template.join("ara.project.json").is_file() {
            references::check(&template)?;
        }
    }
    Ok(())
}

/// Keep the Hub's template preview, which lives in the excluded `evidence/`: the
/// Play capture of the sample's main scene becomes `thumbnail.png` (the rule
/// `ara_hub_service::DirectoryTemplates` applies in a checkout).
fn sample_thumbnail(sample: &Path, templates: &Path) -> Result<()> {
    let marker: serde_json::Value =
        serde_json::from_slice(&fs::read(sample.join("ara.project.json"))?)?;
    let Some(stem) = marker["main"]
        .as_str()
        .and_then(|main| Path::new(main).file_stem())
    else {
        return Ok(());
    };
    let preview = sample
        .join("evidence")
        .join(format!("{}-player.png", stem.to_string_lossy()));
    let target = templates
        .join(sample.file_name().context("sample directory")?)
        .join("thumbnail.png");
    if preview.is_file() && !target.exists() {
        copy(&preview, &target)?;
    }
    Ok(())
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").current_dir(root).args(args).output()?;
    ensure!(output.status.success(), "git {args:?} failed");
    Ok(String::from_utf8(output.stdout)?)
}
/// Ship the reviewed notes verbatim instead of regenerating them from commit subjects.
pub fn notes(root: &Path, version: &str) -> Result<()> {
    let notes = fs::read_to_string(root.join("RELEASE_NOTES.md"))
        .context("read the reviewed RELEASE_NOTES.md")?;
    ensure!(
        notes.lines().next() == Some(format!("# Ara {version}").as_str()),
        "RELEASE_NOTES.md must start with '# Ara {version}' before packaging"
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
                Command::new(support::profile(root, "debug").join("ara-crash.exe"))
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
