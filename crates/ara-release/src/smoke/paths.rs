//! Where the smoke installs, and the check that what it installs fits Windows' path limit.
//!
//! The scratch folder is short and of fixed length wherever the checkout is and whatever
//! the process id is, so a package that fits once fits every run; `ensure_fits` refuses to
//! install a package that cannot, before anything is installed.
use anyhow::{Result, bail};
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

/// Windows cannot create, open or delete a file whose path has this many characters or
/// more (`MAX_PATH` counts the terminating NUL), so an install that reaches it cannot be
/// uninstalled.
const PATH_LIMIT: usize = 260;

/// The folder that holds every smoke scratch folder: `smoke` beside the artifacts folder,
/// so inside the target directory for the default `target/release-dist`.
pub(super) fn root(dist: &Path) -> PathBuf {
    dist.parent().unwrap_or(dist).join("smoke")
}

/// Creates a fresh scratch folder `<root>/<4 hex digits>`. The name never varies in length.
pub(super) fn scratch(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let seed = std::process::id()
        ^ std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .subsec_nanos();
    for attempt in 0..=0xffff_u32 {
        let directory = root.join(format!("{:04x}", seed.wrapping_add(attempt) & 0xffff));
        match fs::create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    bail!(
        "every smoke folder name under {} is taken; delete the old runs",
        root.display()
    )
}

fn length(path: &Path) -> usize {
    path.to_string_lossy().chars().count()
}

/// The longest path (and its length in characters) of `files` installed under `install`.
fn longest_path(
    install: &Path,
    files: impl IntoIterator<Item = PathBuf>,
) -> Option<(PathBuf, usize)> {
    files
        .into_iter()
        .map(|file| install.join(file))
        .map(|path| (length(&path), path))
        .max_by_key(|(length, _)| *length)
        .map(|(length, path)| (path, length))
}

/// Every file under `package`, relative to it.
fn package_files(package: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut pending = vec![package.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path.strip_prefix(package)?.to_path_buf());
            }
        }
    }
    Ok(files)
}

/// Fails, naming the path and its length, if installing `package` (an unpacked package
/// folder) under `install` would create a path of `PATH_LIMIT` characters or more. A
/// package folder that is not there is skipped: the smoke then fails at its own install.
pub(super) fn ensure_fits(install: &Path, package: &Path) -> Result<()> {
    if !package.is_dir() {
        return Ok(());
    }
    if let Some((path, length)) = longest_path(install, package_files(package)?)
        && length >= PATH_LIMIT
    {
        bail!(
            "the smoke install would create a path of {length} characters, but Windows allows \
             {}: {}\nRun it from a shorter artifacts folder or checkout path ({} over).",
            PATH_LIMIT - 1,
            path.display(),
            length + 1 - PATH_LIMIT
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file whose installed path has exactly `length` characters under `C:\i`.
    fn file_of(length: usize) -> PathBuf {
        PathBuf::from("a".repeat(length - r"C:\i\".len()))
    }

    #[test]
    fn the_longest_installed_path_is_reported_with_its_length() {
        let install = Path::new(r"C:\i");
        let files = [file_of(40), file_of(120), file_of(80)];
        let (path, length) = longest_path(install, files).expect("a file");
        assert_eq!(length, 120);
        assert_eq!(super::length(&path), 120);
        assert!(longest_path(install, Vec::new()).is_none());
    }

    #[test]
    fn a_path_of_259_characters_fits_and_260_does_not() {
        let package = tempfile::tempdir().expect("package");
        fs::write(package.path().join(file_of(259)), b"x").expect("file");
        ensure_fits(Path::new(r"C:\i"), package.path()).expect("259 characters fit");
        fs::write(package.path().join(file_of(260)), b"x").expect("file");
        let error = ensure_fits(Path::new(r"C:\i"), package.path())
            .expect_err("260 characters do not")
            .to_string();
        assert!(error.contains("260 characters"), "{error}");
        assert!(error.contains("(1 over)"), "{error}");
    }

    #[test]
    fn nested_package_folders_count_toward_the_length() {
        let package = tempfile::tempdir().expect("package");
        // Neither name is long enough to be refused by the file system; together they are.
        let deep = package.path().join("resources").join("d".repeat(130));
        fs::create_dir_all(&deep).expect("dirs");
        fs::write(deep.join("f".repeat(130)), b"x").expect("file");
        let error = ensure_fits(Path::new(r"C:\i"), package.path()).expect_err("too long");
        assert!(error.to_string().contains("resources"), "{error}");
        // No package folder: nothing to check.
        ensure_fits(Path::new(r"C:\i"), &package.path().join("missing")).expect("skipped");
    }

    #[test]
    fn scratch_folders_have_fixed_short_names() {
        let folder = tempfile::tempdir().expect("root");
        let first = scratch(folder.path()).expect("first");
        let second = scratch(folder.path()).expect("second");
        assert_ne!(first, second);
        for directory in [&first, &second] {
            let name = directory.file_name().expect("name").to_string_lossy();
            assert_eq!(name.len(), 4, "{name}");
            assert!(name.chars().all(|c| c.is_ascii_hexdigit()), "{name}");
        }
        assert_eq!(root_of(&first), folder.path());
        let dist = Path::new("x").join("target").join("release-dist");
        assert_eq!(root(&dist), Path::new("x").join("target").join("smoke"));
    }

    fn root_of(scratch: &Path) -> &Path {
        scratch.parent().expect("root")
    }
}
