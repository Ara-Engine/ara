use crate::files::{collect_sizes, copy_all, copy_scripts};
use crate::{
    ExportPreset, Platform, android, apple,
    event::{Diagnostic, Event, EventKind, EventSink},
    preset::{PlayerTemplate, ScriptLane},
    template,
    tools::{self, Toolchain},
    verify::{self, VerifyReport},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Explicit export inputs for protocol/CLI callers.
pub struct ExportRequest {
    /// Project root, with C# project, content and export preset under version control.
    pub project: PathBuf,
    /// Parsed and validated preset.
    pub preset: ExportPreset,
    /// New output directory; an existing directory is never overwritten.
    pub output: PathBuf,
    /// Shared content-addressed template cache and private job directories.
    pub cache: PathBuf,
    /// Optional explicit Android device serial.
    pub device: Option<String>,
    /// Allows verification on an already running, explicitly selected emulator.
    pub allow_emulator: bool,
}

/// Export artifact summary. The canonical representation is report.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSummary {
    /// Report schema identifier.
    pub format: String,
    /// Destination OS name.
    pub platform: String,
    /// Configuration of the pinned runtime.
    pub config: String,
    /// Selected C# lane.
    pub lane: ScriptLane,
    /// `unsigned`, `android-debug` or `android-upload`; debug-signed packages are not store-ready.
    pub signing: String,
    /// Total wall time including verification and packaging.
    pub elapsed_ms: u64,
    /// Stage timings in milliseconds.
    pub stages_ms: BTreeMap<String, u64>,
    /// File names and sizes (excluding report/log metadata).
    pub artifacts: BTreeMap<String, u64>,
    /// Content pack identity.
    pub pack_id: String,
    /// Identity of the private native/managed symbol archive.
    pub build_id: String,
    /// Selected launch contract; conformance results never imply engine gameplay ran.
    pub template_contract: String,
    /// Runtime verification / explicitly selected static checks.
    pub verify: VerifyReport,
}

struct Driver<'a> {
    sink: &'a mut dyn EventSink,
    seq: u64,
    stage: String,
    platform: Platform,
    work: PathBuf,
    stages: BTreeMap<String, u64>,
}
impl Driver<'_> {
    fn emit(&mut self, kind: EventKind) -> Result<()> {
        self.seq += 1;
        self.sink.emit(&Event {
            version: 1,
            sequence: self.seq,
            stage: self.stage.clone(),
            kind,
        })?;
        Ok(())
    }
    fn stage<T>(&mut self, name: &str, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        self.stage = name.into();
        self.emit(EventKind::Progress {
            status: "started".into(),
            elapsed_ms: None,
        })?;
        let started = Instant::now();
        let result = f(self)?;
        let ms = millis(started.elapsed());
        self.stages.insert(name.into(), ms);
        self.emit(EventKind::Progress {
            status: "completed".into(),
            elapsed_ms: Some(ms),
        })?;
        Ok(result)
    }
    fn log(&self, name: &str) -> PathBuf {
        self.work.join("logs").join(format!("{name}.log"))
    }
    fn artifact(&mut self, path: &Path) -> Result<()> {
        let path = std::path::absolute(path)?;
        let bytes = std::fs::read(&path)?;
        self.emit(EventKind::Artifact {
            path: path.to_string_lossy().into_owned(),
            bytes: bytes.len() as u64,
            hash: blake3::hash(&bytes).to_hex().to_string(),
        })
    }
}

/// Runs template → script/native → pack → assemble/package/sign → verify.
/// A failed export emits one coded error and preserves its private job logs plus
/// failure report. Existing exports are never mutated.
pub fn export(request: &ExportRequest, sink: &mut dyn EventSink) -> Result<ExportSummary> {
    let started = Instant::now();
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
    let work = std::path::absolute(&request.cache)?
        .join("jobs")
        .join(format!(
            "{}-{}-{stamp}",
            request.preset.platform.name(),
            std::process::id()
        ));
    let mut d = Driver {
        sink,
        seq: 0,
        stage: "resolve".into(),
        platform: request.preset.platform,
        work,
        stages: BTreeMap::new(),
    };
    let result = run(request, &mut d, started);
    if let Err(error) = &result {
        let diagnostic = Diagnostic {
            code: extract_code(&format!("{error:#}")).unwrap_or_else(|| {
                format!(
                    "ARA-EXP-{}-100",
                    match d.platform {
                        Platform::Windows => "WIN",
                        Platform::Linux => "LNX",
                        Platform::Android => "AND",
                        _ => "APL",
                    }
                )
            }),
            message: format!("{error:#}"),
            context: json!({"platform":d.platform.name(),"stage":d.stage,"job":d.work,"logs":d.work.join("logs"),"tool":error.downcast_ref::<tools::ToolFailure>().map(|t|json!({"program":t.program,"log":t.log,"exit_code":t.exit_code,"timed_out":t.timed_out}))}),
            fixes: vec![format!(
                "ara-export doctor --platform {}",
                d.platform.name()
            )],
        };
        ara_diag::Diagnostic::from(&diagnostic).emit();
        d.emit(EventKind::Error(diagnostic.clone()))?;
        tools::write_json(
            &d.work.join("report.json"),
            &json!({"format":"ara.export.report/1","passed":false,"error":diagnostic}),
        )?;
    }
    result
}

fn run(r: &ExportRequest, d: &mut Driver<'_>, started: Instant) -> Result<ExportSummary> {
    let p = &r.preset;
    d.stage("resolve", |d| {
        std::fs::create_dir_all(&d.work)?;
        p.validate()?;
        ensure!(
            !r.output.exists(),
            "ARA-EXP-OUT-001: output exists; choose a new export directory"
        );
        Ok(())
    })?;
    // A Mac exports macOS itself; iOS, and macOS from other hosts, use the agent seam.
    if p.platform == Platform::Ios || (p.platform == Platform::Macos && !cfg!(target_os = "macos"))
    {
        return apple::require_agent(p).and_then(|()| {
            Err(anyhow::anyhow!(
                "ARA-EXP-APL-002: Mac agent not implemented"
            ))
        });
    }
    let macos = p.platform == Platform::Macos;
    ensure!(
        !macos || p.plugins.is_empty(),
        "ARA-EXP-APL-009: native plugins are not built for macOS exports yet"
    );
    // Desktop platforms have one architecture.
    let arch = p.platform.arches()[0];
    let project = dunce::canonicalize(&r.project)?;
    let toolchain = Toolchain::discover();
    let output = d.work.join("dist");
    let dist = output.join("build");
    std::fs::create_dir_all(&dist)?;
    // macOS assembles the desktop layout first, then bundles it as `<Game>.app`.
    let staged = if macos {
        d.work.join("staged")
    } else {
        dist.clone()
    };
    std::fs::create_dir_all(&staged)?;
    let templates = d.stage("template", |d| {
        p.templates
            .iter()
            .map(|pin| {
                let resolved = template::resolve(pin, &project, &std::path::absolute(&r.cache)?)?;
                let executable = if p.player_template == PlayerTemplate::Runtime {
                    ensure!(
                        matches!(
                            p.platform,
                            Platform::Windows | Platform::Linux | Platform::Macos
                        ),
                        "the real game runtime template currently requires a desktop platform"
                    );
                    crate::runtime::unpack_template(
                        &resolved,
                        &d.work.join("runtime").join(&pin.arch),
                        if p.platform == Platform::Windows {
                            "ara-player.exe"
                        } else {
                            "ara-player"
                        },
                    )?
                } else {
                    resolved
                };
                Ok((pin.arch.clone(), executable))
            })
            .collect::<Result<BTreeMap<_, _>>>()
    })?;
    let runtime_scripts = p.player_template == crate::preset::PlayerTemplate::Runtime;
    let shipping = if runtime_scripts {
        d.stage("script-prepare", |d| {
            crate::scripts::prepare(&project, &p.assembly, &d.work, &toolchain)
        })?
    } else {
        None
    };
    if runtime_scripts && shipping.is_some() {
        ensure!(
            p.lane == ScriptLane::NativeAot,
            "ARA0920: the runtime gameplay host requires NativeAOT; select nativeAot for this export"
        );
    }
    let script_project = if let Some(shipping) = &shipping {
        Some(shipping.project.clone())
    } else if runtime_scripts {
        None
    } else {
        Some(tools::project_path(&project, &p.script_project)?)
    };
    // Attach once in the linked entry image, rather than once per module.
    let crash_targets = crate::scripts::crash_targets(&d.work, runtime_scripts)?;
    let mut scripts = BTreeMap::new();
    d.stage("script", |d| {
        let Some(script_project) = &script_project else {
            return Ok(());
        };
        let targets: Vec<_> = if p.platform == Platform::Android {
            android::ABIS.iter().map(|(a, _, rid)| (*a, *rid)).collect()
        } else {
            vec![(arch, crate::scripts::desktop_rid(p.platform))]
        };
        for (arch, rid) in targets {
            let out = d.work.join("script").join(arch);
            let mut cmd = crate::scripts::publish_command(
                script_project,
                rid,
                &out,
                p.lane == ScriptLane::NativeAot,
                &crash_targets,
                &toolchain,
            )?;
            if !runtime_scripts {
                cmd.arg(format!(
                    "-p:BaseIntermediateOutputPath={}/",
                    d.work.join("obj").join(arch).display()
                ))
                .arg(format!(
                    "-p:BaseOutputPath={}/",
                    d.work.join("bin").join(arch).display()
                ));
            }
            if p.platform == Platform::Linux && p.lane == ScriptLane::NativeAot {
                let zig = toolchain
                    .zig
                    .as_ref()
                    .context("ARA-EXP-LNX-006: ARA_ZIG not set")?;
                let wrapper = toolchain
                    .zigcc
                    .as_ref()
                    .context("ARA-EXP-LNX-007: production ara-zigcc missing")?;
                cmd.args([
                    "-p:DisableUnsupportedError=true",
                    "-p:CppCompilerAndLinker=ara-zigcc",
                    "-p:LinkerFlavor=lld",
                ])
                .env("ARA_ZIG", zig)
                .env("ARA_ZIGCC", wrapper);
            }
            if p.platform == Platform::Android {
                cmd.args([
                    "-p:DisableUnsupportedError=true",
                    "-p:PublishAotUsingRuntimePack=true",
                ]);
            }
            tools::run(
                &mut cmd,
                &d.log(&format!("script-{arch}")),
                Duration::from_secs(300),
            )?;
            if let Some(shipping) = &shipping {
                let library = crate::scripts::library_name(&p.assembly, rid);
                crate::scripts::write_manifest(&out, &library, &shipping.modules)?;
            }
            scripts.insert(arch.to_owned(), out);
        }
        Ok(())
    })?;
    let plugins = d.stage("native", |d| {
        crate::native::compile_plugins(p, &project, &toolchain, &d.work)
    })?;
    let encryption = p
        .encryption_key_env
        .as_ref()
        .map(|name| -> Result<_> {
            let text = std::env::var(name).map_err(|_| {
                anyhow::anyhow!("ARA-EXP-PACK-002: encryption key environment variable is unset")
            })?;
            Ok(ara_pack::EncryptionKey::from_hex(&text)?)
        })
        .transpose()?;
    let pack = d.stage("pack", |d| {
        let cook = ara_pack::CookKey {
            platform: p.platform.name().into(),
            arch: if p.platform == Platform::Android {
                "universal"
            } else {
                arch
            }
            .into(),
            config: p.config.name().into(),
            cooker: "ara-export/1".into(),
            settings: BTreeMap::new(),
        };
        let built = if p.player_template == PlayerTemplate::Runtime {
            let worker = ara_play_host::PlayConfig::for_project(&project).import_worker;
            ara_pack::build(
                crate::runtime::inputs(
                    &project,
                    &worker,
                    shipping
                        .as_ref()
                        .map(|_| ara_play::ScriptScene::shipping("game/scripts.json")),
                    |progress| {
                        d.emit(EventKind::Cooking {
                            completed: progress.completed,
                            total: progress.total,
                            elapsed_ms: millis(progress.elapsed),
                            stalled_ms: millis(progress.stalled),
                        })
                        .map_err(|error| error.to_string())
                    },
                )?,
                cook,
                p.chunk_size as usize,
                encryption.as_ref(),
            )?
        } else {
            ara_pack::build_directory(
                &tools::project_path(&project, &p.content)?,
                cook,
                p.chunk_size as usize,
                encryption.as_ref(),
            )?
        };
        ara_pack::Pack::open(&built.bytes)?.validate_all(encryption.as_ref())?;
        std::fs::write(staged.join("game.arapack"), &built.bytes)?;
        Ok(built)
    })?;
    d.stage("assemble",|_| {
        if p.player_template == PlayerTemplate::Runtime {
            copy_all(templates[arch].parent().context("runtime template directory")?, &staged)?;
        }
        let library = crate::scripts::library_name(&p.assembly, crate::scripts::desktop_rid(p.platform));
        let launch=json!({"contract":p.player_template.contract(),"pack":"game.arapack","lane":if p.lane==ScriptLane::NativeAot {"nativeaot"} else {"r2r-coreclr"},"script":if p.lane==ScriptLane::NativeAot {format!("game/{library}")} else {"dotnet".into()},"assembly":p.assembly,"config":p.config.name(),"encryption_key_env":p.encryption_key_env});
        tools::write_json(&staged.join("ara-launch.json"),&launch)?;
        if p.platform==Platform::Android {
            for (abi,_,_) in android::ABIS {
                tools::copy(&templates[abi],&dist.join("lib").join(abi).join("libara_export_player.so"))?;
                if let Some(script) = scripts.get(abi) {
                    tools::copy(&script.join(format!("{}.so",p.assembly)),&dist.join("lib").join(abi).join("libgame.so"))?;
                    if let Some(shipping) = &shipping { crate::scripts::write_manifest(&dist.join("lib").join(abi), "libgame.so", &shipping.modules)?; }
                }
            }
        } else {
            tools::copy(&templates[arch],&staged.join(if p.platform==Platform::Windows {"ara-player.exe"} else {"ara-player"}))?;
            if !scripts.is_empty() && p.lane==ScriptLane::NativeAot {
                tools::copy(&scripts[arch].join(&library),&staged.join("game").join(&library))?;
                if shipping.is_some() { tools::copy(&scripts[arch].join("scripts.json"), &staged.join("game/scripts.json"))?; }
            } else if !scripts.is_empty() { copy_scripts(&scripts[arch],&staged.join("dotnet"),&p.assembly)?; }
        }
        for (arch,path) in &plugins {let name=path.file_name().context("plugin has no filename")?; tools::copy(path,&if p.platform==Platform::Android {staged.join("lib").join(arch).join(name)} else {staged.join("plugins").join(name)})?;}
        Ok(())
    })?;
    let build_id = d.stage("symbols", |d| {
        crate::symbols::archive(
            crate::symbols::Inputs {
                preset: p,
                templates: &templates,
                scripts: &scripts,
                plugins: &plugins,
                project: &project,
                cache: &std::path::absolute(&r.cache)?,
                work: &d.work,
                tools: &toolchain,
            },
            &staged,
            &output,
        )
    })?;
    let app = if macos {
        let name = crate::macos::app_name(&project);
        let app = d.stage("bundle", |_| crate::macos::bundle(&staged, &dist, &name, p))?;
        d.stage("code-sign", |d| {
            crate::macos::sign(&app, &d.work.join("logs"))
        })?;
        Some(app)
    } else {
        None
    };
    if p.platform == Platform::Windows {
        d.stage("code-sign", |d| {
            for (name, path) in crate::distribution::files(&dist)? {
                if path.extension().is_some_and(|e| e == "exe" || e == "dll") {
                    crate::distribution::sign(
                        &path,
                        p,
                        &d.log(&format!("sign-{}", name.replace('/', "-"))),
                    )?;
                }
            }
            Ok(())
        })?;
    }
    if p.platform == Platform::Android {
        d.stage("package-sign", |d| {
            android::package(&project, p, &toolchain, &dist, &d.work, &output)
        })?;
    }
    let verify = d.stage("verify", |d| match p.platform {
        Platform::Windows => verify::desktop(&dist),
        Platform::Macos => {
            crate::macos::verify(app.as_ref().context("macOS app bundle")?, &d.work, &output)
        }
        Platform::Linux => verify::linux(&dist),
        Platform::Android => verify::android(
            &output,
            p,
            &toolchain,
            r.device.as_deref(),
            r.allow_emulator,
        ),
        Platform::Ios => unreachable!("iOS returned to the Mac-agent seam before compilation"),
    })?;
    // Verification logs and crash artifacts are private job evidence, never payload.
    if dist.join("verify.log").exists() {
        tools::copy(&dist.join("verify.log"), &output.join("verify.log"))?;
        std::fs::remove_file(dist.join("verify.log"))?;
    }
    let packages = d.stage("distribution", |d| {
        crate::distribution::package(
            p,
            &project,
            &std::path::absolute(&r.cache)?,
            &dist,
            &output,
            &d.work,
        )
    })?;
    for message in &verify.unavailable {
        d.emit(EventKind::Warning(Diagnostic {
            code: extract_code(message).unwrap_or_else(|| "ARA-EXP-VER-010".into()),
            message: message.clone(),
            context: json!({"method":verify.method}),
            fixes: vec![
                "wire renderer capture or request an authorized device verification".into(),
            ],
        }))?;
    }
    if macos {
        d.emit(EventKind::Warning(Diagnostic {
            code: "ARA-EXP-APL-007".into(),
            message: "macOS app is ad-hoc signed: it runs on this Mac; Developer ID signing and notarization are not configured".into(),
            context: json!({"release_ready":false}),
            fixes: vec!["keep this build on this Mac until the Developer ID and notarization lane is configured".into()],
        }))?;
    }
    if p.platform == Platform::Android && p.signing.is_none() {
        d.emit(EventKind::Warning(Diagnostic {
            code: "ARA-EXP-AND-007".into(),
            message: "APK/AAB signed with generated debug key".into(),
            context: json!({"release_ready":false}),
            fixes: vec![
                "configure signing in the project export preset before store upload".into(),
            ],
        }))?;
    }
    let signing = if p.platform == Platform::Android {
        if p.signing.is_some() {
            "android-upload"
        } else {
            "android-debug"
        }
    } else if p.windows_signing.is_some() {
        "authenticode"
    } else if macos {
        "adhoc"
    } else {
        "unsigned"
    };
    let entry_point = match (&app, p.platform) {
        (Some(app), _) => format!(
            "{}/Contents/MacOS/{}",
            app.file_name()
                .context("app bundle name")?
                .to_string_lossy(),
            crate::macos::EXECUTABLE
        ),
        (None, Platform::Windows) => "ara-player.exe".into(),
        (None, Platform::Linux) => "ara-player".into(),
        _ => "android.app.NativeActivity".into(),
    };
    crate::distribution::manifest(p, &build_id, &output, &packages, signing, &entry_point)?;
    let mut artifacts = BTreeMap::new();
    collect_sizes(&output, &output, &mut artifacts)?;
    let summary = ExportSummary {
        format: "ara.export.report/1".into(),
        platform: p.platform.name().into(),
        config: p.config.name().into(),
        lane: p.lane,
        signing: signing.into(),
        elapsed_ms: millis(started.elapsed()),
        stages_ms: d.stages.clone(),
        artifacts,
        pack_id: pack.manifest.id()?,
        build_id,
        template_contract: p.player_template.contract().into(),
        verify,
    };
    tools::write_json(&output.join("report.json"), &summary)?;
    if let Some(parent) = r.output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Preserve compiler/signing logs with the export. Never log secret command arguments.
    let logs = d.work.join("logs");
    if logs.is_dir() {
        copy_all(&logs, &output.join("logs"))?;
    }
    std::fs::create_dir(&r.output).context(
        "ARA-EXP-OUT-001: cannot claim output directory; another export may have created it",
    )?;
    copy_all(&output, &r.output)?;
    for name in summary.artifacts.keys() {
        d.artifact(&r.output.join(name))?;
    }
    d.artifact(&r.output.join("report.json"))?;
    d.emit(EventKind::Finished {
        report: std::path::absolute(r.output.join("report.json"))?
            .to_string_lossy()
            .into_owned(),
        elapsed_ms: summary.elapsed_ms,
    })?;
    Ok(summary)
}

fn millis(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}
fn extract_code(text: &str) -> Option<String> {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|s| s.starts_with("ARA-EXP-"))
        .map(str::to_owned)
}
