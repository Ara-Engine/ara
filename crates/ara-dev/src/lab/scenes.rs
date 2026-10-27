//! Frozen scene selection; no importing or export ever mutates tracked sample files.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Selection {
    pub scene: String,
    pub pack: PathBuf,
    pub preset: String,
    pub size: [u32; 2],
    pub camera: Option<Value>,
    #[serde(default)]
    pub params: Value,
    #[serde(default = "baseline")]
    pub variant: String,
}
fn baseline() -> String {
    "baseline".into()
}

pub(super) fn select(
    root: &Path,
    base: &Path,
    args: &Value,
    binaries: &crate::binaries::Binaries,
) -> Result<Selection> {
    let scene = args["scene"].as_str().unwrap_or("showcase");
    ensure!(
        ["showcase", "outpost", "hangar"].contains(&scene),
        "unknown scene"
    );
    let preset = args["preset"].as_str().unwrap_or("high");
    ensure!(
        ["low", "medium", "high", "epic"].contains(&preset),
        "unknown preset"
    );
    let size = [
        args["width"].as_u64().unwrap_or(2560),
        args["height"].as_u64().unwrap_or(1440),
    ];
    ensure!(
        size.iter().all(|s| (16..=8192).contains(s)),
        "resolution must be 16..8192"
    );
    let pack = if let Some(path) = args["pack"].as_str() {
        PathBuf::from(path).canonicalize()?
    } else {
        let mut candidates = vec![
            base.join("packs").join(scene),
            root.join("target/ara-dev/perf").join(scene),
            PathBuf::from(format!("D:/AraBench/builds/ara/{scene}/build")),
        ];
        for checkout in std::iter::once(root.to_owned()).chain(common_checkout(root)) {
            candidates.push(checkout.join("target/ara-dev/perf").join(scene));
            let folder = checkout.join("target/d26/packs");
            if let Ok(entries) = fs::read_dir(folder) {
                let mut packs = entries
                    .filter_map(|entry| entry.ok())
                    .map(|entry| entry.path())
                    .filter(|path| {
                        path.file_name()
                            .is_some_and(|f| f.to_string_lossy().starts_with(&format!("{scene}-")))
                    })
                    .collect::<Vec<_>>();
                packs.sort();
                packs.reverse();
                candidates.extend(packs);
            }
        }
        if let Some(pack) = candidates
            .into_iter()
            .find(|path| path.join("ara-launch.json").is_file())
        {
            pack
        } else {
            freeze(root, base, scene, binaries)?
        }
    };
    ensure!(
        pack.join("ara-launch.json").is_file(),
        "{} is not an exported pack",
        pack.display()
    );
    let camera = shots(root, scene)?
        .first()
        .map(|shot| shot["camera"].clone());
    Ok(Selection {
        scene: scene.into(),
        pack,
        preset: preset.into(),
        size: size.map(|s| s as u32),
        camera,
        params: json!({}),
        variant: baseline(),
    })
}

/// A linked worktree may reuse immutable D26 packs from its common checkout.
fn common_checkout(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return None;
    }
    let file = fs::read_to_string(git).ok()?;
    let path = PathBuf::from(file.trim().strip_prefix("gitdir:")?.trim());
    let administrative = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    let common = fs::read_to_string(administrative.join("commondir")).ok()?;
    let common = administrative.join(common.trim()).canonicalize().ok()?;
    let checkout = common.parent()?;
    checkout
        .join("Cargo.toml")
        .is_file()
        .then(|| checkout.to_owned())
}

fn freeze(
    root: &Path,
    base: &Path,
    scene: &str,
    binaries: &crate::binaries::Binaries,
) -> Result<PathBuf> {
    let sample = if scene == "showcase" {
        "showcase"
    } else {
        "outpost"
    };
    crate::perf::freeze::preflight(root, sample)?;
    let staging = base.join("staging").join(super::session::id());
    super::watch::copy_tree(&root.join("samples").join(sample), &staging)?;
    let path = staging.join("ara.project.json");
    let mut manifest: Value = serde_json::from_slice(&fs::read(&path)?)?;
    manifest["main"] = json!(format!("levels/{scene}.scene"));
    fs::write(path, serde_json::to_vec_pretty(&manifest)?)?;
    crate::perf::freeze::pack(
        root,
        base,
        &base.join("packs").join(scene),
        &staging,
        binaries,
    )
}

pub(super) fn shots(root: &Path, scene: &str) -> Result<Vec<Value>> {
    let sample = if scene == "showcase" {
        "showcase"
    } else {
        "outpost"
    };
    let look: Value = serde_json::from_slice(&fs::read(
        root.join(format!("samples/{sample}/look/{scene}.look.json")),
    )?)?;
    let shots = look["shots"].as_array().context("look shots")?;
    shots.iter().filter(|shot| scene != "showcase" || ["stage", "materials", "props"].contains(&shot["name"].as_str().unwrap_or("")))
        .map(|shot| {
            let eye: [f32;3] = serde_json::from_value(shot["position"].clone())?;
            let target: [f32;3] = serde_json::from_value(shot["target"].clone())?;
            let direction = std::array::from_fn::<_,3,_>(|i| target[i]-eye[i]);
            let length = direction.iter().map(|v| v*v).sum::<f32>().sqrt();
            ensure!(length > 0., "camera direction is empty");
            Ok(json!({"name":shot["name"],"camera":{"position":eye,"forward":direction.map(|v| v/length),"fov":shot["fov"]}}))
        }).collect()
}
