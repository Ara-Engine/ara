//! Changed immutable files travel in one archive; publication happens under the peer's visual lock.
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
fn inventory(source: &Path, prefix: &str, files: &mut BTreeMap<String, PathBuf>) -> Result<()> {
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
            files.insert(name, entry.path());
        }
    }
    Ok(())
}
fn shaders(source: &Path, prefix: &str, files: &mut BTreeMap<String, PathBuf>) -> Result<()> {
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
            files.insert(name, path);
            continue;
        }
        if path.extension().is_some_and(|e| e == "json")
            && let Ok(v) = serde_json::from_slice::<Value>(&fs::read(&path)?)
            && let Some(stages) = v["stages"].as_array()
        {
            files.insert(name, path);
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
                    ensure!(
                        src.is_file(),
                        "PC cooked artifact missing: {}",
                        src.display()
                    );
                    files.insert(format!("{prefix}/{file}"), src);
                }
            }
        }
    }
    Ok(())
}
fn stamp(path: &Path) -> Result<Value> {
    let meta = fs::metadata(path)?;
    Ok(json!([
        path,
        meta.len(),
        meta.modified()?
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
            .to_string()
    ]))
}
pub(super) fn push(
    root: &Path,
    base: &Path,
    config: &Config,
    activity: &super::super::track::Activity,
) -> Result<()> {
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
        .map(|(name, path)| Ok((name.clone(), stamp(path)?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
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
        return Ok(());
    }
    let job = transport::job(base)?;
    let data = job.join("data");
    fs::create_dir_all(&data)?;
    let mut names = Vec::new();
    for (name, source) in changed {
        let target = data.join(relative(name)?);
        fs::create_dir_all(target.parent().context("artifact parent")?)?;
        if fs::hard_link(source, &target).is_err() {
            fs::copy(source, &target)?;
        }
        names.push(name.as_str());
    }
    drop(_snapshot);
    // A claim/recipe-only publication still needs a valid, empty archive.
    if names.is_empty() {
        names.push(".");
    }
    fs::write(job.join("files.txt"), names.join("\n") + "\n")?;
    let archive = job.join("changed.tar");
    let mut tar = Command::new("tar");
    tar.arg("-cf")
        .arg(&archive)
        .arg("-C")
        .arg(&data)
        .arg("-T")
        .arg(job.join("files.txt"))
        .stdin(Stdio::null());
    super::super::build::background(&mut tar);
    ensure!(
        crate::process::run(&mut tar, Some(Instant::now() + Duration::from_secs(120)))?.success(),
        "cannot archive cooked changes"
    );
    let inbox = format!("mirror/inbox/{}", super::super::session::id());
    let remote = format!("{}/target/ara-dev/lab/{inbox}", config.repo);
    transport::ssh(
        config,
        base,
        &format!("set -eu\nmkdir -p {}\n", transport::quote(&remote)),
        20,
    )?;
    transport::scp(config, base, &archive, &format!("{remote}/changed.tar"))?;
    transport::ssh(
        config,
        base,
        &format!(
            "set -eu\nmkdir -p {0}/data\ntar -xf {0}/changed.tar -C {0}/data\n",
            transport::quote(&remote)
        ),
        120,
    )?;
    // First bootstrap can precede the endpoint; the peer imports this record at startup.
    let import = json!({"directory":inbox,"variants":portable(variants.clone()),"claims":portable(claims.clone()),"shaders_current":shaders_current});
    super::super::session::write(&base.join("mirror/incoming-local.json"), &import)?;
    let record = job.join("incoming.json");
    super::super::session::write(&record, &import)?;
    transport::scp(
        config,
        base,
        &record,
        &format!("{}/target/ara-dev/lab/mirror/incoming.json", config.repo),
    )?;
    let _ = transport::rpc(config, base, "_lab_import", import);
    let mut uploaded = serde_json::to_value(current)?;
    uploaded["_variants"] = variants;
    uploaded["_shaders_current"] = json!(shaders_current);
    uploaded["_claims"] = claims;
    uploaded["_configuration"] = configuration;
    super::super::session::write(&base.join("mirror/uploaded.json"), &uploaded)
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
mod tests {
    use super::*;
    #[test]
    fn shader_bundle_follows_active_metadata_and_requires_metal() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/lab-tests")
            .join(super::super::super::session::id());
        fs::create_dir_all(root.join("key"))?;
        for name in ["a.spv", "a.metallib", "a.metal.json", "orphan.metallib"] {
            fs::write(root.join("key").join(name), b"artifact")?;
        }
        let obsolete = root.join("f".repeat(64));
        fs::create_dir_all(&obsolete)?;
        fs::write(
            obsolete.join("reflection.json"),
            br#"{"stages":[{"spirv":"missing"}]}"#,
        )?;
        let manifest = json!({"stages":[{"spirv":"key/a.spv","metal":{"metallib":"key/a.metallib","reflection":"key/a.metal.json"}}]});
        super::super::super::session::write(&root.join("pipeline.json"), &manifest)?;
        let mut files = BTreeMap::new();
        shaders(&root, "shaders/cooked", &mut files)?;
        assert_eq!(files.len(), 4);
        assert!(!files.keys().any(|p| p.contains("orphan")));
        super::super::super::session::write(
            &root.join("pipeline.json"),
            &json!({"stages":[{"spirv":"key/a.spv"}]}),
        )?;
        assert!(shaders(&root, "shaders/cooked", &mut files).is_err());
        fs::remove_dir_all(root)?;
        Ok(())
    }
    #[test]
    fn artifact_paths_are_confined_and_windows_recipes_are_portable() {
        for p in ["/tmp/a", "../a", "a/../../b", "a\\b", "a\nb"] {
            assert!(relative(p).is_err());
        }
        assert!(relative("shaders/crates/a/cooked/a.json").is_ok());
        assert_eq!(
            portable(json!({"crates\\a\\pipelines.json":{"source":"x\\y.slang"}})),
            json!({"crates/a/pipelines.json":{"source":"x/y.slang"}})
        );
    }
}
