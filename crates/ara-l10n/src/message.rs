use crate::{ParamKind, Result, error};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone)]
pub(crate) enum Node {
    Text(String),
    Value(String, Style),
    Pound,
    Select {
        name: String,
        plural: Option<bool>,
        offset: u32,
        arms: BTreeMap<String, Vec<Node>>,
    },
}

#[derive(Debug, Clone)]
pub(crate) enum Style {
    Default,
    Number,
    Integer,
    Date(String),
    Currency(Option<String>),
}

/// Parsed, signature-checked ICU MessageFormat pattern. Compile once on load/reload.
#[derive(Debug, Clone)]
pub struct Message {
    pub(crate) nodes: Vec<Node>,
    pub(crate) params: BTreeMap<String, ParamKind>,
}

impl Message {
    /// Parses nested select, plural/selectordinal, exact selectors, offsets and
    /// apostrophe quoting. Unknown format styles fail instead of silently degrading.
    pub fn parse(pattern: &str, params: &BTreeMap<String, ParamKind>) -> Result<Self> {
        if pattern.len() > 1_048_576 {
            return Err(error("message exceeds 1 MiB"));
        }
        let mut parser = Parser {
            text: pattern,
            at: 0,
            params,
            used: BTreeSet::new(),
        };
        let nodes = parser.sequence(0, false, false)?;
        if parser.used.len() != params.len() {
            return Err(error("declared parameters must all appear in the message"));
        }
        Ok(Self {
            nodes,
            params: params.clone(),
        })
    }
}

struct Parser<'a> {
    text: &'a str,
    at: usize,
    params: &'a BTreeMap<String, ParamKind>,
    used: BTreeSet<String>,
}

impl Parser<'_> {
    fn peek(&self) -> Option<char> {
        self.text[self.at..].chars().next()
    }
    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.at += c.len_utf8();
        Some(c)
    }
    fn space(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.bump();
        }
    }
    fn expect(&mut self, c: char) -> Result<()> {
        self.space();
        if self.bump() == Some(c) {
            Ok(())
        } else {
            Err(error(format!("expected {c:?} near byte {}", self.at)))
        }
    }
    fn token(&mut self) -> String {
        self.space();
        let start = self.at;
        while self
            .peek()
            .is_some_and(|c| !c.is_whitespace() && !matches!(c, ',' | '{' | '}'))
        {
            self.bump();
        }
        self.text[start..self.at].into()
    }
    fn use_param(&mut self, name: &str, required: Option<ParamKind>) -> Result<()> {
        if matches!(name, "self" | "Self" | "super" | "crate") {
            return Err(error(
                "parameter name is reserved by generated Rust bindings",
            ));
        }
        let actual = self
            .params
            .get(name)
            .ok_or_else(|| error(format!("undeclared parameter {name}")))?;
        if required.is_some_and(|kind| kind != *actual) {
            return Err(error(format!("parameter {name} has incompatible type")));
        }
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            || !name.starts_with(|c: char| c.is_ascii_alphabetic())
        {
            return Err(error("invalid parameter name"));
        }
        self.used.insert(name.into());
        Ok(())
    }
    fn sequence(&mut self, depth: usize, nested: bool, pound: bool) -> Result<Vec<Node>> {
        if depth > 32 {
            return Err(error("message nesting exceeds 32"));
        }
        let mut nodes = Vec::new();
        let mut text = String::new();
        while let Some(c) = self.peek() {
            match c {
                '}' if nested => break,
                '}' => return Err(error("unmatched closing brace")),
                '{' => {
                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }
                    nodes.push(self.argument(depth, pound)?);
                }
                '#' if pound => {
                    self.bump();
                    if !text.is_empty() {
                        nodes.push(Node::Text(std::mem::take(&mut text)));
                    }
                    nodes.push(Node::Pound);
                }
                '\'' => {
                    self.bump();
                    if self.peek() == Some('\'') {
                        self.bump();
                        text.push('\'');
                    } else if self
                        .peek()
                        .is_some_and(|c| matches!(c, '{' | '}') || (pound && c == '#'))
                    {
                        // ICU's apostrophe-friendly quoting: ordinary contractions stay literal.
                        while let Some(q) = self.bump() {
                            if q == '\'' {
                                if self.peek() == Some('\'') {
                                    self.bump();
                                    text.push('\'');
                                } else {
                                    break;
                                }
                            } else {
                                text.push(q);
                            }
                        }
                    } else {
                        text.push('\'');
                    }
                }
                _ => {
                    self.bump();
                    text.push(c);
                }
            }
        }
        if nested && self.peek().is_none() {
            return Err(error("unterminated message branch"));
        }
        if !text.is_empty() {
            nodes.push(Node::Text(text));
        }
        Ok(nodes)
    }
    fn argument(&mut self, depth: usize, inherited_pound: bool) -> Result<Node> {
        self.expect('{')?;
        let name = self.token();
        self.space();
        if self.peek() == Some('}') {
            self.bump();
            self.use_param(&name, None)?;
            return Ok(Node::Value(name, Style::Default));
        }
        self.expect(',')?;
        let kind = self.token();
        self.space();
        if matches!(kind.as_str(), "plural" | "selectordinal" | "select") {
            self.expect(',')?;
            let plural = (kind != "select").then_some(kind == "selectordinal");
            self.use_param(
                &name,
                Some(if plural.is_some() {
                    ParamKind::Number
                } else {
                    ParamKind::Text
                }),
            )?;
            self.space();
            let mut offset = 0;
            if self.text[self.at..].starts_with("offset:") {
                if plural.is_none() {
                    return Err(error("offset is only valid in plural messages"));
                }
                self.at += 7;
                offset = self
                    .token()
                    .parse()
                    .map_err(|_| error("plural offset must be a nonnegative integer"))?;
            }
            let mut arms = BTreeMap::new();
            loop {
                self.space();
                if self.peek() == Some('}') {
                    self.bump();
                    break;
                }
                let selector = self.token();
                if selector.is_empty() {
                    return Err(error("missing selector"));
                }
                if plural.is_some()
                    && !matches!(
                        selector.as_str(),
                        "zero" | "one" | "two" | "few" | "many" | "other"
                    )
                {
                    let exact = selector
                        .strip_prefix('=')
                        .ok_or_else(|| error("invalid plural selector"))?;
                    exact
                        .parse::<crate::Decimal>()
                        .map_err(|_| error("invalid exact plural number"))?;
                }
                self.expect('{')?;
                let body = self.sequence(depth + 1, true, plural.is_some() || inherited_pound)?;
                self.expect('}')?;
                if arms.insert(selector, body).is_some() {
                    return Err(error("duplicate message selector"));
                }
            }
            if !arms.contains_key("other") {
                return Err(error("select/plural requires an other branch"));
            }
            return Ok(Node::Select {
                name,
                plural,
                offset,
                arms,
            });
        }
        let style = if self.peek() == Some(',') {
            self.bump();
            let start = self.at;
            while self.peek().is_some_and(|c| c != '}') {
                self.bump();
            }
            self.text[start..self.at].trim().to_owned()
        } else {
            String::new()
        };
        self.expect('}')?;
        let (style, required) = match (kind.as_str(), style.as_str()) {
            ("number", "" | "decimal") => (Style::Number, ParamKind::Number),
            ("number", "integer") => (Style::Integer, ParamKind::Number),
            ("number", s) if s.starts_with("::currency/") => {
                (Style::Currency(Some(s[11..].into())), ParamKind::Currency)
            }
            ("currency", "") => (Style::Currency(None), ParamKind::Currency),
            ("date", "" | "short" | "medium" | "long" | "full") => {
                (Style::Date(style), ParamKind::Date)
            }
            _ => {
                return Err(error(format!(
                    "unsupported formatter/style {kind}, {style}"
                )));
            }
        };
        self.use_param(&name, Some(required))?;
        Ok(Node::Value(name, style))
    }
}
