//! Cross-import device reuse; bulk clients and host lifetime leases are separate.
mod binary;
mod client;
mod server;
#[cfg(test)]
mod tests;
mod wire;

use ara_asset::{BlockEncoder, Digest, Error, Result};
use std::{
    fs::OpenOptions,
    io::Write,
    net::TcpListener,
    path::Path,
    process::{Command, Stdio},
    sync::{Arc, mpsc},
    thread,
    time::Duration,
};
pub(crate) const ARGUMENT: &str = "--ara-texture-gpu-service";

pub(crate) fn open(cooked: &Path, cache: &Path) -> Result<Box<dyn BlockEncoder>> {
    // Identity comes from the same immutable manifests as local encoding. This
    // reads files only; the disposable worker never creates a Vulkan device.
    let identity = crate::Encoder::new(cooked, cache)?.backend().to_owned();
    let binary = binary::Binary::load(cache)?;
    std::fs::create_dir_all(cache)?;
    let key = Digest::of(format!("{identity}:{}", binary.digest).as_bytes());
    let path = cache.join(format!("service-{key}.json"));
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(cache.join(format!("service-{key}.lock")))?;
    lock.lock()?;
    if let Ok(bytes) = std::fs::read(&path)
        && let Ok(descriptor) = serde_json::from_slice::<wire::Descriptor>(&bytes)
        && descriptor.identity == identity
        && let Ok(remote) = client::Remote::connect(descriptor)
    {
        return Ok(Box::new(remote));
    }
    let descriptor = spawn(cooked, cache, &binary)?;
    if descriptor.identity != identity {
        return Err(Error::EncoderUnavailable(
            "spawned codec revision changed".into(),
        ));
    }
    let value = serde_json::to_value(&descriptor)?;
    let bytes = ara_json::to_canonical_string(&ara_json::sort_keys(&value));
    let mut temporary = tempfile::NamedTempFile::new_in(cache)?;
    temporary.write_all(bytes.as_bytes())?;
    temporary.persist(path).map_err(|e| Error::Io(e.error))?;
    Ok(Box::new(client::Remote::connect(descriptor)?))
}

fn spawn(cooked: &Path, cache: &Path, binary: &binary::Binary) -> Result<wire::Descriptor> {
    let mut command = Command::new(binary.publish()?);
    command
        .arg(ARGUMENT)
        .arg(cooked)
        .arg(cache)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(
            windows_sys::Win32::System::Threading::CREATE_NO_WINDOW
                | windows_sys::Win32::System::Threading::BELOW_NORMAL_PRIORITY_CLASS,
        );
    }
    let mut child = command.spawn()?;
    let mut stdout = child
        .stdout
        .take()
        .expect("service command configures stdout");
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let _ = tx.send(wire::read_json(&mut stdout));
    });
    match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(Ok(descriptor)) => Ok(descriptor),
        result => {
            let _ = child.kill();
            let _ = child.wait();
            Err(Error::EncoderUnavailable(match result {
                Ok(Err(error)) => error.to_string(),
                _ => "codec service startup timed out".into(),
            }))
        }
    }
}

pub(crate) fn run() -> Result<()> {
    let mut args = std::env::args_os()
        .skip_while(|arg| arg != ARGUMENT)
        .skip(1);
    let cooked = args
        .next()
        .ok_or_else(|| Error::Invalid("codec service needs a cooked directory".into()))?;
    let cache = args
        .next()
        .ok_or_else(|| Error::Invalid("codec service needs a cache directory".into()))?;
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let encoders: Vec<Box<dyn BlockEncoder>> = (0..2)
        .map(|index| {
            crate::Encoder::new(
                Path::new(&cooked),
                &Path::new(&cache).join(format!("device-{index}")),
            )
            .map(|encoder| Box::new(encoder) as Box<dyn BlockEncoder>)
        })
        .collect::<Result<_>>()?;
    if encoders
        .iter()
        .any(|encoder| encoder.backend() != encoders[0].backend())
    {
        return Err(Error::EncoderUnavailable(
            "codec cohort changed while opening device pool".into(),
        ));
    }
    let descriptor = server::descriptor(&listener, encoders[0].backend().to_owned())?;
    ara_asset::record_import_trace(
        "texture.service.start",
        "gpu-codec",
        serde_json::json!({"slots":encoders.len(),"executable":std::env::current_exe()?}),
    );
    wire::write_json(&mut std::io::stdout().lock(), &descriptor)?;
    let pool = Arc::new(server::Pool::new(encoders));
    let result = server::serve(
        listener,
        &descriptor,
        Arc::clone(&pool),
        Duration::from_secs(2),
    );
    let initialized = pool.initialized();
    drop(pool);
    ara_asset::record_import_trace(
        "texture.service.exit",
        "gpu-codec",
        serde_json::json!({"devicesCreated":initialized}),
    );
    result
}
