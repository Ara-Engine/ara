//! Installed entry points must succeed without shell integration and preserve user data.
use super::*;

pub(super) fn no_autostart(env: &Environment) -> Result<()> {
    ensure!(
        ara_rpc::DaemonInfo::read(&env.directory.join("config/hub")).is_none(),
        "no-system-integration setup started a Hub service"
    );
    let registry = env.directory.join("config/registry.json");
    let registry: Value = serde_json::from_slice(&fs::read(registry)?)?;
    ensure!(
        registry["engines"]
            .as_array()
            .is_some_and(|engines| engines.iter().any(|engine| engine["executable"]
                .as_str()
                .is_some_and(|path| Path::new(path) == env.installed.join("ara.exe")))),
        "engine was not registered without starting the service"
    );
    Ok(())
}

pub(super) fn mcp_config(env: &Environment) -> Result<()> {
    let expected = env.installed.join("ara.exe");
    for client in ["claude", "codex"] {
        let log = format!("mcp-config-{client}.log");
        env.cli(&["hub", "install-mcp", "--client", client], &log)?;
        let text = fs::read_to_string(env.directory.join(log))?;
        let command = if client == "claude" {
            let value: Value = serde_json::from_str(&text)?;
            ensure!(
                value["mcpServers"]["ara"]["args"] == json!(["mcp"]),
                "MCP arguments changed"
            );
            value["mcpServers"]["ara"]["command"]
                .as_str()
                .context("Claude command")?
                .to_owned()
        } else {
            let value = text
                .lines()
                .find_map(|line| line.strip_prefix("command = "))
                .context("Codex command")?;
            serde_json::from_str::<String>(value)?
        };
        ensure!(
            Path::new(&command).is_absolute()
                && fs::canonicalize(&command)? == fs::canonicalize(&expected)?,
            "{client} MCP config does not use the installed executable: {text}"
        );
    }
    no_autostart(env)
}

/// Snapshot representative projects, user-added installation files and preferences.
pub(super) struct Preserved {
    files: Vec<(PathBuf, Vec<u8>)>,
    projects: Value,
    owned: Vec<PathBuf>,
}
impl Preserved {
    pub(super) fn before_uninstall(env: &Environment) -> Result<Self> {
        let marker = env.installed.join("My projects/keep.txt");
        fs::create_dir_all(marker.parent().context("user project folder")?)?;
        fs::write(&marker, b"User-added project: preserve on uninstall")?;
        let prefs = env.directory.join("config/hub-ui.json");
        support::json(
            &prefs,
            &json!({"light":true,"sort":"name","layout":"list","agent":"codex"}),
        )?;
        let crash = env.directory.join("crashes/keep.txt");
        fs::create_dir_all(crash.parent().context("crash folder")?)?;
        fs::write(&crash, b"Local crash reports stay on disk")?;
        let mut files = vec![
            (marker.clone(), fs::read(marker)?),
            (prefs.clone(), fs::read(prefs)?),
            (crash.clone(), fs::read(crash)?),
        ];
        for project in ["starter project", "Standalone Starter", "Starter export"] {
            let marker = env.directory.join(project).join("ara.project.json");
            if marker.is_file() {
                files.push((marker.clone(), fs::read(marker)?));
            }
        }
        let registry: Value =
            serde_json::from_slice(&fs::read(env.directory.join("config/registry.json"))?)?;
        let inventory: Value =
            serde_json::from_slice(&fs::read(env.installed.join("files.json"))?)?;
        let owned = inventory["files"]
            .as_array()
            .context("installed inventory")?
            .iter()
            .map(|file| {
                file["path"]
                    .as_str()
                    .map(|p| env.installed.join(p))
                    .context("inventory path")
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            files,
            projects: registry["projects"].clone(),
            owned,
        })
    }
    pub(super) fn after_uninstall(self, env: &Environment) -> Result<()> {
        for path in self.owned.into_iter().chain([
            env.installed.join("files.json"),
            env.installed.join("install-receipt.json"),
            env.installed.join("uninstall.cmd"),
        ]) {
            ensure!(
                !path.exists(),
                "uninstall left an owned file: {}",
                path.display()
            );
        }
        for (path, bytes) in self.files {
            ensure!(
                fs::read(&path).with_context(|| format!("preserved file {}", path.display()))?
                    == bytes,
                "uninstall changed user data: {}",
                path.display()
            );
        }
        let registry: Value =
            serde_json::from_slice(&fs::read(env.directory.join("config/registry.json"))?)?;
        ensure!(
            registry["projects"] == self.projects,
            "uninstall changed registered projects"
        );
        Ok(())
    }
}

pub(super) fn uninstall(env: &Environment) -> Result<()> {
    let preserved = Preserved::before_uninstall(env)?;
    let path = env.installed.join("uninstall.cmd");
    let mut command = env.command("cmd.exe");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(format!("/d /s /c \"\"{}\" -Silent\"", path.display()));
    }
    #[cfg(not(windows))]
    command.args(["/d", "/c"]).arg(path).arg("-Silent");
    run(&mut command, &env.directory.join("uninstall.log"))?;
    preserved.after_uninstall(env)
}
