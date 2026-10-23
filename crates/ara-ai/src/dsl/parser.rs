//! Recursive-descent parser. IDs come from the previous behaviour (by kind and
//! name) or are derived deterministically from the asset ID, kind and name.
use super::lexer::{Tok, Token, lex};
use crate::ast::*;
use crate::codes;
use ara_diag::Diagnostic;
use std::collections::BTreeMap;

/// Words that cannot name variables, states or queries.
pub const KEYWORDS: &[&str] = &[
    "behavior", "var", "query", "state", "when", "select", "first", "utility", "random", "every",
    "enter", "exit", "do", "score", "on", "done", "failed", "set", "clear", "emit", "generate",
    "filter", "weight", "not", "and", "or", "true", "false", "none", "self", "time", "item",
    "vec3",
];

/// Deterministic ID for a new named node of `kind` in asset `asset`.
pub fn fresh_id(asset: &str, kind: &str, name: &str) -> String {
    let hash = blake3::hash(format!("{asset}/{kind}/{name}").as_bytes()).to_hex();
    format!("n_{}", &hash[..16])
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    asset: &'a str,
    ids: BTreeMap<(&'static str, String), String>,
}

pub(crate) fn parse(
    source: &str,
    asset: &str,
    previous: Option<&Behavior>,
) -> Result<Behavior, Diagnostic> {
    let mut ids = BTreeMap::new();
    if let Some(p) = previous {
        for v in &p.vars {
            ids.insert(("var", v.name.clone()), v.id.clone());
        }
        for q in &p.queries {
            ids.insert(("query", q.name.clone()), q.id.clone());
        }
        for s in p.all_states() {
            ids.insert(("state", s.name.clone()), s.id.clone());
        }
    }
    let mut parser = Parser {
        tokens: lex(source)?,
        at: 0,
        asset,
        ids,
    };
    let behavior = parser.behavior()?;
    if parser.peek() != &Tok::End {
        return Err(parser.error("expected end of file after the behaviour"));
    }
    Ok(behavior)
}

/// Parses a standalone expression (blackboard overrides, tooling).
pub(crate) fn parse_expr(source: &str) -> Result<Expr, Diagnostic> {
    let mut parser = Parser {
        tokens: lex(source)?,
        at: 0,
        asset: "",
        ids: BTreeMap::new(),
    };
    let e = parser.expr(0)?;
    if parser.peek() != &Tok::End {
        return Err(parser.error("unexpected input after the expression"));
    }
    Ok(e)
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.tokens[self.at].tok
    }
    fn peek_at(&self, offset: usize) -> &Tok {
        &self.tokens[(self.at + offset).min(self.tokens.len() - 1)].tok
    }
    fn error(&self, msg: impl Into<String>) -> Diagnostic {
        let t = &self.tokens[self.at];
        Diagnostic::error(codes::SYNTAX, msg).line_col(t.line, t.col)
    }
    fn bump(&mut self) -> Tok {
        let t = self.tokens[self.at].tok.clone();
        if self.at + 1 < self.tokens.len() {
            self.at += 1;
        }
        t
    }
    fn is_punct(&self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }
    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == w)
    }
    fn punct(&mut self, p: &str) -> Result<(), Diagnostic> {
        if self.is_punct(p) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(format!("expected `{p}`")))
        }
    }
    fn word(&mut self, w: &str) -> Result<(), Diagnostic> {
        if self.is_word(w) {
            self.bump();
            Ok(())
        } else {
            Err(self.error(format!("expected `{w}`")))
        }
    }
    fn eat_word(&mut self, w: &str) -> bool {
        let found = self.is_word(w);
        if found {
            self.bump();
        }
        found
    }
    fn name(&mut self, what: &str) -> Result<String, Diagnostic> {
        match self.peek().clone() {
            Tok::Ident(s) if !KEYWORDS.contains(&s.as_str()) => {
                self.bump();
                Ok(s)
            }
            Tok::Ident(s) => {
                Err(self.error(format!("`{s}` is a keyword; choose another {what} name")))
            }
            _ => Err(self.error(format!("expected a {what} name"))),
        }
    }
    fn number(&mut self) -> Result<f64, Diagnostic> {
        let negative = self.is_punct("-");
        if negative {
            self.bump();
        }
        match self.bump() {
            Tok::Num(n) => Ok(if negative { -n } else { n }),
            _ => Err(self.error("expected a number")),
        }
    }
    fn id(&mut self, kind: &'static str, name: &str) -> String {
        self.ids
            .entry((kind, name.to_owned()))
            .or_insert_with(|| fresh_id(self.asset, kind, name))
            .clone()
    }

    fn behavior(&mut self) -> Result<Behavior, Diagnostic> {
        self.word("behavior")?;
        let name = self.name("behaviour")?;
        self.punct("{")?;
        let mut b = Behavior {
            id: self.asset.to_owned(),
            name,
            vars: Vec::new(),
            queries: Vec::new(),
            states: Vec::new(),
        };
        while !self.is_punct("}") {
            if self.eat_word("var") {
                let name = self.name("variable")?;
                self.punct(":")?;
                let ty = match self.bump() {
                    Tok::Ident(t) => Type::parse(&t).ok_or_else(|| {
                        self.error(format!(
                            "unknown type `{t}`; use bool, float, vec3, entity or points"
                        ))
                    })?,
                    _ => return Err(self.error("expected a type")),
                };
                let default = if self.is_punct("=") {
                    self.bump();
                    Some(self.expr(0)?)
                } else {
                    None
                };
                self.punct(";")?;
                let id = self.id("var", &name);
                b.vars.push(Var {
                    id,
                    name,
                    ty,
                    default,
                });
            } else if self.eat_word("query") {
                b.queries.push(self.query()?);
            } else if self.is_word("state") {
                b.states.push(self.state()?);
            } else {
                return Err(self.error("expected `var`, `query`, `state` or `}`"));
            }
        }
        self.punct("}")?;
        Ok(b)
    }

    fn query(&mut self) -> Result<QueryDef, Diagnostic> {
        let name = self.name("query")?;
        let id = self.id("query", &name);
        let mut q = QueryDef {
            id,
            name,
            generators: Vec::new(),
            filters: Vec::new(),
            scores: Vec::new(),
        };
        self.punct("{")?;
        while !self.is_punct("}") {
            if self.eat_word("generate") {
                q.generators.push(self.call()?);
            } else if self.eat_word("filter") {
                q.filters.push(self.expr(0)?);
            } else if self.eat_word("score") {
                let e = self.expr(0)?;
                let weight = if self.eat_word("weight") {
                    self.number()? as f32
                } else {
                    1.0
                };
                q.scores.push((e, weight));
            } else {
                return Err(self.error("expected `generate`, `filter`, `score` or `}`"));
            }
            self.punct(";")?;
        }
        self.punct("}")?;
        Ok(q)
    }

    fn state(&mut self) -> Result<State, Diagnostic> {
        self.word("state")?;
        let name = self.name("state")?;
        let id = self.id("state", &name);
        let when = if self.eat_word("when") {
            Some(self.expr(0)?)
        } else {
            None
        };
        let mut select = Select::First { every: 0.0 };
        if self.eat_word("select") {
            select = if self.eat_word("first") {
                Select::First { every: 0.0 }
            } else if self.eat_word("utility") {
                Select::Utility { every: 0.0 }
            } else if self.eat_word("random") {
                Select::Random
            } else {
                return Err(self.error("expected `first`, `utility` or `random`"));
            };
            if self.eat_word("every") {
                let every = self.number()? as f32;
                if every < 0.0 {
                    return Err(self.error("`every` must not be negative"));
                }
                select = match select {
                    Select::First { .. } => Select::First { every },
                    Select::Utility { .. } => Select::Utility { every },
                    Select::Random => return Err(self.error("random selection has no period")),
                };
            }
        }
        let mut s = State {
            id,
            name,
            when,
            select,
            enter: Vec::new(),
            exit: Vec::new(),
            tasks: Vec::new(),
            scores: Vec::new(),
            transitions: Vec::new(),
            children: Vec::new(),
        };
        self.punct("{")?;
        while !self.is_punct("}") {
            if self.is_word("state") {
                s.children.push(self.state()?);
                continue;
            }
            if self.eat_word("enter") {
                s.enter.extend(self.actions()?);
            } else if self.eat_word("exit") {
                s.exit.extend(self.actions()?);
            } else if self.eat_word("do") {
                s.tasks.push(self.call()?);
            } else if self.eat_word("score") {
                s.scores.push(self.expr(0)?);
            } else if self.eat_word("on") {
                let trigger = if self.is_word("done") && self.peek_at(1) == &Tok::Punct("->") {
                    self.bump();
                    Trigger::Done
                } else if self.is_word("failed") && self.peek_at(1) == &Tok::Punct("->") {
                    self.bump();
                    Trigger::Failed
                } else {
                    Trigger::When(self.expr(0)?)
                };
                self.punct("->")?;
                let target = self.name("target state")?;
                s.transitions.push(Transition { trigger, target });
            } else {
                return Err(
                    self.error("expected `enter`, `exit`, `do`, `score`, `on`, `state` or `}`")
                );
            }
            self.punct(";")?;
        }
        self.punct("}")?;
        Ok(s)
    }

    fn actions(&mut self) -> Result<Vec<Action>, Diagnostic> {
        let mut out = Vec::new();
        loop {
            let action = if self.eat_word("set") {
                let name = self.name("variable")?;
                self.punct("=")?;
                Action::Set(name, self.expr(0)?)
            } else if self.eat_word("clear") {
                Action::Clear(self.name("variable")?)
            } else if self.eat_word("emit") {
                Action::Emit(self.name("event")?)
            } else {
                return Err(self.error("expected `set`, `clear` or `emit`"));
            };
            out.push(action);
            if !self.is_punct(",") {
                return Ok(out);
            }
            self.bump();
        }
    }

    /// Task/generator call: positional and `name: value` arguments.
    fn call(&mut self) -> Result<Call, Diagnostic> {
        let name = match self.bump() {
            Tok::Ident(n) => n,
            _ => return Err(self.error("expected a call")),
        };
        self.punct("(")?;
        let mut args = Vec::new();
        while !self.is_punct(")") {
            let named = matches!(self.peek(), Tok::Ident(_)) && self.peek_at(1) == &Tok::Punct(":");
            let arg_name = if named {
                let Tok::Ident(n) = self.bump() else {
                    return Err(self.error("expected an argument name"));
                };
                self.bump();
                Some(n)
            } else {
                None
            };
            args.push(Arg {
                name: arg_name,
                value: self.expr(0)?,
            });
            if !self.is_punct(",") {
                break;
            }
            self.bump();
        }
        self.punct(")")?;
        Ok(Call { name, args })
    }

    fn binary_op(&self) -> Option<BinOp> {
        Some(match self.peek() {
            Tok::Ident(w) if w == "or" => BinOp::Or,
            Tok::Ident(w) if w == "and" => BinOp::And,
            Tok::Punct("==") => BinOp::Eq,
            Tok::Punct("!=") => BinOp::Ne,
            Tok::Punct("<") => BinOp::Lt,
            Tok::Punct("<=") => BinOp::Le,
            Tok::Punct(">") => BinOp::Gt,
            Tok::Punct(">=") => BinOp::Ge,
            Tok::Punct("+") => BinOp::Add,
            Tok::Punct("-") => BinOp::Sub,
            Tok::Punct("*") => BinOp::Mul,
            Tok::Punct("/") => BinOp::Div,
            _ => return None,
        })
    }

    /// Precedence climbing; all binary operators are left-associative.
    fn expr(&mut self, min: u8) -> Result<Expr, Diagnostic> {
        let mut left = self.unary()?;
        while let Some(op) = self.binary_op() {
            if op.precedence() <= min {
                break;
            }
            self.bump();
            let right = self.expr(op.precedence())?;
            left = Expr::Binary(op, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<Expr, Diagnostic> {
        if self.eat_word("not") {
            return Ok(Expr::Unary(UnOp::Not, Box::new(self.unary()?)));
        }
        if self.is_punct("-") {
            self.bump();
            return Ok(match self.unary()? {
                Expr::Num(n) => Expr::Num(-n),
                e => Expr::Unary(UnOp::Neg, Box::new(e)),
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Expr, Diagnostic> {
        match self.bump() {
            Tok::Num(n) => Ok(Expr::Num(n)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Punct("(") => {
                let e = self.expr(0)?;
                self.punct(")")?;
                Ok(e)
            }
            Tok::Punct("[") => {
                let mut items = Vec::new();
                while !self.is_punct("]") {
                    items.push(self.expr(0)?);
                    if !self.is_punct(",") {
                        break;
                    }
                    self.bump();
                }
                self.punct("]")?;
                Ok(Expr::List(items))
            }
            Tok::Ident(w) => match w.as_str() {
                "true" => Ok(Expr::Bool(true)),
                "false" => Ok(Expr::Bool(false)),
                "none" => Ok(Expr::None),
                "self" => Ok(Expr::SelfRef),
                "time" => Ok(Expr::Time),
                "item" => Ok(Expr::Item),
                "vec3" => {
                    self.punct("(")?;
                    let x = self.expr(0)?;
                    self.punct(",")?;
                    let y = self.expr(0)?;
                    self.punct(",")?;
                    let z = self.expr(0)?;
                    self.punct(")")?;
                    Ok(Expr::Vec3(Box::new([x, y, z])))
                }
                _ if KEYWORDS.contains(&w.as_str()) => {
                    self.at -= 1;
                    Err(self.error(format!("unexpected keyword `{w}` in an expression")))
                }
                _ if self.is_punct("(") => {
                    self.bump();
                    let mut args = Vec::new();
                    while !self.is_punct(")") {
                        args.push(self.expr(0)?);
                        if !self.is_punct(",") {
                            break;
                        }
                        self.bump();
                    }
                    self.punct(")")?;
                    Ok(Expr::Call(w, args))
                }
                _ => Ok(Expr::Var(w)),
            },
            _ => {
                self.at = self.at.saturating_sub(1);
                Err(self.error("expected an expression"))
            }
        }
    }
}
