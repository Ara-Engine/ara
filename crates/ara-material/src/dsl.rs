//! Bounded, named-expression DSL. Canonical printing preserves ASTs exactly;
//! whitespace and `//` comments are normalized. IDs are explicit, never position-based.

use std::collections::BTreeMap;

use ara_diag::Diagnostic;

use crate::{Binding, Cost, CustomFunction, Expr, Material, Parameter, codes};

#[derive(Debug, Clone)]
struct Token {
    text: String,
    offset: usize,
    quoted: bool,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<Token>,
    cursor: usize,
}

impl Parser<'_> {
    fn error(&self, message: impl Into<String>) -> Diagnostic {
        let offset = self
            .tokens
            .get(self.cursor)
            .map_or(self.source.len(), |t| t.offset);
        let prefix = &self.source[..offset];
        Diagnostic::error(codes::SYNTAX, message).line_col(
            prefix.bytes().filter(|&b| b == b'\n').count() as u32 + 1,
            prefix.rsplit('\n').next().unwrap_or("").chars().count() as u32 + 1,
        )
    }
    fn peek(&self) -> &str {
        self.tokens.get(self.cursor).map_or("", |t| t.text.as_str())
    }
    fn take(&mut self) -> Result<String, Diagnostic> {
        let t = self
            .tokens
            .get(self.cursor)
            .ok_or_else(|| self.error("unexpected end of material"))?
            .text
            .clone();
        self.cursor += 1;
        Ok(t)
    }
    fn expect(&mut self, text: &str) -> Result<(), Diagnostic> {
        if self.peek() != text {
            return Err(self.error(format!("expected `{text}`, found `{}`", self.peek())));
        }
        self.cursor += 1;
        Ok(())
    }
    fn string(&mut self) -> Result<String, Diagnostic> {
        if !self.tokens.get(self.cursor).is_some_and(|t| t.quoted) {
            return Err(self.error("expected a JSON string"));
        }
        self.take()
    }
    fn number(&mut self) -> Result<f64, Diagnostic> {
        let n = self
            .peek()
            .parse::<f64>()
            .map_err(|_| self.error("expected a finite number"))?;
        if !n.is_finite() {
            return Err(self.error("number must be finite"));
        }
        self.cursor += 1;
        Ok(n)
    }
    fn integer(&mut self) -> Result<u32, Diagnostic> {
        let n = self
            .peek()
            .parse::<u32>()
            .map_err(|_| self.error("expected unsigned integer"))?;
        self.cursor += 1;
        Ok(n)
    }
    fn values(&mut self) -> Result<Vec<f64>, Diagnostic> {
        self.expect("[")?;
        let mut values = Vec::new();
        while self.peek() != "]" {
            values.push(self.number()?);
            if self.peek() != "]" {
                self.expect(",")?;
            }
        }
        self.expect("]")?;
        Ok(values)
    }
    fn ty(&mut self) -> Result<crate::Type, Diagnostic> {
        self.take()?.parse().map_err(|e| self.error(e))
    }
}

fn lex(source: &str) -> Result<Vec<Token>, Diagnostic> {
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < source.len() {
        let b = source.as_bytes()[i];
        if b.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if source[i..].starts_with("//") {
            i = source[i..].find('\n').map_or(source.len(), |n| i + n);
            continue;
        }
        let start = i;
        if b == b'"' {
            i += 1;
            let mut escaped = false;
            let mut closed = false;
            while i < source.len() {
                let c = source.as_bytes()[i];
                i += 1;
                if c == b'"' && !escaped {
                    closed = true;
                    break;
                }
                if c == b'\\' {
                    escaped = !escaped;
                } else {
                    escaped = false;
                }
            }
            if !closed {
                return Err(Diagnostic::error(codes::SYNTAX, "unterminated string"));
            }
            let value = ara_json::parse(&source[start..i])
                .map_err(|e| Diagnostic::error(codes::SYNTAX, e.message))?;
            let text = value
                .as_str()
                .ok_or_else(|| Diagnostic::error(codes::SYNTAX, "expected string"))?
                .to_owned();
            tokens.push(Token {
                text,
                offset: start,
                quoted: true,
            });
            continue;
        }
        if source[i..].starts_with("->") {
            i += 2;
        } else if b.is_ascii_alphabetic() || b == b'_' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_alphanumeric()
                    || b"_.".contains(&source.as_bytes()[i]))
            {
                i += 1;
            }
        } else if b.is_ascii_digit() || b == b'-' || b == b'+' {
            i += 1;
            while i < source.len()
                && (source.as_bytes()[i].is_ascii_digit()
                    || b".eE+-".contains(&source.as_bytes()[i]))
            {
                i += 1;
            }
        } else if b"{}():=;,[]".contains(&b) {
            i += 1;
        } else {
            return Err(Diagnostic::error(
                codes::SYNTAX,
                format!("unexpected character at byte {i}"),
            ));
        }
        tokens.push(Token {
            text: source[start..i].to_owned(),
            offset: start,
            quoted: false,
        });
    }
    Ok(tokens)
}

/// Parses source into the single material AST; forward names resolve to stable IDs.
/// Run `check` before cooking or persisting authoring changes.
pub fn parse(source: &str) -> Result<Material, Vec<Diagnostic>> {
    parse_inner(source).map_err(|d| vec![d])
}

fn parse_inner(source: &str) -> Result<Material, Diagnostic> {
    let mut p = Parser {
        source,
        tokens: lex(source)?,
        cursor: 0,
    };
    p.expect("material")?;
    let id = p.take()?;
    let name = p.take()?;
    p.expect(":")?;
    let domain = p.take()?.parse().map_err(|e| p.error(e))?;
    p.expect("budget")?;
    let variant_budget = p.integer()?;
    p.expect("{")?;
    let mut m = Material {
        id,
        name,
        domain,
        variant_budget,
        parameters: BTreeMap::new(),
        bindings: BTreeMap::new(),
        functions: BTreeMap::new(),
        outputs: BTreeMap::new(),
    };
    let mut names = BTreeMap::new();
    while p.peek() != "}" {
        match p.take()?.as_str() {
            "param" | "let" => {
                let parameter = p.tokens[p.cursor - 1].text == "param";
                let id = p.take()?;
                let name = p.take()?;
                p.expect(":")?;
                let ty = p.ty()?;
                p.expect("=")?;
                if names.insert(name.clone(), id.clone()).is_some() || m.node_type(&id).is_some() {
                    return Err(p.error("duplicate variable name or node ID"));
                }
                if parameter {
                    let default = p.values()?;
                    let mut variants = Vec::new();
                    if p.peek() == "variants" {
                        p.take()?;
                        p.expect("[")?;
                        while p.peek() != "]" {
                            variants.push(p.values()?);
                            if p.peek() != "]" {
                                p.expect(",")?;
                            }
                        }
                        p.expect("]")?;
                    }
                    m.parameters.insert(
                        id,
                        Parameter {
                            name,
                            ty,
                            default,
                            variants,
                        },
                    );
                } else {
                    let expr = if p.peek() == "[" {
                        Expr::Literal {
                            values: p.values()?,
                        }
                    } else {
                        let op = p.take()?;
                        p.expect("(")?;
                        let mut args = Vec::new();
                        while p.peek() != ")" {
                            args.push(p.take()?);
                            if p.peek() != ")" {
                                p.expect(",")?;
                            }
                        }
                        p.expect(")")?;
                        Expr::Call { op, args }
                    };
                    m.bindings.insert(id, Binding { name, ty, expr });
                }
                p.expect(";")?;
            }
            "custom" => {
                let name = p.take()?;
                p.expect("(")?;
                let mut inputs = Vec::new();
                while p.peek() != ")" {
                    let name = p.take()?;
                    p.expect(":")?;
                    inputs.push((name, p.ty()?));
                    if p.peek() != ")" {
                        p.expect(",")?;
                    }
                }
                p.expect(")")?;
                p.expect("->")?;
                let output = p.ty()?;
                p.expect("cost")?;
                p.expect("[")?;
                let texture_samples = p.integer()?;
                p.expect(",")?;
                let alu = p.integer()?;
                p.expect(",")?;
                let branches = p.integer()?;
                p.expect("]")?;
                p.expect("doc")?;
                let doc = p.string()?;
                p.expect("slang")?;
                let body = p.string()?;
                p.expect(";")?;
                if m.functions
                    .insert(
                        name,
                        CustomFunction {
                            doc,
                            inputs,
                            output,
                            body,
                            cost: Cost {
                                texture_samples,
                                alu,
                                branches,
                            },
                        },
                    )
                    .is_some()
                {
                    return Err(p.error("duplicate custom function"));
                }
            }
            "output" => {
                let field = p.take()?;
                p.expect("=")?;
                let name = p.take()?;
                p.expect(";")?;
                if m.outputs.insert(field, name).is_some() {
                    return Err(p.error("duplicate output field"));
                }
            }
            other => return Err(p.error(format!("unexpected statement `{other}`"))),
        }
    }
    p.expect("}")?;
    if p.cursor != p.tokens.len() {
        return Err(p.error("unexpected content after material"));
    }
    let resolve = |name: &mut String| -> Result<(), Diagnostic> {
        *name = names
            .get(name)
            .ok_or_else(|| {
                Diagnostic::error(codes::REFERENCE, format!("unknown binding `{name}`"))
            })?
            .clone();
        Ok(())
    };
    for binding in m.bindings.values_mut() {
        if let Expr::Call { args, .. } = &mut binding.expr {
            for arg in args {
                resolve(arg)?;
            }
        }
    }
    for output in m.outputs.values_mut() {
        resolve(output)?;
    }
    Ok(m)
}

fn values(v: &[f64]) -> String {
    format!(
        "[{}]",
        v.iter()
            .map(|&v| ara_json::format_f64(v))
            .collect::<Vec<_>>()
            .join(", ")
    )
}
fn quoted(s: &str) -> String {
    serde_json::to_string(s).expect("strings always serialize as JSON")
}

/// Canonical text view: sorted persistent IDs, canonical numbers and LF newlines.
/// Parsing the result preserves the complete AST, including opaque Slang strings.
pub fn print(m: &Material) -> Result<String, Diagnostic> {
    let finite = m.parameters.values().all(|p| {
        p.default
            .iter()
            .chain(p.variants.iter().flatten())
            .all(|v| v.is_finite())
    }) && m.bindings.values().all(|b| match &b.expr {
        Expr::Literal { values } => values.iter().all(|v| v.is_finite()),
        _ => true,
    });
    if !finite {
        return Err(Diagnostic::error(
            codes::SYNTAX,
            "canonical material text requires finite numbers",
        ));
    }
    let name = |id: &str| {
        m.node_name(id)
            .ok_or_else(|| Diagnostic::error(codes::REFERENCE, format!("unknown binding `{id}`")))
    };
    let mut out = format!(
        "material {} {} : {} budget {} {{\n",
        m.id, m.name, m.domain, m.variant_budget
    );
    for (name, f) in &m.functions {
        let args = f
            .inputs
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!(
            "  custom {name}({args}) -> {} cost [{}, {}, {}] doc {} slang {};\n",
            f.output,
            f.cost.texture_samples,
            f.cost.alu,
            f.cost.branches,
            quoted(&f.doc),
            quoted(&f.body)
        ));
    }
    for (id, p) in &m.parameters {
        out.push_str(&format!(
            "  param {id} {} : {} = {}",
            p.name,
            p.ty,
            values(&p.default)
        ));
        if !p.variants.is_empty() {
            out.push_str(&format!(
                " variants [{}]",
                p.variants
                    .iter()
                    .map(|v| values(v))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        out.push_str(";\n");
    }
    for (id, b) in &m.bindings {
        let expr = match &b.expr {
            Expr::Literal { values: v } => values(v),
            Expr::Call { op, args } => format!(
                "{op}({})",
                args.iter()
                    .map(|id| name(id))
                    .collect::<Result<Vec<_>, _>>()?
                    .join(", ")
            ),
        };
        out.push_str(&format!("  let {id} {} : {} = {expr};\n", b.name, b.ty));
    }
    for (field, id) in &m.outputs {
        out.push_str(&format!("  output {field} = {};\n", name(id)?));
    }
    out.push_str("}\n");
    Ok(out)
}
