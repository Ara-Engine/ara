use crate::{
    Message, ParamKind, Result, error,
    message::{Node, Style},
};
use icu_datetime::{DateTimeFormatter, fieldsets::YMD};
use icu_decimal::DecimalFormatter;
use icu_experimental::dimension::currency::formatter::CurrencyFormatter;
use icu_locale::Locale;
use icu_plurals::{PluralOperands, PluralRules};
use std::collections::BTreeMap;
use writeable::Writeable;

pub use fixed_decimal::Decimal;
pub use icu_datetime::input::Date;

/// Typed value supplied by Rust, generated C# bindings or a host command.
#[derive(Debug, Clone)]
pub enum Argument {
    /// Literal user/application text; bidi controls are sanitized on insertion.
    Text(String),
    /// Exact decimal preserving plural operands, including visible fraction digits.
    Number(Decimal),
    /// Validated proleptic ISO date, converted by ICU4X to the requested calendar.
    Date(IsoDate),
    /// Decimal amount and a validated-at-format ISO currency code.
    Currency {
        /// Exact amount.
        amount: Decimal,
        /// Three-letter ISO 4217 code, such as USD or EGP.
        code: String,
    },
}

impl Argument {
    /// Runtime type checked against the table's declared signature.
    pub fn kind(&self) -> ParamKind {
        match self {
            Self::Text(_) => ParamKind::Text,
            Self::Number(_) => ParamKind::Number,
            Self::Date(_) => ParamKind::Date,
            Self::Currency { .. } => ParamKind::Currency,
        }
    }
}

/// Deterministically ordered parameter values; no implicit string-to-number conversion.
pub type Arguments = BTreeMap<String, Argument>;

/// Validated ISO calendar date used by generated bindings.
pub type IsoDate = Date<icu_calendar::cal::Iso>;

/// Typed currency parameter used by generated bindings.
#[derive(Debug, Clone)]
pub struct Money {
    /// Exact decimal amount.
    pub amount: Decimal,
    /// Three-letter uppercase ISO 4217 currency code.
    pub code: String,
}

impl Message {
    /// Formats with the message language's plural grammar and requested locale's
    /// number/date/currency preferences. Every inserted value uses FSI/PDI isolation.
    pub fn format(&self, language: &str, locale: &str, args: &Arguments) -> Result<String> {
        if self.params.len() != args.len()
            || self
                .params
                .iter()
                .any(|(name, kind)| args.get(name).is_none_or(|arg| arg.kind() != *kind))
        {
            return Err(error(
                "missing, extra or incorrectly typed message arguments",
            ));
        }
        let locale: Locale = locale
            .parse()
            .map_err(|e| error(format!("invalid format locale: {e}")))?;
        let language: Locale = language
            .parse()
            .map_err(|e| error(format!("invalid message locale: {e}")))?;
        let formatter = Formatter {
            args,
            locale,
            language,
        };
        let mut out = String::new();
        formatter.render(&self.nodes, None, &mut out)?;
        Ok(out)
    }
}

struct Formatter<'a> {
    args: &'a Arguments,
    locale: Locale,
    language: Locale,
}

impl Formatter<'_> {
    fn number(&self, number: &Decimal) -> Result<String> {
        let formatter = DecimalFormatter::try_new((&self.locale).into(), Default::default())
            .map_err(|e| error(e.to_string()))?;
        Ok(formatter.format(number).write_to_string().into_owned())
    }
    fn value(&self, value: &Argument, style: &Style) -> Result<String> {
        match value {
            Argument::Text(s) => Ok(s.clone()),
            Argument::Number(n) => {
                if matches!(style, Style::Integer) {
                    let mut n = n.clone();
                    n.round(0);
                    self.number(&n)
                } else {
                    self.number(n)
                }
            }
            Argument::Date(date) => {
                let length = match style {
                    Style::Date(s) if s == "short" => YMD::short(),
                    Style::Date(s) if s == "long" || s == "full" => YMD::long(),
                    _ => YMD::medium(),
                };
                let formatter = DateTimeFormatter::try_new((&self.locale).into(), length)
                    .map_err(|e| error(e.to_string()))?;
                Ok(formatter.format(date).write_to_string().into_owned())
            }
            Argument::Currency { amount, code } => {
                if let Style::Currency(Some(expected)) = style
                    && expected != code
                {
                    return Err(error(
                        "currency argument does not match the message currency skeleton",
                    ));
                }
                if code.len() != 3 || !code.bytes().all(|b| b.is_ascii_uppercase()) {
                    return Err(error("currency must be a three-letter uppercase ISO code"));
                }
                let currency = code
                    .parse()
                    .map_err(|e| error(format!("invalid currency: {e}")))?;
                let formatter = CurrencyFormatter::try_new_symbol(
                    (&self.locale).into(),
                    currency,
                    Default::default(),
                )
                .map_err(|e| error(e.to_string()))?;
                Ok(formatter
                    .format_fixed_decimal(amount)
                    .write_to_string()
                    .into_owned())
            }
        }
    }
    fn render(&self, nodes: &[Node], pound: Option<&Decimal>, out: &mut String) -> Result<()> {
        for node in nodes {
            match node {
                Node::Text(s) => out.push_str(s),
                Node::Value(name, style) => isolate(&self.value(&self.args[name], style)?, out),
                Node::Pound => isolate(
                    &self.number(pound.ok_or_else(|| error("# outside plural"))?)?,
                    out,
                ),
                Node::Select {
                    name,
                    plural,
                    offset,
                    arms,
                } => {
                    if let Some(ordinal) = plural {
                        let Argument::Number(n) = &self.args[name] else {
                            return Err(error("plural needs number"));
                        };
                        let adjusted = subtract(n, *offset)?;
                        let exact = arms.iter().find(|(key, _)| {
                            key.strip_prefix('=').is_some_and(|s| decimal_equal(s, n))
                        });
                        let rules = if *ordinal {
                            PluralRules::try_new_ordinal((&self.language).into())
                        } else {
                            PluralRules::try_new_cardinal((&self.language).into())
                        }
                        .map_err(|e| error(e.to_string()))?;
                        let operands = PluralOperands::from(&adjusted);
                        let category = format!("{:?}", rules.category_for(operands)).to_lowercase();
                        let body = exact
                            .map(|(_, body)| body)
                            .or_else(|| arms.get(&category))
                            .unwrap_or(&arms["other"]);
                        self.render(body, Some(&adjusted), out)?;
                    } else {
                        let Argument::Text(selector) = &self.args[name] else {
                            return Err(error("select needs text"));
                        };
                        self.render(arms.get(selector).unwrap_or(&arms["other"]), pound, out)?;
                    }
                }
            }
            if out.len() > 4_194_304 {
                return Err(error("formatted message exceeds 4 MiB"));
            }
        }
        Ok(())
    }
}

fn decimal_equal(text: &str, value: &Decimal) -> bool {
    let Ok(mut other) = text.parse::<Decimal>() else {
        return false;
    };
    let mut value = value.clone();
    other.trim_end();
    other.trim_start();
    value.trim_end();
    value.trim_start();
    other == value
}

fn subtract(value: &Decimal, offset: u32) -> Result<Decimal> {
    if offset == 0 {
        return Ok(value.clone());
    }
    // Exact decimal arithmetic through signed coefficients, bounded to i128 for
    // offsets. The original value remains intact for exact-selector comparison.
    let text = value.to_string();
    let scale = text
        .split_once('.')
        .map_or(0, |(_, fraction)| fraction.len());
    let factor = 10_i128
        .checked_pow(scale as u32)
        .ok_or_else(|| error("plural offset precision exceeds i128"))?;
    let coefficient: i128 = text
        .replace('.', "")
        .parse()
        .map_err(|_| error("plural offset coefficient exceeds i128"))?;
    let result = coefficient
        .checked_sub(
            i128::from(offset)
                .checked_mul(factor)
                .ok_or_else(|| error("plural offset overflow"))?,
        )
        .ok_or_else(|| error("plural offset overflow"))?;
    let negative = result < 0;
    let mut digits = result.unsigned_abs().to_string();
    if scale > 0 {
        if digits.len() <= scale {
            digits = format!("{}{}", "0".repeat(scale + 1 - digits.len()), digits);
        }
        digits.insert(digits.len() - scale, '.');
    }
    if negative {
        digits.insert(0, '-');
    }
    digits
        .parse()
        .map_err(|e| error(format!("plural offset: {e}")))
}

fn isolate(value: &str, out: &mut String) {
    out.push('\u{2068}');
    // Remove embedding/override/isolate controls from untrusted inserted values so
    // a supplied PDI cannot escape the enclosing isolate and reorder surrounding UI.
    out.extend(
        value
            .chars()
            .filter(|c| !matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')),
    );
    out.push('\u{2069}');
}
