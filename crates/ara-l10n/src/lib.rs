//! Canonical localization documents, compiled ICU messages and translator workflows.
//! Authoring goes through [`TableCommand`] or the registered command/job registry.
//! Compile on a background worker; publish [`LiveCatalog`] only at a frame boundary.
#![warn(missing_docs)]

mod catalog;
mod codegen;
mod exchange;
mod format;
mod message;
mod table;
pub mod tools;

pub use catalog::{Catalog, LiveCatalog, Preview};
pub use codegen::{GeneratedKeys, generate_keys};
pub use exchange::{
    ExchangeUnit, export_po, export_xliff, import_commands, import_po, import_xliff,
};
pub use format::{Argument, Arguments, Date, Decimal, IsoDate, Money};
pub use message::Message;
pub use table::{
    FontFace, LocaleConfig, ParamKind, Provenance, Status, StringEntry, Table, TableCommand,
    register_schema,
};

/// Parameter structure implemented by generated Rust bindings.
pub trait Parameters {
    /// Converts owned, statically typed parameters to runtime arguments.
    fn into_arguments(self) -> Arguments;
}

impl Parameters for () {
    fn into_arguments(self) -> Arguments {
        Arguments::new()
    }
}

/// Generated key paired with its exact parameter structure.
#[derive(Debug)]
pub struct Key<P> {
    id: &'static str,
    marker: std::marker::PhantomData<fn(P)>,
}
impl<P> Copy for Key<P> {}
impl<P> Clone for Key<P> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<P> Key<P> {
    /// Constructs a key in generated code.
    pub const fn new(id: &'static str) -> Self {
        Self {
            id,
            marker: std::marker::PhantomData,
        }
    }
    /// Stable table lookup key.
    pub const fn id(self) -> &'static str {
        self.id
    }
}

/// Localization diagnostics, kept here until subsystem code catalogs are centralized.
pub mod codes {
    use ara_diag::Code;
    /// A locale is missing a source key.
    pub const MISSING_KEY: Code = Code(7401);
    /// A locale contains a key absent from the source table.
    pub const EXTRA_KEY: Code = Code(7402);
    /// Message syntax, parameter signature or runtime arguments are invalid.
    pub const MESSAGE: Code = Code(7403);
    /// A locale document or translator exchange file is invalid.
    pub const TABLE: Code = Code(7404);
    /// A generation response is stale or does not match its request.
    pub const TRANSLATION: Code = Code(7405);
}

/// A rejected localization operation, carrying an agent-readable diagnostic and fix.
pub type Result<T> = std::result::Result<T, ara_diag::Diagnostic>;

fn error(message: impl Into<String>) -> ara_diag::Diagnostic {
    ara_diag::Diagnostic::error(codes::MESSAGE, message)
}
