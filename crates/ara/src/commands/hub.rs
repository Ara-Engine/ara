//! User-level Hub commands and global MCP installation snippets.
use ara_diag::{Diagnostic, codes};
use ara_proto::{Endpoint, RpcError};
use clap::{Subcommand, ValueEnum};
use serde_json::json;
use std::{io, path::PathBuf};

#[derive(Subcommand)]
pub enum HubCommand {
    /// Run the single user-level registry and lifecycle service.
    Serve {
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Internal override for portable/test user registries.
        #[arg(long)]
        config_dir: Option<PathBuf>,
    },
    /// Register an existing project. Files remain where they are.
    Add {
        path: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Unregister a project, preserving every project file.
    Remove { project: String },
    /// Change a registered project's display name.
    Rename { project: String, name: String },
    /// List registered projects.
    Projects,
    /// List available create-project templates.
    Templates,
    /// Create and register a project from a template.
    Create {
        path: PathBuf,
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "blank")]
        template: String,
    },
    /// Open a project in its editor, or in a background daemon.
    Open {
        project: String,
        #[arg(long)]
        headless: bool,
    },
    /// Stop a project's authority process.
    Close { project: String },
    /// Running project PIDs and ports.
    Status { project: Option<String> },
    /// Installed engine versions.
    Engines {
        /// List authenticated GitHub releases instead of installed builds.
        #[arg(long)]
        available: bool,
        /// stable or preview (preview includes stable).
        #[arg(long, default_value = "stable")]
        channel: String,
    },
    /// Download, verify and register an engine version side by side.
    Install { version: String },
    /// Reinstall a pinned engine from its signed release after closing projects using it.
    Repair { version: String },
    /// Remove a Hub-managed engine, rejecting open project pins.
    Uninstall { version: String },
    /// Cancel a download returned by hub.engine.install or hub.update.prepare.
    Cancel { job: String },
    /// Register this installed engine with the running user Hub.
    RegisterEngine {
        /// Register under the registry lock without starting a Hub service.
        #[arg(long)]
        no_start: bool,
    },
    /// Unregister this exact installed build, preserving projects and other versions.
    UnregisterEngine {
        /// Use an existing Hub or its registry offline; never autostart.
        #[arg(long)]
        no_start: bool,
    },
    /// Stop the Hub service; project processes remain open.
    Shutdown,
    /// Attached agents and editors per project.
    Activity,
    /// Print a one-time user-level MCP configuration (no project root).
    InstallMcp {
        #[arg(long, value_enum)]
        client: McpClient,
    },
}

#[derive(Clone, ValueEnum)]
pub enum McpClient {
    Claude,
    Codex,
}

pub fn hub(command: HubCommand) -> Result<bool, RpcError> {
    let executable = std::env::current_exe().map_err(io_error)?;
    let executable = ara_hub_service::normalized_path(&executable).to_owned();
    if let HubCommand::InstallMcp { client } = command {
        let path = executable
            .to_str()
            .ok_or_else(|| io_error("MCP executable path is not UTF-8"))?;
        match client {
            McpClient::Claude => println!(
                "{}",
                ara_json::to_canonical_string(
                    &json!({"mcpServers":{"ara":{"command":path,"args":["mcp"]}}})
                )
            ),
            McpClient::Codex => {
                // JSON string escaping is also valid for this TOML basic string.
                let quoted = serde_json::to_string(path).map_err(io_error)?;
                println!("[mcp_servers.ara]\ncommand = {quoted}\nargs = [\"mcp\"]");
            }
        }
        return Ok(true);
    }
    if let HubCommand::Serve { port, config_dir } = command {
        let directory = match config_dir {
            Some(directory) => directory,
            None => ara_hub_service::config_dir().map_err(io_error)?,
        };
        let directory = std::path::absolute(directory).map_err(io_error)?;
        let config =
            ara_hub_service::HubConfig::for_directory(executable, directory).map_err(io_error)?;
        // Direct/legacy launches must also release the caller's installation directory.
        std::fs::create_dir_all(&config.directory).map_err(io_error)?;
        std::env::set_current_dir(&config.directory).map_err(io_error)?;
        let log = config.directory.join("hub-start.log");
        if let Err(error) = ara_hub_service::serve(config, port) {
            let _ = std::fs::write(log, error.to_string());
            return Err(error);
        }
        return Ok(true);
    }
    if matches!(
        &command,
        HubCommand::RegisterEngine { no_start: true }
            | HubCommand::UnregisterEngine { no_start: true }
    ) {
        let operation = if matches!(&command, HubCommand::RegisterEngine { .. }) {
            ara_hub_service::EngineRegistration::Register
        } else {
            ara_hub_service::EngineRegistration::Remove
        };
        let config = ara_hub_service::HubConfig::user(executable).map_err(io_error)?;
        let result = ara_hub_service::update_installed_engine(config, operation)?;
        println!("{}", ara_json::to_canonical_string(&result));
        return Ok(true);
    }
    let directory = ara_hub_service::config_dir().map_err(io_error)?;
    let mut client = ara_hub_service::connect_or_start(&directory, &executable)?;
    if matches!(
        &command,
        HubCommand::Install { .. } | HubCommand::Repair { .. }
    ) {
        let (version, method) = match &command {
            HubCommand::Install { version } => (version, "hub.engine.install"),
            HubCommand::Repair { version } => (version, "hub.engine.repair"),
            _ => unreachable!("matched install or repair"),
        };
        available_releases(&mut client, "preview")?;
        let job = client.call(method, json!({"version":version}))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(14_400);
        let mut last_progress: Option<u64> = None;
        loop {
            let jobs = client.call("hub.downloads", json!({}))?;
            if let Some(current) = jobs["jobs"]
                .as_array()
                .and_then(|jobs| jobs.iter().find(|j| j["id"] == job["id"]))
            {
                match current["state"].as_str() {
                    Some("complete") => {
                        println!("{}", ara_json::to_canonical_string(&current["result"]));
                        return Ok(true);
                    }
                    Some("failed" | "cancelled") => {
                        return Err(
                            serde_json::from_value::<Diagnostic>(current["error"].clone())
                                .map(RpcError::from)
                                .unwrap_or_else(|_| {
                                    io_error(format!("Engine installation {}", current["state"]))
                                }),
                        );
                    }
                    _ => {}
                }
                if let (Some(received), Some(total)) = (
                    current["progress"]["received"].as_u64(),
                    current["progress"]["total"].as_u64(),
                ) {
                    let percent = received.saturating_mul(100) / total.max(1);
                    if last_progress.is_none_or(|previous| percent >= previous + 5) {
                        eprintln!(
                            "Installing Ara {version}: {percent}% ({received}/{total} bytes)"
                        );
                        last_progress = Some(percent);
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(io_error(format!(
                    "installation still running; inspect hub.downloads or cancel {}",
                    job["id"]
                )));
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    }
    let (method, params) = match command {
        HubCommand::Add { path, name } => (
            "hub.register",
            json!({"path":std::path::absolute(path).map_err(io_error)?,"name":name}),
        ),
        HubCommand::Remove { project } => ("hub.remove", json!({"project":project})),
        HubCommand::Rename { project, name } => {
            ("hub.update", json!({"project":project,"name":name}))
        }
        HubCommand::Projects => ("hub.projects", json!({})),
        HubCommand::Templates => ("hub.templates", json!({})),
        HubCommand::Create {
            path,
            name,
            template,
        } => (
            "hub.create",
            json!({"path":std::path::absolute(path).map_err(io_error)?,"name":name,"template":template}),
        ),
        HubCommand::Open { project, headless } => {
            ("hub.open", json!({"project":project,"headless":headless}))
        }
        HubCommand::Close { project } => ("hub.close", json!({"project":project})),
        HubCommand::Status { project } => ("hub.status", json!({"project":project})),
        HubCommand::Engines {
            available: true,
            channel,
        } => {
            let result = available_releases(&mut client, &channel)?;
            println!("{}", ara_json::to_canonical_string(&result));
            return Ok(true);
        }
        HubCommand::Engines { .. } => ("hub.engines", json!({})),
        HubCommand::Uninstall { version } => ("hub.engine.uninstall", json!({"version":version})),
        HubCommand::Cancel { job } => ("hub.download.cancel", json!({"job":job})),
        HubCommand::RegisterEngine { .. } | HubCommand::UnregisterEngine { .. } => {
            let config = ara_hub_service::HubConfig::user(executable).map_err(io_error)?;
            (
                if matches!(command, HubCommand::RegisterEngine { .. }) {
                    "hub.engine.register"
                } else {
                    "hub.engine.remove"
                },
                json!({"engine":config.engine}),
            )
        }
        HubCommand::Shutdown => ("hub.shutdown", json!({})),
        HubCommand::Activity => ("hub.activity", json!({})),
        HubCommand::Install { .. }
        | HubCommand::Repair { .. }
        | HubCommand::InstallMcp { .. }
        | HubCommand::Serve { .. } => {
            unreachable!("handled above")
        }
    };
    let result = client.call(method, params)?;
    println!("{}", ara_json::to_canonical_string(&result));
    Ok(true)
}

fn available_releases(
    client: &mut ara_rpc::Client,
    channel: &str,
) -> Result<serde_json::Value, RpcError> {
    client.call("hub.releases.check", json!({}))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(240);
    loop {
        let result = client.call("hub.engines.available", json!({"channel":channel}))?;
        if result["checking"] != true {
            return Ok(result);
        }
        if std::time::Instant::now() >= deadline {
            return Err(io_error("release check still running; retry shortly"));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

fn io_error(error: impl std::fmt::Display) -> RpcError {
    Diagnostic::error(codes::IO, error.to_string()).into()
}

pub fn global_mcp() -> Result<bool, String> {
    let directory = ara_hub_service::config_dir().map_err(|e| e.to_string())?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut hub =
        ara_hub_service::connect_or_start(&directory, &executable).map_err(|e| e.to_string())?;
    ara_mcp::serve_global_stdio(&mut hub, &mut io::stdin().lock(), &mut io::stdout().lock())
        .map_err(|e| e.to_string())?;
    Ok(true)
}
