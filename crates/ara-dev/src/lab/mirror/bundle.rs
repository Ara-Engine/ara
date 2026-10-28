//! Changed immutable files travel in one archive; publication happens under the peer's visual lock.
mod packet;
use super::{Config, transport};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

struct File {
    source: PathBuf,
    stamp: Value,
}
impl File {
    fn new(source: PathBuf, metadata: fs::Metadata) -> Result<Self> {
        let stamp = json!([
            source,
            metadata.len(),
            metadata
                .modified()?
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
                .to_string()
        ]);
        Ok(Self { source, stamp })
    }
}

fn relative(path: &str) -> Result<&Path> {
    let p = Path::new(path);
    ensure!(
        !p.is_absolute()
            && !path.contains(['\\', '\n', '\r', '\0'])
            && p.components()
                .all(|c| matches!(c, std::path::Component::Normal(_))),
        "unsafe mirror artifact path {path}"
    );
    Ok(p)
}
fn inventory(source: &Path, prefix: &str, files: &mut BTreeMap<String, File>) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        relative(&name)?;
        if prefix.starts_with("packs/")
            && (entry
                .path()
                .extension()
                .is_some_and(|e| ["exe", "dll", "pdb", "dylib"].iter().any(|x| e == *x))
                || name.ends_with("/game/scripts.json"))
        {
            continue;
        }
        if ty.is_dir() {
            inventory(&entry.path(), &name, files)?;
        } else if ty.is_file() {
            files.insert(name, File::new(entry.path(), entry.metadata()?)?);
        }
    }
    Ok(())
}
fn shaders(source: &Path, prefix: &str, files: &mut BTreeMap<String, File>) -> Result<()> {
    if !source.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let ty = entry.file_type()?;
        if ty.is_symlink() {
            continue;
        }
        let name = format!("{prefix}/{}", entry.file_name().to_string_lossy());
        relative(&name)?;
        if ty.is_dir() {
            // Active metadata adds its own content-addressed artifacts below.
            let component = entry.file_name();
            let component = component.to_string_lossy();
            if component.len() == 64 && component.bytes().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            shaders(&path, &name, files)?;
            continue;
        }
        if path
            .file_name()
            .is_some_and(|n| n == ".ara-shader-reload" || n == "atlas.rgba" || n == "atlas.png")
        {
            files.insert(name, File::new(path, entry.metadata()?)?);
            continue;
        }
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(v) = serde_json::from_slice::<Value>(&fs::read(&path)?)
            && let Some(stages) = v["stages"].as_array()
        {
            files.insert(name, File::new(path, entry.metadata()?)?);
            for stage in stages {
                ensure!(
                    stage["metal"].is_object(),
                    "PC cooked shader has no Metal output; install the optional PC Metal converter and recook before mirroring {prefix}"
                );
                for field in [
                    &stage["spirv"],
                    &stage["metal"]["metallib"],
                    &stage["metal"]["reflection"],
                ] {
                    let file = field.as_str().context("cooked stage artifact missing")?;
                    let src = source.join(relative(file)?);
                    let metadata = fs::metadata(&src).with_context(|| {
                        format!("PC cooked artifact missing: {}", src.display())
                    })?;
                    ensure!(
                        metadata.is_file(),
                        "PC cooked artifact missing: {}",
                        src.display()
                    );
                    files.insert(format!("{prefix}/{file}"), File::new(src, metadata)?);
                }
            }
        }
    }
    Ok(())
}
pub(super) struct Transfer {
    /// Cold/source synchronization imports only after the peer has bootstrapped.
    pub staged_import: Option<Value>,
    pub timing: Value,
}

pub(super) fn push(
    root: &Path,
    base: &Path,
    config: &Config,
    activity: &super::super::track::Activity,
    activate: bool,
) -> Result<Transfer> {
    let started = Instant::now();
    let _snapshot = activity.gpu.read().expect("lab publication");
    let mut files = BTreeMap::new();
    shaders(&base.join("shaders"), "shaders", &mut files)?;
    if let Some(variants) = super::super::session::read(&base.join("variants.json"))
        && let Some(variants) = variants.as_object()
    {
        for name in variants.keys() {
            super::super::session::name(name)?;
            shaders(
                &base.join("variants").join(name).join("shaders"),
                &format!("variants/{name}/shaders"),
                &mut files,
            )?;
        }
    }
    inventory(&base.join("references"), "references", &mut files)?;
    inventory(
        &PathBuf::from("D:/AraBench/report/images/showcase/cycles"),
        "references-ldr/showcase/cycles",
        &mut files,
    )?;
    let binaries = super::super::session::read(&base.join("binaries.json"))
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default();
    for scene in ["showcase", "outpost", "hangar"] {
        let selection =
            super::super::scenes::select(root, base, &json!({"scene":scene}), &binaries)?;
        let cache_name = if scene == "showcase" {
            "showcase-benchmark"
        } else {
            scene
        };
        inventory(&selection.pack, &format!("packs/{cache_name}"), &mut files)?;
    }
    let mut previous =
        super::super::session::read(&base.join("mirror/uploaded.json")).unwrap_or(json!({}));
    let configuration = json!({"host":config.host,"repo":config.repo});
    if previous["_configuration"]["host"] != configuration["host"]
        || previous["_configuration"]["repo"] != configuration["repo"]
    {
        previous = json!({});
    }
    let current = files
        .iter()
        .map(|(name, file)| (name.clone(), file.stamp.clone()))
        .collect::<BTreeMap<_, _>>();
    let changed = files
        .iter()
        .filter(|(name, _)| previous[*name] != current[*name])
        .collect::<Vec<_>>();
    let variants = super::super::session::read(&base.join("variants.json")).unwrap_or(json!({}));
    let claims = super::super::session::read(&base.join("claims.json")).unwrap_or(json!({}));
    let shaders_current = activity
        .shaders_latest
        .load(std::sync::atomic::Ordering::Acquire);
    if changed.is_empty()
        && previous["_variants"] == variants
        && previous["_shaders_current"] == shaders_current
        && previous["_claims"] == claims
    {
        return Ok(Transfer {
            staged_import: if activate {
                None
            } else {
                super::super::session::read(&base.join("mirror/incoming-local.json"))
            },
            timing: json!({"changed":false,"inventory_ms":started.elapsed().as_secs_f64()*1000.,"connections":0}),
        });
    }
    let job = transport::job(base)?;
    let data = job.join("data");
    fs::create_dir_all(&data)?;
    let inventory_ms = started.elapsed().as_secs_f64() * 1000.;
    let mut names = Vec::new();
    for (name, file) in changed {
        let target = data.join(relative(name)?);
        fs::create_dir_all(target.parent().context("artifact parent")?)?;
        if fs::hard_link(&file.source, &target).is_err() {
            fs::copy(&file.source, &target)?;
        }
        names.push(name.as_str());
    }
    drop(_snapshot);
    let inbox = format!("mirror/inbox/{}", super::super::session::id());
    let import = json!({"directory":inbox,"variants":portable(variants.clone()),"claims":portable(claims.clone()),"shaders_current":shaders_current});
    let packing = Instant::now();
    let archive = packet::archive(&job, &names, &import)?;
    let archive_ms = packing.elapsed().as_secs_f64() * 1000.;
    let wire = Instant::now();
    let output = transport::stream(
        config,
        base,
        &archive,
        &packet::receiver(config, &inbox, activate),
    )?;
    if activate {
        transport::response(&output)?;
    } else {
        ensure!(
            serde_json::from_str::<Value>(&output)?["received"] == true,
            "Mac did not acknowledge staged artifacts"
        );
    }
    let timing = json!({"changed":true,"files":names.len(),"bytes":fs::metadata(&archive)?.len(),"inventory_ms":inventory_ms,"archive_ms":archive_ms,"wire_ms":wire.elapsed().as_secs_f64()*1000.,"total_ms":started.elapsed().as_secs_f64()*1000.,"connections":1,"activated":activate});
    super::super::session::write(&job.join("transfer.json"), &timing)?;
    super::super::session::write(&base.join("mirror/incoming-local.json"), &import)?;
    let mut uploaded = serde_json::to_value(current)?;
    uploaded["_variants"] = variants;
    uploaded["_shaders_current"] = json!(shaders_current);
    uploaded["_claims"] = claims;
    uploaded["_configuration"] = configuration;
    super::super::session::write(&base.join("mirror/uploaded.json"), &uploaded)?;
    Ok(Transfer {
        staged_import: (!activate).then_some(import),
        timing,
    })
}

pub(super) fn portable(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(s.replace('\\', "/")),
        Value::Array(a) => Value::Array(a.into_iter().map(portable).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (k.replace('\\', "/"), portable(v)))
                .collect(),
        ),
        v => v,
    }
}

/// Import is called only on the Mac, with its activity write lock held.
pub(in crate::lab) fn import(base: &Path, args: &Value) -> Result<()> {
    let path = relative(
        args["directory"]
            .as_str()
            .context("mirror inbox directory")?,
    )?;
    ensure!(
        path.starts_with("mirror/inbox"),
        "expected lab mirror inbox"
    );
    super::super::variants::publish_tree(&base.join(path).join("data"), base)?;
    super::super::session::write(&base.join("variants.json"), &args["variants"])?;
    super::super::session::write(&base.join("claims.json"), &args["claims"])
}

pub(super) fn pull(config: &Config, base: &Path, value: &mut Value) -> Result<()> {
    let remote = format!("{}/target/ara-dev/lab/", config.repo);
    let mut paths = std::collections::BTreeSet::new();
    fn collect(
        v: &Value,
        prefix: &str,
        out: &mut std::collections::BTreeSet<String>,
    ) -> Result<()> {
        match v {
            Value::String(s) => {
                if let Some(p) = s.strip_prefix(prefix) {
                    relative(p)?;
                    if ["captures/", "measurements/", "sessions/"]
                        .iter()
                        .any(|root| p.starts_with(root))
                    {
                        out.insert(p.into());
                    }
                }
            }
            Value::Array(a) => {
                for v in a {
                    collect(v, prefix, out)?;
                }
            }
            Value::Object(o) => {
                for v in o.values() {
                    collect(v, prefix, out)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    collect(value, &remote, &mut paths)?;
    // Reports include directories as provenance. Only collect existing regular files.
    let job = transport::job(base)?;
    let remote_archive = format!(
        "{remote}mirror/download-{}.tar",
        super::super::session::id()
    );
    let list = paths
        .iter()
        .map(|s| {
            format!(
                "if test -f {0}; then printf '%s\\n' {0}; fi\n",
                transport::quote(s)
            )
        })
        .collect::<String>();
    let script = format!(
        "set -eu\ncd {}\n(\n{list}) | tar -cf {} -T -\n",
        transport::quote(remote.trim_end_matches('/')),
        transport::quote(&remote_archive)
    );
    transport::ssh(config, base, &script, 120)?;
    let archive = job.join("artifacts.tar");
    transport::download(config, base, &remote_archive, &archive)?;
    let local = base.join("mirror/mac");
    fs::create_dir_all(&local)?;
    let mut cmd = Command::new("tar");
    cmd.arg("-xf").arg(&archive).arg("-C").arg(&local);
    super::super::build::background(&mut cmd);
    ensure!(
        crate::process::run(&mut cmd, None)?.success(),
        "cannot unpack Mac evidence"
    );
    fn rewrite(v: &mut Value, remote: &str, local: &Path) {
        match v {
            Value::String(s) => {
                if let Some(p) = s.strip_prefix(remote)
                    && ["captures/", "measurements/", "sessions/"]
                        .iter()
                        .any(|root| p.starts_with(root))
                {
                    *s = local.join(p).to_string_lossy().into_owned();
                }
            }
            Value::Array(a) => {
                for v in a {
                    rewrite(v, remote, local);
                }
            }
            Value::Object(o) => {
                for v in o.values_mut() {
                    rewrite(v, remote, local);
                }
            }
            _ => {}
        }
    }
    rewrite(value, &remote, &local);
    Ok(())
}

#[cfg(test)]
mod tests;
