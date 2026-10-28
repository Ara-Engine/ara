//! Packaging gate: every file a template's documents refer to exists inside it.
//!
//! Checked: `@path#id` strings in text documents, glTF buffer/image URIs, the
//! source beside each `*.import.json` sidecar, a look's `scene` and the project's
//! `main`. A reference that leaves the template counts as missing.
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{collections::BTreeSet, fs, path::Path};

/// Text documents scanned for references (all JSON).
const DOCUMENTS: &[&str] = &[
    ".scene",
    ".prefab",
    ".material",
    ".graph",
    ".look.json",
    ".animation",
    ".terrain",
    ".foliage",
    ".music",
    ".gltf",
    ".import.json",
];

/// Fails, naming each `document -> missing path`, when a template document refers
/// to a file that is absent or outside the template.
pub fn check(template: &Path) -> Result<()> {
    let mut missing = BTreeSet::new();
    let authored = template.join(ara_doc::AUTHORING_ROOT);
    for file in crate::support::files(template)? {
        let document = if file == template.join("ara.project.json") {
            "ara.project.json".to_owned()
        } else if let Ok(path) = file.strip_prefix(&authored) {
            path.to_string_lossy().replace('\\', "/")
        } else {
            continue;
        };
        if document != "ara.project.json" && !DOCUMENTS.iter().any(|ext| document.ends_with(ext)) {
            continue;
        }
        let value: Value = serde_json::from_slice(&fs::read(&file)?)
            .with_context(|| format!("template document {document} is not JSON"))?;
        for target in references(&document, &value) {
            let resolved = normalize(&target);
            if !resolved
                .as_ref()
                .is_some_and(|path| authored.join(path).is_file())
            {
                missing.insert(format!("{document} -> {}", resolved.unwrap_or(target)));
            }
        }
    }
    ensure!(
        missing.is_empty(),
        "template {} has {} missing referenced file(s); generate them (ara-samples kits) or fetch the sample assets:\n  {}",
        template.display(),
        missing.len(),
        missing.into_iter().collect::<Vec<_>>().join("\n  ")
    );
    Ok(())
}

/// Authored-relative targets referenced by a logical document or the root marker.
fn references(document: &str, value: &Value) -> Vec<String> {
    let mut out = Vec::new();
    strings(value, &mut |text| {
        if let Some(reference) = text.strip_prefix('@') {
            let (path, id) = reference.split_once('#').unwrap_or((reference, ""));
            if !path.is_empty() && (!id.is_empty() || path.contains('.')) {
                out.push(path.to_owned());
            }
        }
    });
    let directory = document
        .rsplit_once('/')
        .map_or("", |(directory, _)| directory);
    let field = |name: &str| value[name].as_str().map(str::to_owned);
    if document.ends_with(".gltf") {
        for list in ["buffers", "images"] {
            for uri in value[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|item| item["uri"].as_str())
                .filter(|uri| !uri.starts_with("data:"))
            {
                out.push(join(directory, &percent_decode(uri)));
            }
        }
    } else if let Some(source) = document.strip_suffix(".import.json") {
        out.push(source.to_owned());
    } else if document.ends_with(".look.json") {
        out.extend(field("scene"));
    } else if document == "ara.project.json" {
        out.extend(field("main"));
    }
    out
}

fn strings(value: &Value, visit: &mut impl FnMut(&str)) {
    match value {
        Value::String(text) => visit(text),
        Value::Array(items) => items.iter().for_each(|item| strings(item, visit)),
        Value::Object(map) => map.values().for_each(|item| strings(item, visit)),
        _ => {}
    }
}

fn join(directory: &str, path: &str) -> String {
    if directory.is_empty() {
        path.to_owned()
    } else {
        format!("{directory}/{path}")
    }
}

/// Lexically resolves `.`/`..`; `None` for absolute paths or paths leaving the root.
fn normalize(path: &str) -> Option<String> {
    if path.starts_with(['/', '\\']) || path.contains(':') {
        return None;
    }
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn template(files: &[(&str, &str)]) -> tempfile::TempDir {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join(ara_doc::AUTHORING_ROOT)).unwrap();
        crate::support::json(
            &root.path().join("ara.project.json"),
            &serde_json::json!({"layoutVersion":ara_doc::PROJECT_LAYOUT_VERSION,"main":"levels/a.scene"}),
        ).unwrap();
        for (path, text) in files {
            let path = root.path().join(ara_doc::AUTHORING_ROOT).join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, text).unwrap();
        }
        root
    }

    #[test]
    fn missing_generated_kits_and_textures_are_named() {
        let root = template(&[
            (
                "levels/a.scene",
                r#"{"records":{"e":{"mesh":"@assets/kit/hut.gltf#as_kithut","name":"@ not a path"}}}"#,
            ),
            ("assets/kit/hut.gltf.import.json", r#"{"id":"as_kithut"}"#),
            ("assets/kit/road.gltf.import.json", r#"{"id":"as_kitroad"}"#),
            (
                "assets/kit/road.gltf",
                r#"{"buffers":[{"uri":"road.bin"}],"images":[{"uri":"../third_party/trail/trail%20color.jpg"},{"uri":"data:image/png;base64,AA"}]}"#,
            ),
            ("look/a.look.json", r#"{"scene":"levels/gone.scene"}"#),
            (
                "prefabs/escape.prefab",
                r#"{"source":"@../outside.gltf#x"}"#,
            ),
        ]);
        let error = format!("{:#}", check(root.path()).unwrap_err());
        for missing in [
            "levels/a.scene -> assets/kit/hut.gltf",
            "assets/kit/hut.gltf.import.json -> assets/kit/hut.gltf",
            "assets/kit/road.gltf -> assets/kit/road.bin",
            "assets/kit/road.gltf -> assets/third_party/trail/trail color.jpg",
            "look/a.look.json -> levels/gone.scene",
            "prefabs/escape.prefab -> ../outside.gltf",
        ] {
            assert!(error.contains(missing), "{missing} not named in:\n{error}");
        }
        assert!(!error.contains("not a path"), "{error}");
        assert!(!error.contains("data:"), "{error}");
        assert!(!error.contains("ara.project.json"), "{error}");
    }

    #[test]
    fn project_root_decoys_cannot_satisfy_authored_references() {
        let root = template(&[]);
        fs::create_dir_all(root.path().join("levels")).unwrap();
        fs::write(root.path().join("levels/a.scene"), "{}").unwrap();
        let error = format!("{:#}", check(root.path()).unwrap_err());
        assert!(
            error.contains("ara.project.json -> levels/a.scene"),
            "{error}"
        );
    }

    #[test]
    fn a_complete_template_passes() {
        let root = template(&[
            (
                "levels/a.scene",
                r#"{"mesh":"@assets/kit/road.gltf#as_kitroad"}"#,
            ),
            ("assets/kit/road.gltf.import.json", "{}"),
            (
                "assets/kit/road.gltf",
                r#"{"buffers":[{"uri":"road.bin"}],"images":[{"uri":"../third_party/t/c.jpg"}]}"#,
            ),
            ("assets/kit/road.bin", ""),
            ("assets/third_party/t/c.jpg", ""),
        ]);
        check(root.path()).unwrap();
    }
}
