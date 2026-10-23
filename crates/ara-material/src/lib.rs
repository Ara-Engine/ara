//! One typed material AST with text and graph views (A8), cooked offline (D20).
//! Authoring commands reconcile either view into the same attributed kernel ops.

mod ast;
mod check;
pub mod commands;
mod compiler;
pub mod dsl;
mod generate;
pub mod instance_asset;
pub mod library;
mod projection;
mod variants;

pub use ast::*;
pub use check::{CheckedMaterial, check, fold_constants};
pub use compiler::{CompiledShader, ShaderCompiler, ShaderTarget};
pub use generate::{ENGINE_TEMPLATE, Generated, LineMapping, generate};
pub use instance_asset::{InstanceAsset, register};
pub use projection::{MaterialAdapter, edge_id};
pub use variants::*;

/// Material diagnostics, owned here until ara-diag's catalog can be extended.
pub mod codes {
    use ara_diag::Code;
    /// Invalid DSL syntax or AST structure.
    pub const SYNTAX: Code = Code(3200);
    /// Expression or output type mismatch.
    pub const TYPE: Code = Code(3201);
    /// Unknown binding, operation, or invalid custom function signature.
    pub const REFERENCE: Code = Code(3202);
    /// Operation or output is unavailable in this material domain.
    pub const DOMAIN: Code = Code(3203);
    /// Declared static combinations exceed the material's variant budget.
    pub const VARIANT_BUDGET: Code = Code(3204);
    /// Instance override is invalid.
    pub const OVERRIDE: Code = Code(3205);
    /// Slang process, target generation or artifact validation failed.
    pub const COMPILER: Code = Code(3206);
}
