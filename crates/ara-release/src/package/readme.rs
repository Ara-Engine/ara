//! Release text uses the version being packaged, including its installer example.

pub(crate) fn render(version: &str) -> String {
    include_str!("../../../../release/README-FIRST.txt").replace("{{VERSION}}", version)
}

#[cfg(test)]
mod tests {
    #[test]
    fn installed_readme_uses_the_package_version_everywhere() {
        for version in ["0.0.2-alpha.4", "1.2.3-preview.8"] {
            let readme = super::render(version);
            assert!(readme.starts_with(&format!("ARA {version} -")), "{readme}");
            assert!(readme.contains(&format!("ara-{version}-windows-x64-setup.exe")));
            assert!(!readme.contains("0.0.1-alpha"));
        }
    }
}
