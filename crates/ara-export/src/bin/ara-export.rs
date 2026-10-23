use anyhow::{Context, Result};
use ara_export::{
    ExportPreset, ExportRequest, Platform,
    doctor::{DoctorOptions, Status},
    event::{Diagnostic, Event, EventKind, EventSink, Ndjson},
    tools::Toolchain,
};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(about = "Ara production export pipeline (temporary CLI until ara wiring)")]
struct Cli {
    #[command(subcommand)]
    command: Cmd,
}
#[derive(Subcommand)]
enum Cmd {
    /// Checks prerequisites without starting WSL or an emulator.
    Doctor {
        #[arg(long)]
        preset: Option<PathBuf>,
        #[arg(long, default_value = ".")]
        project: PathBuf,
        #[arg(long)]
        platform: Option<Platform>,
        #[arg(long, default_value_t = 36)]
        target_api: u32,
        #[arg(long)]
        devices: bool,
    },
    /// Exports a project using its schema-registered canonical preset.
    Export {
        #[arg(long)]
        project: PathBuf,
        #[arg(long)]
        preset: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        cache: Option<PathBuf>,
        #[arg(long)]
        device: Option<String>,
        #[arg(long)]
        allow_emulator: bool,
    },
    /// Canonicalizes and validates a project export preset.
    PresetFmt { path: PathBuf },
    /// Prints the schema contract for CLI/protocol integration.
    Schema,
    /// Creates a content hash pin for a prebuilt runtime template.
    Pin {
        #[arg(long)]
        platform: Platform,
        #[arg(long)]
        arch: String,
        #[arg(long, default_value = "release")]
        config: ara_export::preset::Configuration,
        #[arg(long)]
        file: PathBuf,
        #[arg(long)]
        symbols: Option<PathBuf>,
    },
    /// Creates a generic shipping preset from an existing platform/template preset.
    Preset {
        #[arg(long)]
        kind: ara_export::preset::PackageFormat,
        #[arg(long)]
        base: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Produces a pin for an installer stub, AppImage runtime or debug file.
    ArtifactPin {
        #[arg(long)]
        file: PathBuf,
    },
    /// Processes a native dump or Rust/managed crash record locally.
    Crash {
        #[command(subcommand)]
        command: CrashCommand,
    },
}
#[derive(Subcommand)]
enum CrashCommand {
    /// Symbolizes with local build-matched symbols, and emits a host diagnostic.
    Symbolize {
        dump: PathBuf,
        #[arg(long)]
        symbols: PathBuf,
    },
}
fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            let _ = Ndjson(std::io::stdout().lock()).emit(&Event {
                version: 1,
                sequence: 1,
                stage: "resolve".into(),
                kind: EventKind::Error(Diagnostic {
                    code: "ARA-EXP-CLI-001".into(),
                    message: format!("{e:#}"),
                    context: serde_json::json!({}),
                    fixes: vec!["ara-export --help".into()],
                }),
            });
            ExitCode::FAILURE
        }
    }
}
fn run() -> Result<bool> {
    match Cli::parse().command {
        Cmd::Doctor {
            preset,
            project,
            platform,
            target_api,
            devices,
        } => {
            let opts = DoctorOptions {
                platform,
                target_api,
                devices,
                scratch: std::env::temp_dir().join(format!("ara-doctor-{}", std::process::id())),
            };
            let tools = Toolchain::discover();
            let checks = if let Some(path) = preset {
                let preset = ExportPreset::from_text(&std::fs::read_to_string(path)?)?;
                anyhow::ensure!(
                    platform.is_none_or(|p| p == preset.platform),
                    "ARA-EXP-CLI-002: --platform does not match preset"
                );
                ara_export::doctor::doctor_for_preset(&project, &preset, &opts, &tools)
            } else {
                ara_export::doctor::doctor(&opts, &tools)
            };
            for c in &checks {
                println!("{}", serde_json::to_string(c)?);
            }
            Ok(!checks.iter().any(|c| c.status == Status::Error))
        }
        Cmd::Export {
            project,
            preset,
            output,
            cache,
            device,
            allow_emulator,
        } => {
            let preset_path = if preset.is_file() {
                preset
            } else {
                project
                    .join("presets")
                    .join(format!("{}.export.json", preset.to_string_lossy()))
            };
            let preset = ExportPreset::from_text(&std::fs::read_to_string(preset_path)?)?;
            let cache = cache.unwrap_or_else(|| project.join(".sn/export-cache"));
            let request = ExportRequest {
                project,
                preset,
                output,
                cache,
                device,
                allow_emulator,
            };
            Ok(ara_export::export(&request, &mut Ndjson(std::io::stdout().lock())).is_ok())
        }
        Cmd::PresetFmt { path } => {
            let preset = ExportPreset::from_text(&std::fs::read_to_string(&path)?)?;
            std::fs::write(path, preset.to_text()?)?;
            Ok(true)
        }
        Cmd::Schema => {
            let mut db = ara_schema::SchemaDb::new();
            ExportPreset::register(&mut db);
            println!(
                "{}",
                ara_json::to_canonical_string(
                    &db.type_json_schema(
                        db.get("ExportPreset")
                            .context("registered preset schema missing")?
                    )
                )
            );
            Ok(true)
        }
        Cmd::Pin {
            platform,
            arch,
            config,
            file,
            symbols,
        } => {
            let file = file.canonicalize()?;
            let pin = ara_export::preset::TemplatePin {
                platform,
                arch,
                config,
                hash: blake3::hash(&std::fs::read(&file)?).to_hex().to_string(),
                source: file.to_string_lossy().into_owned(),
                symbols: symbols
                    .map(|path| -> Result<_> {
                        let path = path.canonicalize()?;
                        Ok(ara_export::preset::ArtifactPin {
                            hash: blake3::hash(&std::fs::read(&path)?).to_hex().to_string(),
                            source: path.to_string_lossy().into_owned(),
                        })
                    })
                    .transpose()?,
            };
            println!(
                "{}",
                ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(pin)?))
            );
            Ok(true)
        }
        Cmd::Preset { kind, base, output } => {
            let mut preset = ExportPreset::from_text(&std::fs::read_to_string(base)?)?;
            preset.packaging = kind;
            let text = preset.to_text()?;
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(output, text)?;
            Ok(true)
        }
        Cmd::ArtifactPin { file } => {
            let file = file.canonicalize()?;
            let pin = ara_export::preset::ArtifactPin {
                hash: blake3::hash(&std::fs::read(&file)?).to_hex().to_string(),
                source: file.to_string_lossy().into_owned(),
            };
            println!(
                "{}",
                ara_json::to_canonical_string(&ara_json::sort_keys(&serde_json::to_value(pin)?))
            );
            Ok(true)
        }
        Cmd::Crash {
            command: CrashCommand::Symbolize { dump, symbols },
        } => {
            let report = ara_crash::symbols::symbolize(&dump, &symbols)?;
            println!(
                "{}",
                ara_json::to_canonical_string(&ara_json::sort_keys(
                    &serde_json::json!({"report":report,"diagnostic":report.diagnostic()})
                ))
            );
            Ok(true)
        }
    }
}
