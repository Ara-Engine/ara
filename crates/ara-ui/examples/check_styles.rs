//! Validate native stylesheets the way debug editors and the Hub load them: the files
//! are concatenated in order, checked against the editor rules (tokens only, known
//! properties) and resolved against Ara Dark and Ara Light.
//!
//! `cargo run -p ara-ui --example check_styles -- crates/ara-hub/src/view/hub.css crates/ara-ui/src/select/select.css`
use ara_ui::theme::{EditorTheme, check_editor, tokens_from_json};

fn main() -> std::process::ExitCode {
    let mut css = String::new();
    for path in std::env::args().skip(1) {
        match std::fs::read_to_string(&path) {
            Ok(source) => {
                if let Err(error) = check_editor("<ui id='check' />", &source) {
                    eprintln!("{path}: {error}");
                    return std::process::ExitCode::FAILURE;
                }
                css.push_str(&source);
            }
            Err(error) => {
                eprintln!("{path}: {error}");
                return std::process::ExitCode::FAILURE;
            }
        }
    }
    // Live theme files when run from a checkout, else the embedded ones.
    for (name, theme) in [
        ("ara-dark", EditorTheme::Dark),
        ("ara-light", EditorTheme::Light),
    ] {
        let source = std::fs::read_to_string(format!("assets/themes/{name}.json"))
            .unwrap_or_else(|_| theme.json().to_owned());
        let result = tokens_from_json(&source)
            .and_then(|tokens| ara_ui::style::Sheet::with_tokens(&css, tokens).map(drop));
        if let Err(error) = result {
            eprintln!("{name}: {error}");
            return std::process::ExitCode::FAILURE;
        }
    }
    println!("styles valid against Ara Dark and Ara Light");
    std::process::ExitCode::SUCCESS
}
