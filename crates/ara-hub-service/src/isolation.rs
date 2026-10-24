//! Development binaries must never connect to or write the owner's real Hub profile.
use std::{
    io,
    path::{Component, Path, PathBuf},
};

/// Reject the real user profile when called by a checkout build, including tests.
/// Explicit --config-dir overrides are checked too, before discovery or file creation.
pub fn validate_development_config(directory: &Path) -> io::Result<()> {
    if checkout_executable(&std::env::current_exe()?) && real_profile(directory)? {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "checkout/test Hub binaries cannot use the real Ara profile; set ARA_CONFIG_DIR to a scratch directory under target/",
        ));
    }
    Ok(())
}

pub(crate) fn validate_engine(directory: &Path, executable: &Path) -> io::Result<()> {
    if real_profile(directory)? && checkout_executable(executable) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "cannot register a worktree engine in the real Ara profile; set ARA_CONFIG_DIR to a scratch directory under target/",
        ));
    }
    Ok(())
}

fn checkout_executable(executable: &Path) -> bool {
    let Ok(executable) = resolved(executable) else {
        return true;
    };
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    if resolved(&workspace.join("target")).is_ok_and(|target| inside(&executable, &target)) {
        return true;
    }
    // Include custom Cargo target directories and other checkouts/worktrees.
    executable.ancestors().any(|ancestor| {
        ancestor.join(".cargo-lock").is_file()
            || (ancestor.file_name().is_some_and(|name| name == "target")
                && ancestor
                    .parent()
                    .is_some_and(|root| root.join("Cargo.toml").is_file()))
    })
}

fn real_profile(directory: &Path) -> io::Result<bool> {
    let directory = resolved(directory)?;
    let Some(profile) = crate::user_config_dir() else {
        return Ok(false);
    };
    Ok(inside(&directory, &resolved(&profile)?))
}

fn inside(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        let fold = |p: &Path| p.to_string_lossy().replace('\\', "/").to_lowercase();
        let path = fold(path);
        let root = fold(root);
        path == root
            || path
                .strip_prefix(&root)
                .is_some_and(|rest| rest.starts_with('/'))
    }
    #[cfg(not(windows))]
    {
        path.starts_with(root)
    }
}

// Resolve existing ancestors (including junctions) without creating the destination.
fn resolved(path: &Path) -> io::Result<PathBuf> {
    let mut absolute = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(
                    absolute.components().next_back(),
                    Some(Component::Normal(_))
                ) {
                    absolute.pop();
                }
            }
            _ => absolute.push(component.as_os_str()),
        }
    }
    let mut ancestor = absolute.as_path();
    let mut missing = Vec::new();
    loop {
        match ancestor.canonicalize() {
            Ok(mut result) => {
                for name in missing.iter().rev() {
                    result.push(name);
                }
                return Ok(result);
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.file_name().ok_or(error)?);
                ancestor = ancestor
                    .parent()
                    .ok_or_else(|| io::Error::other("config path has no existing ancestor"))?;
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkout_cannot_resolve_real_profile_or_register_a_worktree_build() {
        let Some(profile) = crate::user_config_dir() else {
            return;
        };
        let executable = std::env::current_exe().expect("test executable");
        assert!(checkout_executable(&executable));
        for path in [profile.clone(), profile.join("nested/../config")] {
            assert_eq!(
                validate_development_config(&path)
                    .expect_err("real profile must be blocked")
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
            assert_eq!(
                validate_engine(&path, &executable)
                    .expect_err("worktree registration must be blocked")
                    .kind(),
                io::ErrorKind::PermissionDenied
            );
        }
        // Packaged builds outside checkouts still support the real user's registry.
        validate_engine(
            &profile,
            &std::env::temp_dir().join("ara-installed/ara.exe"),
        )
        .expect("installed build remains allowed");
        validate_development_config(&crate::config_dir().expect("isolated test config"))
            .expect("scratch profile remains allowed");
    }
}
