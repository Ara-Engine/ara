//! Canonical text: two-space indentation, fixed item order, minimal parentheses.
use crate::ast::*;
use std::fmt::Write;

/// Prints a behaviour in canonical form; parsing it back yields the same AST.
pub fn print(b: &Behavior) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "behavior {} {{", b.name);
    for v in &b.vars {
        let _ = write!(out, "  var {}: {}", v.name, v.ty.name());
        if let Some(d) = &v.default {
            let _ = write!(out, " = {}", expr(d));
        }
        out.push_str(";\n");
    }
    for q in &b.queries {
        if !out.ends_with("{\n") {
            out.push('\n');
        }
        let _ = writeln!(out, "  query {} {{", q.name);
        for g in &q.generators {
            let _ = writeln!(out, "    generate {};", call(g));
        }
        for f in &q.filters {
            let _ = writeln!(out, "    filter {};", expr(f));
        }
        for (s, w) in &q.scores {
            if *w == 1.0 {
                let _ = writeln!(out, "    score {};", expr(s));
            } else {
                let _ = writeln!(
                    out,
                    "    score {} weight {};",
                    expr(s),
                    number(f64::from(*w))
                );
            }
        }
        out.push_str("  }\n");
    }
    for s in &b.states {
        if !out.ends_with("{\n") {
            out.push('\n');
        }
        state(&mut out, s, 1);
    }
    out.push_str("}\n");
    out
}

fn state(out: &mut String, s: &State, depth: usize) {
    let pad = "  ".repeat(depth);
    let _ = write!(out, "{pad}state {}", s.name);
    if let Some(w) = &s.when {
        let _ = write!(out, " when {}", expr(w));
    }
    match s.select {
        Select::First { every } if every == 0.0 => {}
        Select::First { every } => {
            let _ = write!(out, " select first every {}", number(f64::from(every)));
        }
        Select::Utility { every } if every == 0.0 => out.push_str(" select utility"),
        Select::Utility { every } => {
            let _ = write!(out, " select utility every {}", number(f64::from(every)));
        }
        Select::Random => out.push_str(" select random"),
    }
    out.push_str(" {\n");
    let inner = "  ".repeat(depth + 1);
    if !s.enter.is_empty() {
        let _ = writeln!(out, "{inner}enter {};", actions(&s.enter));
    }
    if !s.exit.is_empty() {
        let _ = writeln!(out, "{inner}exit {};", actions(&s.exit));
    }
    for e in &s.scores {
        let _ = writeln!(out, "{inner}score {};", expr(e));
    }
    for t in &s.tasks {
        let _ = writeln!(out, "{inner}do {};", call(t));
    }
    for t in &s.transitions {
        let trigger = match &t.trigger {
            Trigger::Done => "done".to_owned(),
            Trigger::Failed => "failed".to_owned(),
            Trigger::When(e) => expr(e),
        };
        let _ = writeln!(out, "{inner}on {trigger} -> {};", t.target);
    }
    for c in &s.children {
        state(out, c, depth + 1);
    }
    let _ = writeln!(out, "{pad}}}");
}

fn actions(list: &[Action]) -> String {
    list.iter()
        .map(|a| match a {
            Action::Set(n, e) => format!("set {n} = {}", expr(e)),
            Action::Clear(n) => format!("clear {n}"),
            Action::Emit(n) => format!("emit {n}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn call(c: &Call) -> String {
    let args: Vec<String> = c
        .args
        .iter()
        .map(|a| match &a.name {
            Some(n) => format!("{n}: {}", expr(&a.value)),
            None => expr(&a.value),
        })
        .collect();
    format!("{}({})", c.name, args.join(", "))
}

/// Shortest round-tripping decimal; integers print without a fraction.
pub(crate) fn number(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e15 {
        format!("{}", n as i64)
    } else {
        format!("{n}")
    }
}

/// Prints an expression with the minimal parentheses for its precedence.
pub fn expr(e: &Expr) -> String {
    match e {
        Expr::Num(n) => number(*n),
        Expr::Bool(b) => b.to_string(),
        Expr::Str(s) => format!("\"{}\"", s.replace('"', "\\\"")),
        Expr::None => "none".into(),
        Expr::SelfRef => "self".into(),
        Expr::Time => "time".into(),
        Expr::Item => "item".into(),
        Expr::Var(v) => v.clone(),
        Expr::Vec3(v) => format!("vec3({}, {}, {})", expr(&v[0]), expr(&v[1]), expr(&v[2])),
        Expr::List(items) => format!(
            "[{}]",
            items.iter().map(expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::Call(n, args) => format!(
            "{n}({})",
            args.iter().map(expr).collect::<Vec<_>>().join(", ")
        ),
        Expr::Unary(op, inner) => {
            let text = match inner.as_ref() {
                Expr::Binary(..) => format!("({})", expr(inner)),
                // `- -1` would re-lex as a folded literal; keep it explicit.
                Expr::Num(n) if *n < 0.0 => format!("({})", expr(inner)),
                _ => expr(inner),
            };
            match op {
                UnOp::Not => format!("not {text}"),
                UnOp::Neg => format!("-{text}"),
            }
        }
        Expr::Binary(op, l, r) => {
            let side = |child: &Expr, right: bool| match child {
                Expr::Binary(c, ..)
                    if c.precedence() < op.precedence()
                        || (right && c.precedence() == op.precedence()) =>
                {
                    format!("({})", expr(child))
                }
                _ => expr(child),
            };
            format!("{} {} {}", side(l, false), op.symbol(), side(r, true))
        }
    }
}
