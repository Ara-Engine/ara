//! Recursive-descent parser: tokens to a name-based draft, then to the ID-keyed AST.

use super::lexer::{Tok, Token, error};
use crate::{
    ast::{AudioGraph, Node, Output, Param, ParamKind, Type, Value},
    library::Op,
};
use ara_diag::Diagnostic;
use std::collections::BTreeMap;

const KEYWORDS: &[&str] = &[
    "graph", "param", "trigger", "let", "output", "range", "true", "false", "_",
];

#[derive(Debug, Clone)]
pub(super) enum Raw {
    Number(f32),
    Clip(String),
    Ref(String, Option<String>),
    Placeholder,
}

struct RawLet {
    name: String,
    ty: Option<Type>,
    op: String,
    positional: Vec<(Raw, usize)>,
    named: Vec<(String, Raw, usize)>,
    offset: usize,
}

struct RawParam {
    name: String,
    kind: ParamKind,
    doc: String,
    offset: usize,
}

pub(super) struct Draft {
    name: String,
    params: Vec<RawParam>,
    lets: Vec<RawLet>,
    outputs: Vec<(Output, Raw, usize)>,
}

struct Cursor<'a> {
    source: &'a str,
    tokens: &'a [Token],
    at: usize,
}

impl Cursor<'_> {
    fn offset(&self) -> usize {
        self.tokens
            .get(self.at)
            .map_or(self.source.len(), |t| t.offset)
    }
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at).map(|t| &t.tok)
    }
    fn fail(&self, message: impl Into<String>) -> Diagnostic {
        error(self.source, self.offset(), message)
    }
    fn next(&mut self) -> Option<Tok> {
        let tok = self.tokens.get(self.at).map(|t| t.tok.clone());
        self.at += 1;
        tok
    }
    fn punct(&mut self, c: char) -> Result<(), Diagnostic> {
        match self.peek() {
            Some(Tok::Punct(p)) if *p == c => {
                self.at += 1;
                Ok(())
            }
            _ => Err(self.fail(format!("expected `{c}`"))),
        }
    }
    fn eat(&mut self, c: char) -> bool {
        if matches!(self.peek(), Some(Tok::Punct(p)) if *p == c) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn keyword(&mut self, word: &str) -> bool {
        if matches!(self.peek(), Some(Tok::Word(w)) if w == word) {
            self.at += 1;
            true
        } else {
            false
        }
    }
    fn ident(&mut self, what: &str) -> Result<String, Diagnostic> {
        match self.peek() {
            Some(Tok::Word(w)) if !w.contains('.') && !KEYWORDS.contains(&w.as_str()) => {
                let w = w.clone();
                self.at += 1;
                Ok(w)
            }
            _ => Err(self.fail(format!("expected {what} name"))),
        }
    }
    fn number(&mut self) -> Result<f32, Diagnostic> {
        match self.next() {
            Some(Tok::Number(n)) => Ok(n),
            _ => {
                self.at -= 1;
                Err(self.fail("expected a number"))
            }
        }
    }
    fn doc(&mut self) -> String {
        match self.peek() {
            Some(Tok::Str(s)) => {
                let s = s.clone();
                self.at += 1;
                s
            }
            _ => String::new(),
        }
    }
    fn value(&mut self) -> Result<Raw, Diagnostic> {
        match self.next() {
            Some(Tok::Number(n)) => Ok(Raw::Number(n)),
            Some(Tok::Clip(c)) => Ok(Raw::Clip(c)),
            Some(Tok::Word(w)) if w == "true" => Ok(Raw::Number(1.0)),
            Some(Tok::Word(w)) if w == "false" => Ok(Raw::Number(0.0)),
            Some(Tok::Word(w)) if w == "_" => Ok(Raw::Placeholder),
            Some(Tok::Word(w)) => Ok(match w.split_once('.') {
                Some((name, port)) => Raw::Ref(name.to_owned(), Some(port.to_owned())),
                None => Raw::Ref(w, None),
            }),
            _ => {
                self.at -= 1;
                Err(self.fail("expected a number, @clip or name"))
            }
        }
    }
    fn ty(&mut self) -> Result<Option<Type>, Diagnostic> {
        if !self.eat(':') {
            return Ok(None);
        }
        let offset = self.offset();
        match self.next() {
            Some(Tok::Word(w)) => Type::parse(&w).map(Some).ok_or_else(|| {
                error(
                    self.source,
                    offset,
                    format!("unknown type `{w}` (audio, trigger, clip)"),
                )
            }),
            _ => Err(error(self.source, offset, "expected a type")),
        }
    }
}

pub(super) fn parse(source: &str, tokens: &[Token]) -> Result<Draft, Diagnostic> {
    let mut c = Cursor {
        source,
        tokens,
        at: 0,
    };
    if !c.keyword("graph") {
        return Err(c.fail("expected `graph Name {`"));
    }
    let name = c.ident("graph")?;
    c.punct('{')?;
    let mut draft = Draft {
        name,
        params: Vec::new(),
        lets: Vec::new(),
        outputs: Vec::new(),
    };
    while !c.eat('}') {
        let offset = c.offset();
        if c.keyword("param") {
            let name = c.ident("parameter")?;
            let ty = c.ty()?;
            c.punct('=')?;
            let kind = match (ty, c.value()?) {
                (None | Some(Type::Audio), Raw::Number(default)) => {
                    let range = if c.keyword("range") {
                        c.punct('(')?;
                        let lo = c.number()?;
                        c.punct(',')?;
                        let hi = c.number()?;
                        c.punct(')')?;
                        Some([lo, hi])
                    } else {
                        None
                    };
                    ParamKind::Value { default, range }
                }
                (None | Some(Type::Clip), Raw::Clip(default)) => ParamKind::Clip { default },
                _ => {
                    return Err(error(
                        source,
                        offset,
                        "parameters default to a number (audio) or @clip (clip); declare triggers with `trigger name;`",
                    ));
                }
            };
            let doc = c.doc();
            c.punct(';')?;
            draft.params.push(RawParam {
                name,
                kind,
                doc,
                offset,
            });
        } else if c.keyword("trigger") {
            let name = c.ident("trigger")?;
            let doc = c.doc();
            c.punct(';')?;
            draft.params.push(RawParam {
                name,
                kind: ParamKind::Trigger,
                doc,
                offset,
            });
        } else if c.keyword("let") {
            let name = c.ident("node")?;
            let ty = c.ty()?;
            c.punct('=')?;
            let op = match c.next() {
                Some(Tok::Word(w)) => w,
                _ => {
                    c.at -= 1;
                    return Err(c.fail("expected an operation call such as `lowpass(x, 800)`"));
                }
            };
            c.punct('(')?;
            let mut positional = Vec::new();
            let mut named = Vec::new();
            if !c.eat(')') {
                loop {
                    let arg_offset = c.offset();
                    let is_named = matches!(c.peek(), Some(Tok::Word(w)) if !w.contains('.'))
                        && matches!(
                            c.tokens.get(c.at + 1).map(|t| &t.tok),
                            Some(Tok::Punct(':'))
                        );
                    if is_named {
                        let key = c.ident("argument")?;
                        c.punct(':')?;
                        named.push((key, c.value()?, arg_offset));
                    } else if named.is_empty() {
                        positional.push((c.value()?, arg_offset));
                    } else {
                        return Err(c.fail("positional arguments must come before named ones"));
                    }
                    if c.eat(')') {
                        break;
                    }
                    c.punct(',')?;
                }
            }
            c.punct(';')?;
            draft.lets.push(RawLet {
                name,
                ty,
                op,
                positional,
                named,
                offset,
            });
        } else if c.keyword("output") {
            let field_offset = c.offset();
            let field = c.ident("output")?;
            let Some(field) = Output::parse(&field) else {
                return Err(error(
                    source,
                    field_offset,
                    format!("unknown output `{field}` (out, left, right, done)"),
                ));
            };
            c.punct('=')?;
            let value = c.value()?;
            c.punct(';')?;
            draft.outputs.push((field, value, offset));
        } else if c.peek().is_none() {
            return Err(c.fail("missing `}` at the end of the graph"));
        } else {
            return Err(c.fail("expected `param`, `trigger`, `let`, `output` or `}`"));
        }
    }
    if c.peek().is_some() {
        return Err(c.fail("unexpected text after the graph"));
    }
    Ok(draft)
}

fn value(
    source: &str,
    raw: &Raw,
    offset: usize,
    ids: &BTreeMap<String, String>,
) -> Result<Option<Value>, Diagnostic> {
    Ok(match raw {
        Raw::Number(n) => Some(Value::number(*n)),
        Raw::Clip(c) => Some(Value::Clip {
            reference: c.clone(),
        }),
        Raw::Placeholder => None,
        Raw::Ref(name, port) => {
            let Some(id) = ids.get(name) else {
                let names: Vec<&str> = ids.keys().map(String::as_str).collect();
                let hint = ara_diag::did_you_mean(name, names)
                    .map_or(String::new(), |n| format!("; did you mean `{n}`?"));
                return Err(error(
                    source,
                    offset,
                    format!("unknown name `{name}`{hint}"),
                ));
            };
            Some(Value::Ref {
                node: id.clone(),
                port: port.clone(),
            })
        }
    })
}

/// Resolves names to IDs (`id_for`) and positional/named arguments to ports.
pub(super) fn build(
    source: &str,
    draft: Draft,
    asset_id: &str,
    id_for: impl Fn(&str) -> String,
) -> Result<AudioGraph, Vec<Diagnostic>> {
    let mut diags = Vec::new();
    let mut ids = BTreeMap::new();
    let declared = draft
        .params
        .iter()
        .map(|p| (&p.name, p.offset))
        .chain(draft.lets.iter().map(|l| (&l.name, l.offset)));
    for (name, offset) in declared {
        if ids.insert(name.clone(), id_for(name)).is_some() {
            diags.push(error(source, offset, format!("`{name}` is declared twice")));
        }
    }
    let mut graph = AudioGraph::new(asset_id, draft.name);
    for (order, p) in draft.params.into_iter().enumerate() {
        graph.params.insert(
            ids[&p.name].clone(),
            Param {
                name: p.name,
                order: order as u32,
                kind: p.kind,
                doc: p.doc,
            },
        );
    }
    for (order, l) in draft.lets.into_iter().enumerate() {
        let Some(op) = Op::from_name(&l.op) else {
            let names: Vec<&str> = Op::ALL.iter().map(|o| o.spec().name).collect();
            let hint = ara_diag::did_you_mean(&l.op, names)
                .map_or(String::new(), |n| format!("; did you mean `{n}`?"));
            diags.push(error(
                source,
                l.offset,
                format!("unknown operation `{}`{hint}", l.op),
            ));
            continue;
        };
        let spec = op.spec();
        let mut args = BTreeMap::new();
        let capacity = spec.inputs.len() + spec.variadic.map_or(0, |v| v.max);
        if l.positional.len() > capacity {
            diags.push(error(
                source,
                l.offset,
                format!("`{}` takes at most {capacity} arguments", spec.name),
            ));
        }
        for (index, (raw, offset)) in l.positional.iter().enumerate().take(capacity) {
            match value(source, raw, *offset, &ids) {
                Ok(Some(v)) => {
                    args.insert(spec.port(index), v);
                }
                Ok(None) if index >= spec.inputs.len() => {
                    diags.push(error(source, *offset, "variadic items cannot be `_`"));
                }
                Ok(None) => {}
                Err(d) => diags.push(d),
            }
        }
        for (key, raw, offset) in &l.named {
            if spec.input(key).is_none() {
                let names: Vec<&str> = spec.inputs.iter().map(|i| i.name).collect();
                diags.push(error(
                    source,
                    *offset,
                    format!(
                        "`{}` has no input `{key}` (inputs: {})",
                        spec.name,
                        names.join(", ")
                    ),
                ));
                continue;
            }
            match value(source, raw, *offset, &ids) {
                Ok(Some(v)) => {
                    if args.insert(key.clone(), v).is_some() {
                        diags.push(error(
                            source,
                            *offset,
                            format!("input `{key}` is given twice"),
                        ));
                    }
                }
                Ok(None) => {}
                Err(d) => diags.push(d),
            }
        }
        graph.nodes.insert(
            ids[&l.name].clone(),
            Node {
                name: l.name,
                order: order as u32,
                op,
                args,
                declared: l.ty,
            },
        );
    }
    for (field, raw, offset) in draft.outputs {
        match value(source, &raw, offset, &ids) {
            Ok(Some(v)) => {
                if graph.outputs.insert(field, v).is_some() {
                    diags.push(error(
                        source,
                        offset,
                        format!("output `{}` is set twice", field.name()),
                    ));
                }
            }
            Ok(None) => diags.push(error(source, offset, "outputs cannot be `_`")),
            Err(d) => diags.push(d),
        }
    }
    normalize_ports(&mut graph);
    if diags.is_empty() {
        Ok(graph)
    } else {
        Err(diags)
    }
}

/// A reference naming its target's first output stores no port: one AST per program.
fn normalize_ports(graph: &mut AudioGraph) {
    let first: BTreeMap<String, &'static str> = graph
        .nodes
        .iter()
        .map(|(id, n)| (id.clone(), n.op.spec().outputs[0].name))
        .collect();
    let fix = |v: &mut Value| {
        if let Value::Ref { node, port } = v
            && port.as_deref().is_some_and(|p| first.get(node) == Some(&p))
        {
            *port = None;
        }
    };
    for node in graph.nodes.values_mut() {
        node.args.values_mut().for_each(fix);
    }
    graph.outputs.values_mut().for_each(fix);
}
