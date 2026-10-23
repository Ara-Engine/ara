//! Human-readable rendering, in the style of rustc:
//!
//! ```text
//! error[ARA1101]: unknown field `positon` on `Transform`
//!   --> levels/arena.scene · e_8b21 · Transform.positon
//!   = about: asset:@meshes/crate.glb
//!   = fix: Rename `positon` to `position`
//! ```

use std::fmt;

use crate::{Diagnostic, Location, Severity};

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        })
    }
}

impl fmt::Display for Location {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts = Vec::new();
        if let Some(file) = &self.file {
            parts.push(match (self.line, self.column) {
                (Some(l), Some(c)) => format!("{file}:{l}:{c}"),
                (Some(l), None) => format!("{file}:{l}"),
                _ => file.clone(),
            });
        } else if let Some(doc) = &self.doc {
            parts.push(doc.clone());
        }
        if let Some(i) = self.command {
            parts.push(format!("call #{i}"));
        }
        if let Some(r) = &self.record {
            parts.push(r.clone());
        }
        if let Some(p) = &self.path {
            parts.push(p.clone());
        }
        f.write_str(&parts.join(" · "))
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}[{}]: {}", self.severity, self.code, self.message)?;
        if !self.location.is_empty() {
            write!(f, "\n  --> {}", self.location)?;
        }
        if !self.subjects.is_empty() {
            let about: Vec<String> = self.subjects.iter().map(ToString::to_string).collect();
            write!(f, "\n  = about: {}", about.join(", "))?;
        }
        for fix in &self.fixes {
            write!(f, "\n  = fix: {}", fix.title)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{Diagnostic, FixAction, codes};

    #[test]
    fn renders_like_rustc() {
        let d = Diagnostic::error(
            codes::UNKNOWN_FIELD,
            "unknown field `positon` on `Transform`",
        )
        .file("levels/arena.scene")
        .record("e_8b21")
        .path("Transform.positon")
        .with_fix(
            "Rename `positon` to `position`",
            FixAction::Rename {
                record: None,
                path: "Transform.positon".into(),
                to: "position".into(),
            },
        );
        assert_eq!(
            d.to_string(),
            "error[ARA1101]: unknown field `positon` on `Transform`\n  --> levels/arena.scene · e_8b21 · Transform.positon\n  = fix: Rename `positon` to `position`"
        );
    }
}
