//! The built-in explainers. Each module is one explainer (or, for `material`,
//! evidence shared by two), self-contained so its subsystem can take it over.

pub mod asset;
pub mod code;
pub mod frame;
mod material;
pub mod reload;
pub mod subject;
pub mod visibility;

use ara_diag::store::Record;
use ara_diag::{Fix, FixAction};
use serde_json::{Value, json};

use crate::Explainer;

/// Every built-in explainer; the fallback (`subject`) last.
pub(crate) fn builtin() -> Vec<Box<dyn Explainer>> {
    vec![
        Box::new(visibility::Visibility),
        Box::new(code::CodeExplainer),
        Box::new(reload::Reload),
        Box::new(frame::Frame),
        Box::new(asset::AssetExplainer),
        Box::new(subject::Subject),
    ]
}

/// A `property.set` fix, runnable through `ara_apply` / `ara do`.
pub(crate) fn set(title: impl Into<String>, entity: &str, path: &str, value: Value) -> Fix {
    Fix {
        title: title.into(),
        action: FixAction::Command {
            command: "property.set".into(),
            params: json!({ "entity": entity, "path": path, "value": value }),
        },
    }
}

/// A record as one line: `error ARA2101 ×3 `crate.glb` has no meshes (levels/a.scene · e_1)`.
pub(crate) fn line(r: &Record) -> String {
    let d = &r.diagnostic;
    let mut s = format!("{} {}", d.severity, d.code);
    if r.count > 1 {
        s.push_str(&format!(" ×{}", r.count));
    }
    s.push_str(&format!(" {}", d.message));
    if !d.location.is_empty() {
        s.push_str(&format!(" ({})", d.location));
    }
    if let Some(t) = r.tick {
        s.push_str(&format!(" @tick {t}"));
    }
    s
}

/// Re-addresses a record's fixes from document paths to `property.set` commands
/// when they are `Set` fixes on a record, so they run through `ara_apply`.
pub(crate) fn runnable(fixes: &[Fix]) -> Vec<Fix> {
    fixes
        .iter()
        .map(|f| match &f.action {
            FixAction::Set {
                record: Some(r),
                path,
                value,
            } if r.starts_with("e_") => set(f.title.clone(), r, path, value.clone()),
            _ => f.clone(),
        })
        .collect()
}
