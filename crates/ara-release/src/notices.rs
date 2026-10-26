//! Inventory all metadata packages and retain their original license/notice texts.
use crate::support::{self, copy, files};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path, process::Command};

fn notice(path: &Path) -> bool {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    [
        "license",
        "licence",
        "notice",
        "copying",
        "copyright",
        "ofl",
    ]
    .iter()
    .any(|prefix| name.contains(prefix))
        && !name.ends_with(".rs")
        && !name.ends_with(".c")
        && !name.ends_with(".h")
}
pub fn generate(root: &Path, stage: &Path, dist: &Path) -> Result<()> {
    let output = Command::new("cargo")
        .current_dir(root)
        .args(["metadata", "--locked", "--format-version", "1"])
        .output()?;
    ensure!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::write(dist.join("logs/cargo-metadata.json"), &output.stdout)?;
    let metadata: Value = serde_json::from_slice(&output.stdout)?;
    let mut inventory = Vec::new();
    let mut sections = BTreeMap::new();
    for package in metadata["packages"]
        .as_array()
        .context("metadata packages missing")?
    {
        let name = package["name"].as_str().context("package name")?;
        let version = package["version"].as_str().context("package version")?;
        let manifest = Path::new(
            package["manifest_path"]
                .as_str()
                .context("package manifest")?,
        );
        let directory = manifest.parent().context("package directory")?;
        if package["source"].is_string()
            && let Some(expression) = package["license"].as_str()
        {
            for id in expression
                .split(|c: char| !c.is_ascii_alphanumeric() && !".-+".contains(c))
                .filter(|id| !id.is_empty() && !["AND", "OR", "WITH"].contains(id))
            {
                ensure!(
                    root.join("release/licenses/spdx")
                        .join(format!("{id}.txt"))
                        .is_file(),
                    "missing standard license text {id} for {name} {version}; update release/licenses/spdx"
                );
            }
        }
        let mut texts = Vec::new();
        // Workspace vendors are handled once below, rather than repeated per crate.
        let candidates = if package["source"].is_null() {
            fs::read_dir(directory)?
                .map(|entry| Ok(entry?.path()))
                .collect::<Result<Vec<_>>>()?
        } else {
            files(directory)?
        };
        for file in candidates
            .into_iter()
            .filter(|path| path.is_file() && notice(path))
        {
            let relative = file.strip_prefix(directory)?;
            let target = stage
                .join("licenses/crates")
                .join(format!("{name}-{version}"))
                .join(relative);
            copy(&file, &target)?;
            texts.push(format!(
                "{}\n{}",
                relative.display(),
                String::from_utf8_lossy(&fs::read(&file)?)
            ));
        }
        if let Some(file) = package["license_file"].as_str() {
            let file = directory.join(file);
            if file.is_file()
                && !texts.iter().any(|text| {
                    text.starts_with(
                        file.file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .as_ref(),
                    )
                })
            {
                copy(
                    &file,
                    &stage
                        .join("licenses/crates")
                        .join(format!("{name}-{version}"))
                        .join(file.file_name().context("license filename")?),
                )?;
                texts.push(String::from_utf8_lossy(&fs::read(&file)?).into_owned());
            }
        }
        // meshoptimizer's original MIT notice is embedded in its public header.
        if name == "meshopt" {
            let header = fs::read_to_string(directory.join("vendor/src/meshoptimizer.h"))?;
            let start = header.rfind("/*").context("meshoptimizer license block")?;
            texts.push(format!(
                "meshoptimizer original header notice\n{}",
                &header[start..]
            ));
        }
        let license = package["license"]
            .as_str()
            .unwrap_or(if package["source"].is_null() {
                "Ara: all rights reserved"
            } else {
                "See original license files"
            });
        inventory.push(json!({"name":name,"version":version,"license":license,"repository":package["repository"],"source":package["source"],"originalLicenseFiles":texts.len()}));
        let source = match package["source"].as_str() {
            None => "Workspace or vendored source in https://github.com/Snowy7/ara".into(),
            Some(source) if source.starts_with("git+") => source.to_owned(),
            Some(_) => format!("https://crates.io/api/v1/crates/{name}/{version}/download"),
        };
        sections.insert(
            format!("{name} {version}"),
            format!(
                "SPDX: {license}\nAuthors (Cargo metadata): {}\nRepository: {}\nSource: {source}\n{}",
                package["authors"],
                package["repository"],
                texts.join("\n\n")
            ),
        );
    }
    for file in files(&root.join("crates"))?
        .into_iter()
        .filter(|path| notice(path))
    {
        let relative = file.strip_prefix(root)?;
        copy(&file, &stage.join("licenses/vendored").join(relative))?;
        sections.insert(
            relative.to_string_lossy().into_owned(),
            String::from_utf8_lossy(&fs::read(&file)?).into_owned(),
        );
    }
    // Recast carries its original zlib license in source headers, not a LICENSE file.
    let recast = root.join("crates/ara-nav-sys/vendor/Recast/Source/Recast.cpp");
    let header = fs::read_to_string(recast)?
        .lines()
        .take_while(|line| line.starts_with("//") || line.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    sections.insert("Recast / Detour (original source notice)".into(), header);
    for file in files(&root.join("release/licenses"))? {
        copy(
            &file,
            &stage
                .join("licenses/additional")
                .join(file.strip_prefix(root.join("release/licenses"))?),
        )?;
        sections.insert(
            format!(
                "Additional original licenses / {}",
                file.strip_prefix(root.join("release/licenses"))?.display()
            ),
            String::from_utf8_lossy(&fs::read(&file)?).into_owned(),
        );
    }
    managed_notices(root, stage, &mut sections)?;
    let mut text = String::from(
        "ARA THIRD-PARTY NOTICES\n\nGenerated from cargo metadata --locked, every crate's license files, vendored libraries, NuGet packages and shader tool notices.\nThis inventory includes build/dev/target-specific dependencies for completeness; listing does not imply every package is linked.\nAra's own code is all rights reserved. Third-party licenses remain in force.\n\n",
    );
    for (title, body) in sections {
        text.push_str(&format!("\n===== {title} =====\n\n{body}\n"));
    }
    fs::write(stage.join("THIRD_PARTY_NOTICES.txt"), text)?;
    support::json(
        &stage.join("licenses/crates.json"),
        &json!({"packages":inventory}),
    )?;
    Ok(())
}

fn managed_notices(
    root: &Path,
    stage: &Path,
    sections: &mut BTreeMap<String, String>,
) -> Result<()> {
    // Hub ships no managed assemblies and needs no prior engine NuGet restore.
    if !stage.join("dotnet").is_dir() {
        return Ok(());
    }
    for project in ["Ara.CompileServer", "Ara.RunWorker", "Ara.Host"] {
        let path = root.join(format!(
            "dotnet/artifacts/obj/{project}/project.assets.json"
        ));
        let assets: Value = serde_json::from_slice(&fs::read(&path).with_context(|| {
            format!(
                "restore/publish managed payload before inventorying NuGet notices: {}",
                path.display()
            )
        })?)?;
        for (id, library) in assets["libraries"]
            .as_object()
            .context("NuGet libraries missing")?
        {
            if library["type"] != "package" {
                continue;
            }
            let relative = library["path"].as_str().context("NuGet package path")?;
            let package = assets["packageFolders"]
                .as_object()
                .context("NuGet package folders")?
                .keys()
                .map(|folder| Path::new(folder).join(relative))
                .find(|dir| dir.is_dir())
                .context("NuGet package directory missing")?;
            let mut text = String::new();
            for file in files(&package)?
                .into_iter()
                .filter(|file| notice(file) || file.extension().is_some_and(|ext| ext == "nuspec"))
            {
                let rel = file.strip_prefix(&package)?;
                copy(&file, &stage.join("licenses/nuget").join(id).join(rel))?;
                text.push_str(&format!(
                    "\n{}\n{}\n",
                    rel.display(),
                    String::from_utf8_lossy(&fs::read(&file)?)
                ));
            }
            ensure!(!text.is_empty(), "no NuGet licensing metadata for {id}");
            sections.insert(format!("NuGet {id}"), text);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn standalone_notices_need_no_restore_but_managed_payload_still_requires_licenses() {
        let target =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/release-notice-tests");
        fs::create_dir_all(&target).expect("target");
        let root = tempfile::tempdir_in(target).expect("isolated checkout");
        let stage = root.path().join("stage");
        fs::create_dir(&stage).expect("stage");
        let mut sections = BTreeMap::new();
        managed_notices(root.path(), &stage, &mut sections)
            .expect("Hub without restored artifacts");
        fs::create_dir(stage.join("dotnet")).expect("managed payload");
        let error = managed_notices(root.path(), &stage, &mut sections)
            .expect_err("managed licenses cannot be silently omitted");
        assert!(format!("{error:#}").contains("restore/publish managed payload"));
    }
}
