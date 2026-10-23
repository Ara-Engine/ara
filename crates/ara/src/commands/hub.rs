//! User-level Hub commands and global MCP installation snippets.
use ara_proto::Endpoint;
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
    Engines,
    /// Register this installed engine with the running user Hub.
    RegisterEngine,
    /// Unregister this exact installed build, preserving projects and other versions.
    UnregisterEngine,
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

pub fn hub(command: HubCommand) -> Result<bool, String> {
    if let HubCommand::InstallMcp { client } = command {
        match client {
            McpClient::Claude => println!(
                "{}",
                ara_json::to_canonical_string(
                    &json!({"mcpServers":{"ara":{"command":"ara","args":["mcp"]}}})
                )
            ),
            McpClient::Codex => println!("[mcp_servers.ara]\ncommand = \"ara\"\nargs = [\"mcp\"]"),
        }
        return Ok(true);
    }
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    if let HubCommand::Serve { port, config_dir } = command {
        let mut config = ara_hub_service::HubConfig::user(executable).map_err(|e| e.to_string())?;
        if let Some(directory) = config_dir {
            if !config
                .engine
                .executable
                .parent()
                .is_some_and(|dir| dir.join("templates").is_dir())
            {
                config.templates = Box::new(ara_hub_service::DirectoryTemplates(
                    directory.join("templates"),
                ));
            }
            config.directory = directory;
        }
        let log = config.directory.join("hub-start.log");
        if let Err(error) = ara_hub_service::serve(config, port) {
            let _ = std::fs::write(log, error.to_string());
            return Err(error.to_string());
        }
        return Ok(true);
    }
    let directory = ara_hub_service::config_dir().map_err(|e| e.to_string())?;
    let mut client =
        ara_hub_service::connect_or_start(&directory, &executable).map_err(|e| e.to_string())?;
    let (method, params) = match command {
        HubCommand::Add { path, name } => (
            "hub.register",
            json!({"path":std::path::absolute(path).map_err(|e|e.to_string())?,"name":name}),
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
            json!({"path":std::path::absolute(path).map_err(|e|e.to_string())?,"name":name,"template":template}),
        ),
        HubCommand::Open { project, headless } => {
            ("hub.open", json!({"project":project,"headless":headless}))
        }
        HubCommand::Close { project } => ("hub.close", json!({"project":project})),
        HubCommand::Status { project } => ("hub.status", json!({"project":project})),
        HubCommand::Engines => ("hub.engines", json!({})),
        HubCommand::RegisterEngine | HubCommand::UnregisterEngine => {
            let config = ara_hub_service::HubConfig::user(executable).map_err(|e| e.to_string())?;
            (
                if matches!(command, HubCommand::RegisterEngine) {
                    "hub.engine.register"
                } else {
                    "hub.engine.remove"
                },
                json!({"engine":config.engine}),
            )
        }
        HubCommand::Shutdown => ("hub.shutdown", json!({})),
        HubCommand::Activity => ("hub.activity", json!({})),
        HubCommand::InstallMcp { .. } | HubCommand::Serve { .. } => unreachable!("handled above"),
    };
    let result = client.call(method, params).map_err(|e| e.to_string())?;
    println!("{}", ara_json::to_canonical_string(&result));
    Ok(true)
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
