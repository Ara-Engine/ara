//! Parent-bound compiler transport reused across preparation calls, never simulation state.
use super::{ToolPaths, io};
use ara_diag::Diagnostic;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Mutex, OnceLock, mpsc},
    time::Duration,
};

struct Session {
    identity: Vec<(PathBuf, String)>,
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Result<Value, Diagnostic>>,
}

pub(super) fn invoke<T: DeserializeOwned>(
    root: &Path,
    mut request: Value,
) -> Result<T, Diagnostic> {
    static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    let tools =
        ToolPaths::locate(root).ok_or_else(|| failure("installed C# project tools are missing"))?;
    let directory = tools
        .compile_server
        .parent()
        .ok_or_else(|| failure("compiler needs a directory"))?;
    let mut identity = Vec::new();
    for name in [
        "Ara.CompileServer.dll",
        "Ara.Runtime.dll",
        "Ara.Generators.dll",
        "Ara.Analyzers.dll",
        "Ara.Editor.dll",
    ] {
        let path = directory.join(name);
        let hash = ara_doc::file_state::FileState::bytes(&std::fs::read(&path).map_err(io)?).hash;
        identity.push((path, hash));
    }
    request["project"] = root.to_string_lossy().into_owned().into();
    let mut session = SESSION
        .get_or_init(|| Mutex::new(None))
        .lock()
        .map_err(|_| failure("C# project transport lock was poisoned"))?;
    if session
        .as_ref()
        .is_none_or(|session| session.identity != identity)
    {
        *session = Some(Session::start(directory, &tools.compile_server, identity)?);
    }
    let result = session
        .as_mut()
        .expect("compiler session was started")
        .call(request);
    if result.is_err() {
        *session = None;
    }
    serde_json::from_value(result?).map_err(io)
}

impl Session {
    fn start(
        directory: &Path,
        compiler: &Path,
        identity: Vec<(PathBuf, String)>,
    ) -> Result<Self, Diagnostic> {
        let mut command = Command::new("dotnet");
        command
            .arg("exec")
            .arg(compiler)
            .args(["--project-session", "--parent"])
            .arg(std::process::id().to_string())
            .current_dir(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_remove("DOTNET_MODIFIABLE_ASSEMBLIES");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000 | 0x0000_4000);
        }
        let mut child = command.spawn().map_err(io)?;
        let input = child.stdin.take().expect("piped compiler input");
        let stdout = child.stdout.take().expect("piped compiler output");
        let mut stderr = child.stderr.take().expect("piped compiler errors");
        std::thread::spawn(move || {
            let _ = std::io::copy(&mut stderr, &mut std::io::sink());
        });
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let value = line
                    .map_err(io)
                    .and_then(|text| serde_json::from_str(&text).map_err(io));
                if send.send(value).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            identity,
            child,
            input,
            output,
        })
    }

    fn call(&mut self, request: Value) -> Result<Value, Diagnostic> {
        let mut bytes = serde_json::to_vec(&request).map_err(io)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).map_err(io)?;
        self.input.flush().map_err(io)?;
        self.output
            .recv_timeout(Duration::from_secs(90))
            .map_err(|error| {
                failure(&format!(
                    "C# project worker exited or exceeded its 90 second budget: {error}"
                ))
            })?
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill(); // exact owned child process
        let _ = self.child.wait();
    }
}

fn failure(message: &str) -> Diagnostic {
    Diagnostic::error(crate::codes::COMPILE_SERVER, message)
}
