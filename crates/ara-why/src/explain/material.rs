//! Material evidence shared by the visibility and asset explainers: does a
//! `.material` file compile, which domain is it, and are its opacity outputs a
//! constant zero?

use ara_diag::Diagnostic;
use ara_material::{Binding, Domain, Expr, ParameterValue, check, dsl, fold_constants};

use crate::{Asset, Facts};

/// What a material file says about drawing with it.
pub(crate) struct MaterialStatus {
    /// Parse and check errors, located in the file.
    pub errors: Vec<Diagnostic>,
    /// Its domain, when it parses.
    pub domain: Option<Domain>,
    /// Opacity outputs that are a constant zero: `(output, from a parameter default)`.
    pub zero: Vec<(&'static str, bool)>,
}

impl MaterialStatus {
    /// True for domains that do not draw scene meshes.
    pub fn draws_meshes(&self) -> bool {
        !matches!(self.domain, Some(Domain::Ui | Domain::PostProcess))
    }
}

/// Points an error that names an identifier (`unknown output value `pain``) at the
/// line that uses it, quotes that line, and suggests the declared name it most
/// likely meant. Parser locations can point at the end of the file.
fn pinpoint(mut d: Diagnostic, source: &str) -> Diagnostic {
    let Some(name) = d.message.split('`').nth(1).map(str::to_owned) else {
        return d;
    };
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    let found = source.lines().enumerate().find_map(|(i, line)| {
        let code = line.split("//").next().unwrap_or(line);
        code.match_indices(name.as_str())
            .find(|(at, _)| {
                let before = code[..*at].chars().next_back();
                let after = code[at + name.len()..].chars().next();
                !before.is_some_and(is_word) && !after.is_some_and(is_word)
            })
            .map(|(at, _)| (i, at, line.trim()))
    });
    let Some((line, column, text)) = found else {
        return d;
    };
    d.location.line = Some(line as u32 + 1);
    d.location.column = Some(column as u32 + 1);
    d.message.push_str(&format!(" in `{text}`"));
    // Declared names: `param x`, `static param x`, `let x`.
    let declared: Vec<&str> = source
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l
                .strip_prefix("static param ")
                .or_else(|| l.strip_prefix("param "))
                .or_else(|| l.strip_prefix("let "))?;
            rest.split(|c: char| !is_word(c)).next()
        })
        .collect();
    if !declared.contains(&name.as_str())
        && let Some(near) = ara_diag::did_you_mean(&name, declared.iter().copied())
    {
        d.message.push_str(&format!("; did you mean `{near}`?"));
    }
    d
}

/// Inspects a resolved `.material` asset; `None` for other assets or unreadable files.
pub(crate) fn inspect(facts: &dyn Facts, asset: &Asset) -> Option<MaterialStatus> {
    let path = asset.path.as_deref()?;
    if !path.ends_with(".material") {
        return None;
    }
    let source = facts.read(path)?;
    let id = asset.id.clone().unwrap_or_default();
    let located = |diags: Vec<Diagnostic>| -> Vec<Diagnostic> {
        diags
            .into_iter()
            .filter(Diagnostic::is_error)
            .map(|d| {
                let d = if d.location.file.is_none() {
                    d.file(path)
                } else {
                    d
                };
                pinpoint(d, &source)
            })
            .collect()
    };
    let material = match dsl::parse(&source, &id) {
        Ok(m) => m,
        Err(diags) => {
            return Some(MaterialStatus {
                errors: located(diags),
                domain: None,
                zero: Vec::new(),
            });
        }
    };
    if let Err(diags) = check(&material) {
        return Some(MaterialStatus {
            errors: located(diags),
            domain: Some(material.domain),
            zero: Vec::new(),
        });
    }
    let folded = fold_constants(&material).unwrap_or(material);
    let mut zero = Vec::new();
    for output in ["opacity", "opacityMask"] {
        let Some(node) = folded.outputs.get(output) else {
            continue;
        };
        let literal_zero = |b: &Binding| matches!(&b.expr, Expr::Literal { values } if values.iter().all(|v| *v == 0.0));
        if folded.bindings.get(node).is_some_and(literal_zero) {
            zero.push((output, false));
        } else if folded.parameters.get(node).is_some_and(
            |p| matches!(&p.default, ParameterValue::Numeric(v) if v.iter().all(|x| *x == 0.0)),
        ) {
            zero.push((output, true));
        }
    }
    Some(MaterialStatus {
        errors: Vec::new(),
        domain: Some(folded.domain),
        zero,
    })
}
