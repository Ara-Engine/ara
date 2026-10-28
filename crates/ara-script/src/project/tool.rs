//! Bounded, windowless installed SDK tool processes shared by project and editor preparation.
use super::{ToolPaths, io};
use ara_diag::Diagnostic;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

pub(super) fn run<T: DeserializeOwned>(
    root: &Path,
    mode: &str,
    request: Value,
) -> Result<T, Diagnostic> {
    let failure = |message| Diagnostic::error(crate::codes::COMPILE_SERVER, message);
    let request = serde_json::to_vec(&request).map_err(io)?;
    let tools = ToolPaths::locate(root).ok_or_else(|| failure("the installed C# compile tools are missing; build dotnet/Ara.slnx or repair the engine SDK".into()))?;
    let mut command = Command::new("dotnet");
    command
        .arg("exec")
        .arg(&tools.compile_server)
        .arg(mode)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env_remove("DOTNET_MODIFIABLE_ASSEMBLIES");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000 | 0x0000_4000); // no window, BelowNormal
    }
    let mut child = command.spawn().map_err(io)?;
    let output = drain(
        child
            .stdout
            .take()
            .ok_or_else(|| io("C# tool has no output pipe"))?,
    );
    let errors = drain(
        child
            .stderr
            .take()
            .ok_or_else(|| io("C# tool has no error pipe"))?,
    );
    let input = child
        .stdin
        .take()
        .expect("C# tool input was piped")
        .write_all(&request);
    if let Err(error) = input {
        let _ = child.kill();
        let _ = child.wait();
        return Err(io(error));
    }
    let deadline = Instant::now() + Duration::from_secs(90);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io(error));
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill(); // only this owned child process
            let _ = child.wait();
            return Err(failure(format!(
                "C# tool {mode} exceeded its 90 second preparation budget"
            )));
        }
        thread::sleep(Duration::from_millis(5));
    };
    let output = output
        .recv_timeout(Duration::from_secs(1))
        .map_err(io)?
        .map_err(io)?;
    let errors = errors
        .recv_timeout(Duration::from_secs(1))
        .map_err(io)?
        .map_err(io)?;
    if !status.success() {
        return Err(failure(String::from_utf8_lossy(&errors).trim().to_owned()));
    }
    serde_json::from_slice(&output)
        .map_err(|error| failure(format!("invalid C# tool result: {error}")))
}

fn drain(mut input: impl Read + Send + 'static) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (send, receive) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = input.read_to_end(&mut bytes).map(|_| bytes);
        let _ = send.send(result);
    });
    receive
}
