//! Lossless native UI source, compiled syntax and revision-checked designer operations.
#![warn(missing_docs)]
pub mod codes;
mod diagnostic;
mod edit;
mod markup;
pub mod stylesheet;
pub use diagnostic::{Diagnostic, Result};
pub use edit::{Command, DocumentKind, Patch, SourceDocument, SourceOp};
pub use markup::{Element, InlineStyle, compile, escape, inline_styles, parse, stable_id};
