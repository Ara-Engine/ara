use super::lexer::{Token, lex};
use crate::{
    Binding, Cost, CustomFunction, Expr, Material, Parameter, ParameterValue, Type, codes,
};
use ara_diag::Diagnostic;
use std::collections::{BTreeMap, BTreeSet};

pub(super) struct Parser<'a> {
    pub source: &'a str,
    pub tokens: Vec<Token>,
    pub cursor: usize,
    pub issues: Vec<Diagnostic>,
}
impl Parser<'_> {
    pub fn error(&self, message: impl Into<String>) -> Diagnostic {
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
    pub fn peek(&self) -> &str {
        self.tokens.get(self.cursor).map_or("", |t| t.text.as_str())
    }
    pub fn take(&mut self) -> Result<String, Diagnostic> {
        let t = self
            .tokens
            .get(self.cursor)
            .ok_or_else(|| self.error("unexpected end of material"))?
            .text
            .clone();
        self.cursor += 1;
        Ok(t)
    }
    pub fn expect(&mut self, text: &str) -> Result<(), Diagnostic> {
        if self.peek() != text {
            return Err(self.error(format!("expected `{text}`, found `{}`", self.peek())));
        }
        self.cursor += 1;
        Ok(())
    }
    pub fn ident(&mut self) -> Result<String, Diagnostic> {
        let name = self.take()?;
        if !crate::check::identifier(&name) {
            return Err(self.error("expected an identifier"));
        }
        Ok(name)
    }
    pub fn number(&mut self) -> Result<f64, Diagnostic> {
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
            .parse()
            .map_err(|_| self.error("expected unsigned integer"))?;
        self.cursor += 1;
        Ok(n)
    }
    fn string(&mut self) -> Result<String, Diagnostic> {
        if !self.tokens.get(self.cursor).is_some_and(|t| t.quoted) {
            return Err(self.error("expected a JSON string"));
        }
        self.take()
    }
    fn ty(&mut self) -> Result<Type, Diagnostic> {
        self.take()?.parse().map_err(|e| self.error(e))
    }
    pub fn legacy(&mut self, message: &str) {
        let mut diagnostic = self.error(message);
        diagnostic.code = codes::LEGACY_SYNTAX;
        self.issues.push(diagnostic);
    }
    pub fn numbers(&mut self, open: &str, close: &str) -> Result<Vec<f64>, Diagnostic> {
        self.expect(open)?;
        let mut values = Vec::new();
        while self.peek() != close {
            values.push(self.number()?);
            if self.peek() != close {
                self.expect(",")?;
            }
        }
        self.expect(close)?;
        Ok(values)
    }
    fn custom(&mut self) -> Result<(String, CustomFunction), Diagnostic> {
        let name = self.ident()?;
        self.expect("(")?;
        let mut inputs = Vec::new();
        while self.peek() != ")" {
            let name = self.ident()?;
            self.expect(":")?;
            inputs.push((name, self.ty()?));
            if self.peek() != ")" {
                self.expect(",")?;
            }
        }
        self.expect(")")?;
        self.expect("->")?;
        let output = self.ty()?;
        self.expect("cost")?;
        let mut cost = Cost::default();
        if self.peek() == "[" {
            self.legacy("custom Slang costs use cost(alu: N, samples: N, branches: N), not positional arrays");
            self.expect("[")?;
            cost.texture_samples = self.integer()?;
            self.expect(",")?;
            cost.alu = self.integer()?;
            self.expect(",")?;
            cost.branches = self.integer()?;
            self.expect("]")?;
        } else {
            self.expect("(")?;
            let mut keys = BTreeSet::new();
            while self.peek() != ")" {
                let key = self.ident()?;
                if !keys.insert(key.clone()) {
                    return Err(self.error("duplicate cost field"));
                }
                self.expect(":")?;
                let n = self.integer()?;
                match key.as_str() {
                    "alu" => cost.alu = n,
                    "samples" => cost.texture_samples = n,
                    "branches" => cost.branches = n,
                    _ => return Err(self.error("cost fields are alu, samples and branches")),
                }
                if self.peek() != ")" {
                    self.expect(",")?;
                }
            }
            self.expect(")")?;
        }
        self.expect("doc")?;
        let doc = self.string()?;
        self.expect("slang")?;
        let body = self.string()?;
        self.expect(";")?;
        Ok((
            name,
            CustomFunction {
                doc,
                inputs,
                output,
                body,
                cost,
            },
        ))
    }
}

fn fresh(asset: &str, name: &str, reserved: &BTreeSet<String>) -> String {
    for salt in 0u64.. {
        let hash = blake3::hash(format!("{asset}/{name}/{salt}").as_bytes()).to_hex();
        let id = format!("n_{}", &hash[..20]);
        if !reserved.contains(&id) {
            return id;
        }
    }
    unreachable!("a finite material cannot exhaust the u64 identity salt space")
}

pub(super) fn parse(
    source: &str,
    asset_id: &str,
    previous: Option<&Material>,
) -> Result<Material, Vec<Diagnostic>> {
    let (material, issues) = inner(source, asset_id, previous).map_err(|d| vec![d])?;
    if issues.is_empty() {
        return Ok(material);
    }
    let replacement = super::print(&material).map_err(|d| vec![d])?;
    Err(issues
        .into_iter()
        .map(|d| super::source_fix(d, replacement.clone()))
        .collect())
}

fn inner(
    source: &str,
    asset_id: &str,
    previous: Option<&Material>,
) -> Result<(Material, Vec<Diagnostic>), Diagnostic> {
    let mut p = Parser {
        source,
        tokens: lex(source)?,
        cursor: 0,
        issues: Vec::new(),
    };
    p.expect("material")?;
    let mut name = p.ident()?;
    if ara_schema::grammar::split_id(&name).is_some_and(|(prefix, _)| prefix == "gr")
        && p.peek() != ":"
    {
        p.legacy("material asset IDs come from the file/graph, not the material header");
        name = p.ident()?;
    }
    p.expect(":")?;
    let domain = p.take()?.parse().map_err(|e| p.error(e))?;
    p.expect("budget")?;
    let variant_budget = p.integer()?;
    p.expect("{")?;
    let mut m = Material {
        id: asset_id.into(),
        name,
        domain,
        variant_budget,
        parameters: BTreeMap::new(),
        bindings: BTreeMap::new(),
        functions: BTreeMap::new(),
        outputs: BTreeMap::new(),
    };
    let mut reserved: BTreeSet<_> = previous
        .into_iter()
        .flat_map(|m| m.parameters.keys().chain(m.bindings.keys()).cloned())
        .collect();
    let mut names = BTreeMap::new();
    let mut casts = Vec::new();
    while p.peek() != "}" {
        let is_static = if p.peek() == "static" {
            p.take()?;
            true
        } else {
            false
        };
        let statement = p.take()?;
        if is_static && statement != "param" {
            return Err(p.error("static applies to boolean parameters"));
        }
        match statement.as_str() {
            "param" | "let" => {
                let parameter = statement == "param";
                let mut name = p.ident()?;
                if ara_schema::grammar::split_id(&name).is_some_and(|(prefix, _)| prefix == "n")
                    && p.peek() != ":"
                    && !p.peek().starts_with('#')
                {
                    p.legacy("semantic node IDs belong in the graph; declarations use names only");
                    name = p.ident()?;
                }
                let annotated = if p.peek().starts_with("#n_") {
                    Some(p.take()?[1..].to_owned())
                } else {
                    None
                };
                if !crate::check::value_name(&name) {
                    return Err(p.error("value name is a reserved keyword or literal"));
                }
                let id = if let Some(id) = annotated {
                    if id == "n_output"
                        || ara_schema::grammar::split_id(&id)
                            .is_none_or(|(prefix, _)| prefix != "n")
                        || m.node_type(&id).is_some()
                    {
                        return Err(
                            p.error("invalid, reserved or duplicate annotated graph identity")
                        );
                    }
                    id
                } else if let Some(id) = previous.and_then(|m| m.node_id(&name)) {
                    id.to_owned()
                } else {
                    fresh(asset_id, &name, &reserved)
                };
                if names.insert(name.clone(), id.clone()).is_some() || m.node_type(&id).is_some() {
                    return Err(p.error("material names and identities must be unique"));
                }
                reserved.insert(id.clone());
                p.expect(":")?;
                let mut ty = p.ty()?;
                p.expect("=")?;
                if parameter {
                    let mut default=p.literal(ty)?.ok_or_else(||p.error("parameter default must be a typed literal, asset reference or built-in texture"))?;
                    let mut is_static = is_static;
                    if p.peek() == "variants" {
                        p.legacy("use static param name : bool = true/false; variant sets derive from static switches");
                        p.take()?;
                        p.expect("[")?;
                        let mut choices = Vec::new();
                        while p.peek() != "]" {
                            choices.push(p.numbers("[", "]")?);
                            if p.peek() != "]" {
                                p.expect(",")?;
                            }
                        }
                        p.expect("]")?;
                        if ty == Type::Float
                            && choices.len() == 2
                            && choices.contains(&vec![0.0])
                            && choices.contains(&vec![1.0])
                        {
                            let enabled =
                                matches!(&default,ParameterValue::Numeric(v) if v==&[1.0]);
                            ty = Type::Bool;
                            default = ParameterValue::Bool(enabled);
                            is_static = true;
                            casts.push(id.clone());
                        }
                    }
                    m.parameters.insert(
                        id,
                        Parameter {
                            name,
                            order: m.parameters.len() as u32,
                            ty,
                            default,
                            is_static,
                        },
                    );
                } else {
                    let mut texture_default = None;
                    let expr = if let Some(value) = p.literal(ty)? {
                        let values = match value {
                            ParameterValue::Numeric(v) => v,
                            ParameterValue::Bool(v) => vec![if v { 1.0 } else { 0.0 }],
                            value @ ParameterValue::Texture(_) => {
                                p.legacy("texture resources are parameter declarations, not numeric let constants");
                                texture_default = Some(value);
                                Vec::new()
                            }
                        };
                        Expr::Literal { values }
                    } else {
                        let op = p.take()?;
                        if op.starts_with("custom.") {
                            p.legacy("call custom functions directly by name");
                        }
                        let mut args = Vec::new();
                        let op = if p.peek() == ";" {
                            args.push(op);
                            "copy".into()
                        } else {
                            p.expect("(")?;
                            while p.peek() != ")" {
                                args.push(p.ident()?);
                                if p.peek() != ")" {
                                    p.expect(",")?;
                                }
                            }
                            p.expect(")")?;
                            op
                        };
                        Expr::Call { op, args }
                    };
                    if let Some(default) = texture_default {
                        m.parameters.insert(
                            id,
                            Parameter {
                                name,
                                order: m.parameters.len() as u32,
                                ty,
                                default,
                                is_static: false,
                            },
                        );
                    } else {
                        m.bindings.insert(
                            id,
                            Binding {
                                name,
                                order: m.bindings.len() as u32,
                                ty,
                                expr,
                            },
                        );
                    }
                }
                p.expect(";")?;
            }
            "custom" => {
                let (name, function) = p.custom()?;
                if m.functions.insert(name, function).is_some() {
                    return Err(p.error("duplicate custom function"));
                }
            }
            "output" => {
                let field = p.ident()?;
                p.expect("=")?;
                let name = p.ident()?;
                p.expect(";")?;
                if m.outputs.insert(field, name).is_some() {
                    return Err(p.error("duplicate domain output"));
                }
            }
            _ => return Err(p.error(format!("unexpected statement `{statement}`"))),
        }
    }
    p.expect("}")?;
    if p.cursor != p.tokens.len() {
        return Err(p.error("unexpected content after material"));
    }
    for binding in m.bindings.values_mut() {
        if let Expr::Call { op, args } = &mut binding.expr {
            if m.functions.contains_key(op) {
                *op = format!("custom.{op}");
            }
            for arg in args {
                *arg = names
                    .get(arg)
                    .ok_or_else(|| p.error(format!("unknown binding `{arg}`")))?
                    .clone();
            }
        }
    }
    for output in m.outputs.values_mut() {
        *output = names
            .get(output)
            .ok_or_else(|| p.error(format!("unknown output value `{output}`")))?
            .clone();
    }
    for source in casts {
        let base = format!("{}Value", m.node_name(&source).unwrap_or("switch"));
        let mut name = base.clone();
        let mut salt = 0;
        while names.contains_key(&name) {
            salt += 1;
            name = format!("{base}{salt}");
        }
        let id = fresh(asset_id, &name, &reserved);
        reserved.insert(id.clone());
        names.insert(name.clone(), id.clone());
        for binding in m.bindings.values_mut() {
            if let Expr::Call { args, .. } = &mut binding.expr {
                for arg in args {
                    if *arg == source {
                        *arg = id.clone();
                    }
                }
            }
        }
        for output in m.outputs.values_mut() {
            if *output == source {
                *output = id.clone();
            }
        }
        m.bindings.insert(
            id,
            Binding {
                name,
                order: 0,
                ty: Type::Float,
                expr: Expr::Call {
                    op: "float".into(),
                    args: vec![source],
                },
            },
        );
    }
    m.normalize_order()?;
    Ok((m, p.issues))
}
