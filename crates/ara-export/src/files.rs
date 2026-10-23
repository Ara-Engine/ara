//! Export assembly file operations.
use crate::tools;
use anyhow::Result;
use std::{collections::BTreeMap, path::Path};
pub(crate) fn copy_scripts(from: &Path, to: &Path, assembly: &str) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let e = entry?;
        let path = e.path();
        if e.file_type()?.is_dir() {
            copy_scripts(&path, &to.join(e.file_name()), assembly)?;
        } else if !path.extension().is_some_and(|e| e == "pdb" || e == "dbg")
            && e.file_name() != format!("{assembly}.exe").as_str()
            && e.file_name() != assembly
        {
            tools::copy(&path, &to.join(e.file_name()))?;
        }
    }
    Ok(())
}
pub(crate) fn copy_all(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let e = entry?;
        if e.file_type()?.is_dir() {
            copy_all(&e.path(), &to.join(e.file_name()))?;
        } else {
            tools::copy(&e.path(), &to.join(e.file_name()))?;
        }
    }
    Ok(())
}
pub(crate) fn collect_sizes(
    root: &Path,
    path: &Path,
    out: &mut BTreeMap<String, u64>,
) -> Result<()> {
    for entry in std::fs::read_dir(path)? {
        let e = entry?;
        if e.file_type()?.is_dir() {
            collect_sizes(root, &e.path(), out)?;
        } else if !e.path().extension().is_some_and(|x| x == "log") {
            out.insert(
                e.path()
                    .strip_prefix(root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
                e.metadata()?.len(),
            );
        }
    }
    Ok(())
}
