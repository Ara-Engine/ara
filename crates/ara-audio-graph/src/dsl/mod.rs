//! The `.audiograph` text format: compact, name-based and ID-free.
//!
//! ```text
//! graph Wind {
//!   param strength = 0.5 range(0, 1) "Gust intensity from the weather system";
//!   let gust = noise.smooth(0.3, 7);
//!   let air = noise.pink(3);
//!   let shaped = map(gust, -1, 1, 300, 1800);
//!   let tone = bandpass(air, shaped, 1.5);
//!   let level = mul(tone, strength);
//!   output out = level;
//! }
//! ```
//!
//! Arguments are positional in the op's input order (`_` leaves an optional input
//! unconnected) or named (`lowpass(x, q: 2)`). `name.port` selects a secondary output.
//! Identity lives in the graph document: `reconcile` maps names to existing node IDs.

mod lexer;
mod parser;
mod printer;

use crate::ast::{AudioGraph, mint_id};
use ara_diag::Diagnostic;

pub(crate) use printer::dependency_order;
pub use printer::print;

/// Parses a new graph; node IDs are minted from names.
pub fn parse(source: &str, asset_id: &str) -> Result<AudioGraph, Vec<Diagnostic>> {
    let tokens = lexer::lex(source).map_err(|d| vec![d])?;
    let draft = parser::parse(source, &tokens).map_err(|d| vec![d])?;
    parser::build(source, draft, asset_id, mint_id)
}

/// Parses an edit of `previous`: surviving names keep their node IDs (and so their
/// canvas layout and overrides); new names get minted IDs.
pub fn reconcile(source: &str, previous: &AudioGraph) -> Result<AudioGraph, Vec<Diagnostic>> {
    let tokens = lexer::lex(source).map_err(|d| vec![d])?;
    let draft = parser::parse(source, &tokens).map_err(|d| vec![d])?;
    parser::build(source, draft, &previous.id, |name| {
        previous
            .find(name)
            .map_or_else(|| mint_id(name), |(id, _)| id.to_owned())
    })
}
