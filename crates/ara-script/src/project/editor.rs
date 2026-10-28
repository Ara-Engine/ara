//! Persistent, hidden editor worker. Construct and call only on a preparation thread.
use super::{Inventory, ToolPaths, io, publish_outputs};
use ara_diag::Diagnostic;
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

/// One project's editor extension process, retaining its last good loaded generation.
/// Callbacks receive immutable snapshots and return typed commands, never authority access.
pub struct EditorClient {
    root: PathBuf,
    child: Child,
    input: ChildStdin,
    output: mpsc::Receiver<Result<Value, Diagnostic>>,
}

impl EditorClient {
    /// Start the installed SDK worker without creating a window. Owned process only.
    pub fn start(root: &Path) -> Result<Self, Diagnostic> {
        let root = std::fs::canonicalize(root).map_err(io)?;
        let tools = ToolPaths::locate(&root)
            .ok_or_else(|| failure("installed C# editor tools are missing"))?;
        let mut command = Command::new("dotnet");
        command
            .arg("exec")
            .arg(tools.compile_server)
            .arg("--editor-session")
            .arg("--parent")
            .arg(std::process::id().to_string())
            .current_dir(&root)
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
        let input = child.stdin.take().expect("piped editor input");
        let stdout = child.stdout.take().expect("piped editor output");
        let mut stderr = child.stderr.take().expect("piped editor errors");
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
            root,
            child,
            input,
            output,
        })
    }

    /// Compile/register (`operation: catalog`) or invoke a loaded command over a
    /// scene/selection snapshot. Failed catalog candidates retain the previous generation.
    /// Published schema paths/results use the same inventory flow as project opening.
    pub fn call(&mut self, mut request: Value) -> Result<Value, Diagnostic> {
        self.call_while(&mut request, || false)
    }

    /// As `call`, with cancellation checked while the owned worker runs. Cancellation
    /// stops only this child and lets an editor close without a lingering compiler.
    pub fn call_cancelled(
        &mut self,
        mut request: Value,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Value, Diagnostic> {
        self.call_while(&mut request, || {
            cancelled.load(std::sync::atomic::Ordering::Relaxed)
        })
    }

    fn call_while(
        &mut self,
        request: &mut Value,
        cancelled: impl Fn() -> bool,
    ) -> Result<Value, Diagnostic> {
        let _publication = super::lock::PublicationLock::acquire(&self.root, &cancelled)?;
        let fields = request
            .as_object_mut()
            .ok_or_else(|| failure("editor request must be an object"))?;
        fields.insert("project".into(), json!(self.root));
        fields.insert("authoringRoot".into(), ara_doc::AUTHORING_ROOT.into());
        fields.insert(
            "diagnosticContext".into(),
            super::lifecycle::context(&self.root, "editor")?,
        );
        let mut bytes = serde_json::to_vec(request).map_err(io)?;
        bytes.push(b'\n');
        self.input.write_all(&bytes).map_err(io)?;
        self.input.flush().map_err(io)?;
        let deadline = std::time::Instant::now() + Duration::from_secs(90);
        let mut result = loop {
            if cancelled() || std::time::Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                return Err(failure(
                    "C# editor worker was cancelled or exceeded its 90 second budget",
                ));
            }
            match self.output.recv_timeout(Duration::from_millis(50)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(error) => return Err(failure(&format!("C# editor worker exited: {error}"))),
            }
        };
        if !result["inventory"].is_null() {
            let inventory: Inventory =
                serde_json::from_value(result["inventory"].take()).map_err(io)?;
            result["inventory"] =
                serde_json::to_value(publish_outputs(&self.root, inventory)?).map_err(io)?;
        }
        Ok(result)
    }
}

impl Drop for EditorClient {
    fn drop(&mut self) {
        let _ = self.child.kill(); // exact owned child; no process-name matching
        let _ = self.child.wait();
    }
}
fn failure(message: &str) -> Diagnostic {
    Diagnostic::error(crate::codes::COMPILE_SERVER, message)
}
