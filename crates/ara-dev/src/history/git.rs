//! Git owns hashing/compression; only the newly cloned bare repository is writable.
use anyhow::{Context, Result, ensure};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub(super) fn command() -> Command {
    let mut command = Command::new("git");
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_REPLACE_REF_BASE",
        "GIT_NAMESPACE",
        "GIT_CONFIG_COUNT",
    ] {
        command.env_remove(variable);
    }
    command.env("GIT_NO_REPLACE_OBJECTS", "1");
    command.env("GIT_TERMINAL_PROMPT", "0");
    command.args(["-c", "gc.auto=0", "-c", "core.hooksPath=/dev/null"]);
    command
}

pub(super) struct Git(pub PathBuf);
impl Git {
    fn command(&self, args: &[&str]) -> Command {
        let mut c = command();
        c.current_dir(&self.0).args(args);
        c
    }
    pub fn run(&self, args: &[&str]) -> Result<Vec<u8>> {
        self.input(args, &[])
    }
    pub fn text(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8(self.run(args)?)?
            .trim_end_matches('\n')
            .into())
    }
    pub fn input(&self, args: &[&str], input: &[u8]) -> Result<Vec<u8>> {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        child.stdin.take().context("Git input")?.write_all(input)?;
        let output = child.wait_with_output()?;
        ensure!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(output.stdout)
    }
    pub fn write(&self, kind: &str, bytes: &[u8]) -> Result<String> {
        Ok(
            String::from_utf8(self.input(&["hash-object", "-w", "-t", kind, "--stdin"], bytes)?)?
                .trim()
                .into(),
        )
    }
    pub fn fsck(&self) -> Result<Vec<u8>> {
        let output = self
            .command(&["fsck", "--full", "--no-dangling", "--no-reflogs"])
            .output()?;
        let log = [output.stdout, output.stderr].concat();
        ensure!(
            output.status.success(),
            "git fsck failed: {}",
            String::from_utf8_lossy(&log)
        );
        Ok(log)
    }
    pub fn batch(&self) -> Result<Batch> {
        let mut child = self
            .command(&["cat-file", "--batch"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let input = child.stdin.take().context("cat-file input")?;
        let output = BufReader::new(child.stdout.take().context("cat-file output")?);
        Ok(Batch {
            child,
            input: Some(input),
            output,
        })
    }
    pub fn validate_isolation(&self) -> Result<()> {
        ensure!(
            self.text(&["rev-parse", "--is-bare-repository"])? == "true",
            "requires the newly created bare mirror"
        );
        ensure!(
            self.text(&["rev-parse", "--is-shallow-repository"])? == "false",
            "shallow history cannot be rewritten"
        );
        ensure!(
            !self.0.join("objects/info/alternates").exists(),
            "shared object stores are forbidden"
        );
        ensure!(
            !self.0.join("commondir").exists() && !self.0.join("worktrees").exists(),
            "attached/shared worktrees are forbidden"
        );
        ensure!(
            self.text(&["config", "--get", "remote.origin.pushurl"])?
                == "disabled://history-rehearsal",
            "push destination must be disabled"
        );
        ensure!(
            self.text(&["symbolic-ref", "HEAD"])?.starts_with("refs/"),
            "mirror HEAD must be symbolic"
        );
        Ok(())
    }
}

pub(super) struct Object {
    pub kind: String,
    pub bytes: Vec<u8>,
}
pub(super) struct Batch {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
}
impl Batch {
    pub fn read(&mut self, oid: &str) -> Result<Object> {
        let input = self.input.as_mut().context("cat-file stdin")?;
        writeln!(input, "{oid}")?;
        input.flush()?;
        let mut header = String::new();
        self.output.read_line(&mut header)?;
        let fields: Vec<_> = header.split_whitespace().collect();
        ensure!(
            fields.len() == 3 && fields[0] == oid,
            "unexpected cat-file response: {header}"
        );
        let size = fields[2].parse::<usize>()?;
        let mut bytes = vec![0; size];
        self.output.read_exact(&mut bytes)?;
        let mut newline = [0];
        self.output.read_exact(&mut newline)?;
        ensure!(newline == *b"\n", "invalid cat-file framing");
        Ok(Object {
            kind: fields[1].into(),
            bytes,
        })
    }
}
impl Drop for Batch {
    fn drop(&mut self) {
        self.input.take();
        let _ = self.child.wait();
    }
}

/// Clone transfers objects rather than linking them to any live checkout.
pub(super) fn clone(source: &str, destination: &Path) -> Result<Git> {
    let output = command()
        .args([
            "-c",
            "pack.threads=2",
            "clone",
            "--mirror",
            "--no-local",
            "--",
        ])
        .arg(git_path(Path::new(source))?)
        .arg(git_path(destination)?)
        .output()?;
    ensure!(
        output.status.success(),
        "mirror clone failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let git = Git(destination.canonicalize()?);
    git.run(&[
        "config",
        "remote.origin.pushurl",
        "disabled://history-rehearsal",
    ])?;
    git.run(&["config", "remote.origin.mirror", "false"])?;
    git.validate_isolation()?;
    Ok(git)
}

/// Git for Windows cannot create a mirror using Rust's verbatim path prefix.
fn git_path(path: &Path) -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let text = path.to_str().context("non-Unicode Git path")?;
        if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
            return Ok(PathBuf::from(format!(r"\\{unc}")));
        }
        if let Some(local) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(local));
        }
    }
    Ok(path.to_path_buf())
}
