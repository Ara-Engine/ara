//! Every diagnostic code in the workspace has a catalog entry, and no two crates
//! define the same number (except the known collisions below).
//!
//! The sources are scanned rather than linked, so the check covers crates `ara-diag`
//! cannot depend on and the C# sources: `Code(<n>)` literals, `NAME = <n>,` lines of
//! `codes!` tables, `ARA-EXP-*` and `ARA_GPU_*` names in Rust, and `"ARA0nnn"` in C#.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use ara_diag::catalog;

/// Numbers two crates define with different meanings, documented in one entry each.
/// Never add to this list: give the newer code a free number instead.
const KNOWN_COLLISIONS: &[u16] = &[3600, 3601];

/// Export failures without a specific code use `ARA-EXP-<platform>-100`.
const DYNAMIC: &[&str] = &[
    "ARA-EXP-WIN-100",
    "ARA-EXP-LNX-100",
    "ARA-EXP-AND-100",
    "ARA-EXP-APL-100",
];

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn files(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let path = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if !matches!(name.as_str(), "target" | "bin" | "obj" | "node_modules")
                && !name.starts_with('.')
            {
                files(&path, ext, out);
            }
        } else if name.ends_with(ext) {
            out.push(path);
        }
    }
}

/// Every `<prefix><digits>` with exactly `width` digits (or any width when 0).
fn numbers_after<'a>(
    text: &'a str,
    prefix: &'a str,
) -> impl Iterator<Item = (usize, &'a str)> + 'a {
    text.match_indices(prefix).filter_map(move |(i, _)| {
        let rest = &text[i + prefix.len()..];
        let digits: &str = &rest[..rest
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(rest.len())];
        (!digits.is_empty()).then_some((i, digits))
    })
}

/// `ARA-EXP-AREA-NNN` and `ARA_GPU_NAME` tokens.
fn names(text: &str, prefix: &str, out: &mut BTreeSet<String>) {
    for (i, _) in text.match_indices(prefix) {
        let token: String = text[i..]
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '-' || *c == '_')
            .collect();
        let token = token.trim_end_matches(['-', '_']).to_owned();
        let complete = if prefix == "ARA-EXP-" {
            token
                .rsplit('-')
                .next()
                .is_some_and(|n| n.len() == 3 && n.chars().all(|c| c.is_ascii_digit()))
        } else {
            token.len() > prefix.len()
        };
        if complete {
            out.insert(token);
        }
    }
}

#[test]
fn every_code_in_the_sources_is_documented_and_numbers_do_not_collide() {
    assert!(catalog::problems().is_empty(), "{:#?}", catalog::problems());
    let root = workspace();
    let mut rust = Vec::new();
    files(&root.join("crates"), ".rs", &mut rust);
    let mut csharp = Vec::new();
    files(&root.join("dotnet/src"), ".cs", &mut csharp);
    assert!(rust.len() > 100, "the scan found the crates");

    let mut numbers: BTreeMap<u16, BTreeSet<String>> = BTreeMap::new();
    let mut named: BTreeSet<String> = DYNAMIC.iter().map(|s| (*s).to_owned()).collect();
    for path in &rust {
        let text = fs::read_to_string(path).unwrap_or_default();
        let rel = path
            .strip_prefix(&root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        let krate = rel.split('/').nth(1).unwrap_or_default().to_owned();
        let tests = rel.contains("/tests/") || rel.contains("/benches/");
        let mut add = |n: &str| {
            if let Ok(n) = n.parse::<u16>() {
                numbers.entry(n).or_default().insert(krate.clone());
            }
        };
        for (_, n) in numbers_after(&text, "Code(") {
            if !tests {
                add(n);
            }
        }
        if rel.ends_with("/codes.rs") {
            for line in text.lines() {
                let line = line.trim();
                if let Some((name, rest)) = line.split_once(" = ")
                    && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
                    && let Some(n) = rest.strip_suffix(',')
                {
                    add(n);
                }
            }
        }
        if !tests {
            names(&text, "ARA-EXP-", &mut named);
            names(&text, "ARA_GPU_", &mut named);
        }
    }
    for path in &csharp {
        let text = fs::read_to_string(path).unwrap_or_default();
        for (i, n) in numbers_after(&text, "\"ARA") {
            if n.len() == 4 && text[i + 5 + 4..].starts_with('"') {
                numbers
                    .entry(n.parse().unwrap_or(0))
                    .or_default()
                    .insert("dotnet".into());
            }
        }
    }

    let mut missing = Vec::new();
    for (n, crates) in &numbers {
        if catalog::get(ara_diag::Code(*n)).is_none() {
            missing.push(format!(
                "ARA{n:04} (defined in {})",
                crates.iter().cloned().collect::<Vec<_>>().join(", ")
            ));
        }
    }
    for name in &named {
        if catalog::lookup(name).is_none() {
            missing.push(name.clone());
        }
    }
    assert!(
        missing.is_empty(),
        "codes without a catalog entry in crates/ara-diag/codes/:\n{}",
        missing.join("\n")
    );

    // `ara-diag` defines the kernel and bus constants and `ara-why` refers to other
    // crates' numbers; neither counts as a second owner. C# and `ara-script` share
    // the generated 0900-range table.
    let owners = |crates: &BTreeSet<String>| -> BTreeSet<String> {
        crates
            .iter()
            .filter(|c| !matches!(c.as_str(), "ara-diag" | "ara-why"))
            .map(|c| {
                if c == "dotnet" {
                    "ara-script".to_owned()
                } else {
                    c.clone()
                }
            })
            .collect()
    };
    let collisions: Vec<String> = numbers
        .iter()
        .filter(|(n, crates)| owners(crates).len() > 1 && !KNOWN_COLLISIONS.contains(n))
        .map(|(n, crates)| {
            format!(
                "ARA{n:04}: {}",
                owners(crates).into_iter().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    assert!(
        collisions.is_empty(),
        "several crates define the same code:\n{}",
        collisions.join("\n")
    );
}
