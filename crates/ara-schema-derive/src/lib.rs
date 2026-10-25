//! `#[derive(Schema)]` for the Ara Schema DB.
//!
//! Structs with named fields and unit-only enums are supported. Doc comments are
//! required on the type, every field and every variant; they become inspector
//! tooltips, docs and agent tool descriptions. Field names are stored in camelCase
//! and variant names with a lowercase first letter, matching
//! `#[serde(rename_all = "camelCase")]`.
//!
//! Type attributes, `#[ara(...)]`:
//! - role: `component`, `params`, `record(prefix = "e", components)`; none means a
//!   plain value struct;
//! - `name = "..."`, `module = "..."`, `version = 2`, `alias = "Old"`,
//!   `deprecated = "why"`, `upgrade = path::to::fn` (an `ara_schema::UpgradeFn`).
//!
//! Field attributes:
//! - `rename = "..."`, `skip`, `default = <literal>`, `example = <literal>` (repeatable);
//! - `unit = Meters` (an `ara_schema::Unit` variant), `range(min = 0, max = 1)`;
//! - `since = 2`, `deprecated = "why"`, `alias = "old"`;
//! - `replicate(audience = Relevant, quantize = 0.01, interpolate = Linear, rollback)`;
//! - `editor(group = "...", slider, step = 0.1, multiline, hidden, read_only, color)`;
//! - `editor(show_if(field = "model", set))`, `show_if(field = "model", empty)` or
//!   `show_if(field = "kind", one_of = ["spot"])` (an `ara_schema::ShowIf`);
//! - `keyed` (a `Vec` of structs with `id`, addressed by ID), `asset_kind = "prefab"`.
//!
//! Variant attributes: `rename`, `alias`, `deprecated`.
//!
//! Literals are JSON-like: numbers, strings, booleans, `null` and arrays of those.

use proc_macro::TokenStream;
use proc_macro2::{Literal, TokenStream as Ts};
use quote::quote;
use syn::meta::ParseNestedMeta;
use syn::{
    Attribute, Data, DeriveInput, Expr, Fields, Ident, Lit, LitInt, LitStr, Meta, Path, Type, UnOp,
    parse_macro_input,
};

/// Implements `ara_schema::Schema` and `ara_schema::FieldType`. See the crate docs.
#[proc_macro_derive(Schema, attributes(ara))]
pub fn derive_schema(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

fn expand(input: &DeriveInput) -> syn::Result<Ts> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "ara: generic types cannot derive Schema",
        ));
    }
    let ident = &input.ident;
    let attrs = TypeAttrs::parse(&input.attrs)?;
    let doc = doc_of(&input.attrs)
        .ok_or_else(|| syn::Error::new_spanned(ident, "ara: every reflected type needs a doc comment (it becomes the tooltip, docs and agent tool description)"))?;
    let name = attrs.name.clone().unwrap_or_else(|| ident.to_string());

    let (shape, deps) = match &input.data {
        Data::Struct(s) => {
            let Fields::Named(named) = &s.fields else {
                return Err(syn::Error::new_spanned(
                    &s.fields,
                    "ara: only structs with named fields can derive Schema",
                ));
            };
            let mut fields = Vec::new();
            let mut deps = Vec::new();
            for f in &named.named {
                let fa = FieldAttrs::parse(&f.attrs)?;
                if fa.skip {
                    continue;
                }
                deps.push(f.ty.clone());
                fields.push(field_tokens(
                    f,
                    &fa,
                    attrs.role == Role::Params && attrs.record.is_none(),
                )?);
            }
            let role = attrs.role_tokens();
            (
                quote!(::ara_schema::Shape::Struct { role: #role, fields: ::std::vec![#(#fields),*] }),
                deps,
            )
        }
        Data::Enum(e) => {
            if attrs.role != Role::Value || attrs.record.is_some() {
                return Err(syn::Error::new_spanned(
                    ident,
                    "ara: enums cannot be components, records or params",
                ));
            }
            let mut variants = Vec::new();
            for v in &e.variants {
                if !matches!(v.fields, Fields::Unit) {
                    return Err(syn::Error::new_spanned(
                        v,
                        "ara: only unit variants are supported",
                    ));
                }
                let va = FieldAttrs::parse(&v.attrs)?;
                let doc = doc_of(&v.attrs).ok_or_else(|| {
                    syn::Error::new_spanned(v, "ara: every variant needs a doc comment")
                })?;
                let vname = va
                    .rename
                    .clone()
                    .unwrap_or_else(|| lower_first(&v.ident.to_string()));
                let aliases = &va.aliases;
                let deprecated = opt_string(&va.deprecated);
                variants.push(quote!(::ara_schema::VariantDesc {
                    name: ::std::string::String::from(#vname),
                    doc: ::std::string::String::from(#doc),
                    aliases: ::std::vec![#(::std::string::String::from(#aliases)),*],
                    deprecated: #deprecated,
                }));
            }
            (
                quote!(::ara_schema::Shape::Enum {
                    variants: ::std::vec![#(#variants),*]
                }),
                Vec::new(),
            )
        }
        Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                ident,
                "ara: unions cannot derive Schema",
            ));
        }
    };

    let module = attrs.module.clone().unwrap_or_default();
    let version = attrs.version.unwrap_or(1);
    let aliases = &attrs.aliases;
    let deprecated = opt_string(&attrs.deprecated);
    let upgrade = match &attrs.upgrade {
        Some(p) => quote!(::std::option::Option::Some(#p as ::ara_schema::UpgradeFn)),
        None => quote!(::std::option::Option::None),
    };
    Ok(quote! {
        impl ::ara_schema::Schema for #ident {
            fn describe() -> ::ara_schema::TypeDesc {
                ::ara_schema::TypeDesc {
                    name: ::std::string::String::from(#name),
                    doc: ::std::string::String::from(#doc),
                    module: ::std::string::String::from(#module),
                    version: #version,
                    aliases: ::std::vec![#(::std::string::String::from(#aliases)),*],
                    deprecated: #deprecated,
                    shape: #shape,
                    upgrade: #upgrade,
                }
            }

            fn register_deps(db: &mut ::ara_schema::SchemaDb) {
                let _ = &db;
                #(<#deps as ::ara_schema::FieldType>::register(db);)*
            }
        }

        impl ::ara_schema::FieldType for #ident {
            fn value_type() -> ::ara_schema::ValueType {
                ::ara_schema::ValueType::Named(::std::string::String::from(#name))
            }

            fn register(db: &mut ::ara_schema::SchemaDb) {
                db.register::<Self>();
            }
        }
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Value,
    Component,
    Params,
}

#[derive(Default)]
struct RecordAttr {
    prefix: String,
    components: bool,
}

struct TypeAttrs {
    role: Role,
    record: Option<RecordAttr>,
    name: Option<String>,
    module: Option<String>,
    version: Option<u32>,
    aliases: Vec<String>,
    deprecated: Option<String>,
    upgrade: Option<Path>,
}

impl Role {
    fn tokens(self) -> Ts {
        match self {
            Role::Value => quote!(::ara_schema::StructRole::Value),
            Role::Component => quote!(::ara_schema::StructRole::Component),
            Role::Params => quote!(::ara_schema::StructRole::Params),
        }
    }
}

impl TypeAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = TypeAttrs {
            role: Role::Value,
            record: None,
            name: None,
            module: None,
            version: None,
            aliases: Vec::new(),
            deprecated: None,
            upgrade: None,
        };
        for attr in attrs.iter().filter(|a| a.path().is_ident("ara")) {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("component") {
                    out.role = Role::Component;
                } else if meta.path.is_ident("params") {
                    out.role = Role::Params;
                } else if meta.path.is_ident("record") {
                    let mut rec = RecordAttr::default();
                    meta.parse_nested_meta(|m| {
                        if m.path.is_ident("prefix") {
                            rec.prefix = string(&m)?;
                        } else if m.path.is_ident("components") {
                            rec.components = true;
                        } else {
                            return Err(m.error("ara: expected `prefix` or `components`"));
                        }
                        Ok(())
                    })?;
                    out.record = Some(rec);
                } else if meta.path.is_ident("name") {
                    out.name = Some(string(&meta)?);
                } else if meta.path.is_ident("module") {
                    out.module = Some(string(&meta)?);
                } else if meta.path.is_ident("version") {
                    out.version = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
                } else if meta.path.is_ident("alias") {
                    out.aliases.push(string(&meta)?);
                } else if meta.path.is_ident("deprecated") {
                    out.deprecated = Some(string(&meta)?);
                } else if meta.path.is_ident("upgrade") {
                    out.upgrade = Some(meta.value()?.parse()?);
                } else {
                    return Err(meta.error("ara: unknown type attribute"));
                }
                Ok(())
            })?;
        }
        Ok(out)
    }
}

impl TypeAttrs {
    fn role_tokens(&self) -> Ts {
        match &self.record {
            Some(r) => {
                let (prefix, components) = (&r.prefix, r.components);
                quote!(::ara_schema::StructRole::Record(::ara_schema::RecordInfo {
                    prefix: ::std::string::String::from(#prefix),
                    components: #components,
                }))
            }
            None => self.role.tokens(),
        }
    }
}

#[derive(Default)]
struct FieldAttrs {
    rename: Option<String>,
    skip: bool,
    default: Option<Ts>,
    examples: Vec<Ts>,
    unit: Option<Ident>,
    min: Option<Expr>,
    max: Option<Expr>,
    since: Option<u32>,
    deprecated: Option<String>,
    aliases: Vec<String>,
    audience: Option<Ident>,
    quantize: Option<Expr>,
    interpolate: Option<Ident>,
    rollback: bool,
    group: Option<String>,
    slider: bool,
    step: Option<Expr>,
    multiline: bool,
    hidden: bool,
    read_only: bool,
    color: bool,
    /// `show_if`: the sibling field and the condition's tokens.
    show_if: Option<(String, Ts)>,
    keyed: bool,
    asset_kind: Option<String>,
}

impl FieldAttrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = FieldAttrs::default();
        for attr in attrs.iter().filter(|a| a.path().is_ident("ara")) {
            attr.parse_nested_meta(|meta| out.parse_one(&meta))?;
        }
        Ok(out)
    }

    fn parse_one(&mut self, meta: &ParseNestedMeta<'_>) -> syn::Result<()> {
        let is = |name: &str| meta.path.is_ident(name);
        if is("rename") {
            self.rename = Some(string(meta)?);
        } else if is("skip") {
            self.skip = true;
        } else if is("default") {
            self.default = Some(json_tokens(&meta.value()?.parse()?)?);
        } else if is("example") {
            self.examples.push(json_tokens(&meta.value()?.parse()?)?);
        } else if is("unit") {
            self.unit = Some(meta.value()?.parse()?);
        } else if is("range") {
            meta.parse_nested_meta(|m| {
                if m.path.is_ident("min") {
                    self.min = Some(m.value()?.parse()?);
                } else if m.path.is_ident("max") {
                    self.max = Some(m.value()?.parse()?);
                } else {
                    return Err(m.error("ara: expected `min` or `max`"));
                }
                Ok(())
            })?;
        } else if is("since") {
            self.since = Some(meta.value()?.parse::<LitInt>()?.base10_parse()?);
        } else if is("deprecated") {
            self.deprecated = Some(string(meta)?);
        } else if is("alias") {
            self.aliases.push(string(meta)?);
        } else if is("replicate") {
            meta.parse_nested_meta(|m| {
                if m.path.is_ident("audience") {
                    self.audience = Some(m.value()?.parse()?);
                } else if m.path.is_ident("quantize") {
                    self.quantize = Some(m.value()?.parse()?);
                } else if m.path.is_ident("interpolate") {
                    self.interpolate = Some(m.value()?.parse()?);
                } else if m.path.is_ident("rollback") {
                    self.rollback = true;
                } else {
                    return Err(m.error(
                        "ara: expected `audience`, `quantize`, `interpolate` or `rollback`",
                    ));
                }
                Ok(())
            })?;
        } else if is("editor") {
            meta.parse_nested_meta(|m| {
                if m.path.is_ident("group") {
                    self.group = Some(string(&m)?);
                } else if m.path.is_ident("slider") {
                    self.slider = true;
                } else if m.path.is_ident("step") {
                    self.step = Some(m.value()?.parse()?);
                } else if m.path.is_ident("multiline") {
                    self.multiline = true;
                } else if m.path.is_ident("hidden") {
                    self.hidden = true;
                } else if m.path.is_ident("read_only") {
                    self.read_only = true;
                } else if m.path.is_ident("color") {
                    self.color = true;
                } else if m.path.is_ident("show_if") {
                    self.show_if = Some(show_if(&m)?);
                } else {
                    return Err(m.error("ara: expected `group`, `slider`, `step`, `multiline`, `hidden`, `read_only`, `color` or `show_if`"));
                }
                Ok(())
            })?;
        } else if is("keyed") {
            self.keyed = true;
        } else if is("asset_kind") {
            self.asset_kind = Some(string(meta)?);
        } else {
            return Err(meta.error("ara: unknown field attribute"));
        }
        Ok(())
    }
}

fn field_tokens(f: &syn::Field, a: &FieldAttrs, params: bool) -> syn::Result<Ts> {
    let Some(ident) = &f.ident else {
        return Err(syn::Error::new_spanned(f, "ara: fields must be named"));
    };
    let rust_name = ident.to_string();
    let rust_name = rust_name.strip_prefix("r#").unwrap_or(&rust_name);
    let name = a.rename.clone().unwrap_or_else(|| camel(rust_name));
    let doc = doc_of(&f.attrs).ok_or_else(|| {
        syn::Error::new_spanned(ident, "ara: every reflected field needs a doc comment (it becomes the tooltip, docs and agent tool description)")
    })?;
    let ty = &f.ty;
    let mut ty_expr = quote!(<#ty as ::ara_schema::FieldType>::value_type());
    if a.keyed {
        ty_expr = quote!(#ty_expr.keyed());
    }
    if let Some(kind) = &a.asset_kind {
        ty_expr = quote!(#ty_expr.with_asset_kind(#kind));
    }
    let required = params && a.default.is_none() && !is_option(ty);
    let some = |t: Ts| quote!(::std::option::Option::Some(#t));
    let none = quote!(::std::option::Option::None);
    let default = a.default.clone().map_or(none.clone(), some);
    let unit = a
        .unit
        .as_ref()
        .map_or(none.clone(), |u| some(quote!(::ara_schema::Unit::#u)));
    let as_f64 = |e: &Option<Expr>| {
        e.as_ref()
            .map_or(none.clone(), |e| some(quote!((#e) as f64)))
    };
    let range = if a.min.is_some() || a.max.is_some() {
        let (min, max) = (as_f64(&a.min), as_f64(&a.max));
        some(quote!(::ara_schema::Range { min: #min, max: #max }))
    } else {
        none.clone()
    };
    let examples = &a.examples;
    let since = a.since.map_or(none.clone(), |s| some(quote!(#s)));
    let deprecated = opt_string(&a.deprecated);
    let aliases = &a.aliases;
    let audience = a
        .audience
        .clone()
        .unwrap_or_else(|| Ident::new("None", proc_macro2::Span::call_site()));
    let interpolate = a
        .interpolate
        .clone()
        .unwrap_or_else(|| Ident::new("None", proc_macro2::Span::call_site()));
    let quantize = as_f64(&a.quantize);
    let rollback = a.rollback;
    let group = opt_string(&a.group);
    let step = as_f64(&a.step);
    let (slider, multiline, hidden, read_only, color) =
        (a.slider, a.multiline, a.hidden, a.read_only, a.color);
    let show_if = a.show_if.as_ref().map_or(none.clone(), |(field, when)| {
        some(quote!(::ara_schema::ShowIf {
            field: ::std::string::String::from(#field),
            when: #when,
        }))
    });
    Ok(quote!(::ara_schema::FieldDesc {
        name: ::std::string::String::from(#name),
        ty: #ty_expr,
        doc: ::std::string::String::from(#doc),
        default: #default,
        required: #required,
        unit: #unit,
        range: #range,
        examples: ::std::vec![#(#examples),*],
        since: #since,
        deprecated: #deprecated,
        aliases: ::std::vec![#(::std::string::String::from(#aliases)),*],
        replicate: ::ara_schema::Replication {
            audience: ::ara_schema::Audience::#audience,
            quantize: #quantize,
            interpolate: ::ara_schema::Interpolate::#interpolate,
            rollback: #rollback,
        },
        editor: ::ara_schema::EditorHints {
            group: #group,
            slider: #slider,
            step: #step,
            multiline: #multiline,
            hidden: #hidden,
            read_only: #read_only,
            color: #color,
            show_if: #show_if,
        },
    }))
}

/// Parses `show_if(field = "...", set | empty | one_of = ["..."])`: the sibling's
/// serialized name and an `ara_schema::ShowWhen` expression.
fn show_if(meta: &ParseNestedMeta<'_>) -> syn::Result<(String, Ts)> {
    let (mut field, mut when) = (None, None);
    meta.parse_nested_meta(|m| {
        if m.path.is_ident("field") {
            field = Some(string(&m)?);
        } else if m.path.is_ident("set") {
            when = Some(quote!(::ara_schema::ShowWhen::Set));
        } else if m.path.is_ident("empty") {
            when = Some(quote!(::ara_schema::ShowWhen::Empty));
        } else if m.path.is_ident("one_of") {
            let list: syn::ExprArray = m.value()?.parse()?;
            let names = list
                .elems
                .iter()
                .map(|e| match e {
                    Expr::Lit(syn::ExprLit {
                        lit: Lit::Str(s), ..
                    }) => Ok(s.value()),
                    other => Err(syn::Error::new_spanned(other, "ara: expected a string")),
                })
                .collect::<syn::Result<Vec<_>>>()?;
            when = Some(quote!(::ara_schema::ShowWhen::OneOf(::std::vec![
                #(::std::string::String::from(#names)),*
            ])));
        } else {
            return Err(m.error("ara: expected `field`, `set`, `empty` or `one_of`"));
        }
        Ok(())
    })?;
    match (field, when) {
        (Some(field), Some(when)) => Ok((field, when)),
        _ => Err(meta.error(
            "ara: `show_if` needs `field = \"...\"` and one of `set`, `empty`, `one_of = [...]`",
        )),
    }
}

fn string(meta: &ParseNestedMeta<'_>) -> syn::Result<String> {
    Ok(meta.value()?.parse::<LitStr>()?.value())
}

fn opt_string(s: &Option<String>) -> Ts {
    match s {
        Some(s) => quote!(::std::option::Option::Some(::std::string::String::from(#s))),
        None => quote!(::std::option::Option::None),
    }
}

fn is_option(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Option"))
}

/// Joins `///` lines into the doc text: soft-wrapped lines join with a space;
/// blank lines (paragraphs) and list items keep their line breaks.
fn doc_of(attrs: &[Attribute]) -> Option<String> {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            Meta::NameValue(nv) => match &nv.value {
                Expr::Lit(syn::ExprLit {
                    lit: Lit::Str(s), ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|chunk| {
            chunk
                .lines()
                .map(|l| l.strip_prefix(' ').unwrap_or(l).trim_end().to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    let mut text = String::new();
    for line in &lines {
        let list_item = line.starts_with("- ") || line.starts_with("* ");
        if text.is_empty() {
            text.push_str(line);
        } else if line.is_empty() {
            text.push_str("\n\n");
        } else if text.ends_with('\n') || list_item {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(line);
        } else {
            text.push(' ');
            text.push_str(line);
        }
    }
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// `max_speed` -> `maxSpeed`, matching serde's `camelCase`.
fn camel(snake: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in snake.chars() {
        if c == '_' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

/// `SpotLight` -> `spotLight`, matching serde's `camelCase` on variants.
fn lower_first(s: &str) -> String {
    let mut chars = s.chars();
    chars
        .next()
        .map(|c| c.to_lowercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Converts a JSON-like literal expression into tokens building a `serde_json::Value`.
fn json_tokens(e: &Expr) -> syn::Result<Ts> {
    let value = quote!(::ara_schema::__private::Value);
    let err = || {
        syn::Error::new_spanned(
            e,
            "ara: expected a JSON-like literal (number, string, bool, null or array)",
        )
    };
    match e {
        Expr::Lit(l) => lit_tokens(&l.lit, false).ok_or_else(err),
        Expr::Unary(u) if matches!(u.op, UnOp::Neg(_)) => match u.expr.as_ref() {
            Expr::Lit(l) => lit_tokens(&l.lit, true).ok_or_else(err),
            _ => Err(err()),
        },
        Expr::Array(a) => {
            let items = a
                .elems
                .iter()
                .map(json_tokens)
                .collect::<syn::Result<Vec<_>>>()?;
            Ok(quote!(#value::Array(::std::vec![#(#items),*])))
        }
        Expr::Paren(p) => json_tokens(&p.expr),
        Expr::Group(g) => json_tokens(&g.expr),
        Expr::Path(p) if p.path.is_ident("null") => Ok(quote!(#value::Null)),
        _ => Err(err()),
    }
}

fn lit_tokens(lit: &Lit, negative: bool) -> Option<Ts> {
    let value = quote!(::ara_schema::__private::Value);
    Some(match lit {
        Lit::Int(i) if negative => {
            let n = Literal::i64_suffixed(i.base10_parse().ok()?);
            quote!(#value::from(-#n))
        }
        Lit::Int(i) => {
            let n = Literal::u64_suffixed(i.base10_parse().ok()?);
            quote!(#value::from(#n))
        }
        Lit::Float(f) => {
            let n = Literal::f64_suffixed(f.base10_parse().ok()?);
            if negative {
                quote!(#value::from(-#n))
            } else {
                quote!(#value::from(#n))
            }
        }
        Lit::Str(s) if !negative => quote!(#value::from(#s)),
        Lit::Bool(b) if !negative => {
            let b = b.value;
            quote!(#value::Bool(#b))
        }
        _ => return None,
    })
}
