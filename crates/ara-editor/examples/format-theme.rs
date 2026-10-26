//! Canonicalize edited theme resources: cargo run -p ara-editor --example
//! format-theme -- assets/themes/ara-dark.json assets/themes/ara-light.json
fn main() -> ara_ui::Result<()> {
    for file in std::env::args_os().skip(1) {
        let path = std::path::PathBuf::from(file);
        let source = std::fs::read_to_string(&path)?;
        let value = ara_json::parse(&source)?;
        ara_ui::theme::tokens_from_json(&source)?;
        std::fs::write(path, ara_json::to_canonical_string(&value))?;
    }
    Ok(())
}
