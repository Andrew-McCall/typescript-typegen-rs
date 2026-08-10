//! Rendering IR into TypeScript source text.
//!
//! This module knows nothing about Rust. All policy that depends on `Config` —
//! integer width, optionality style, interface vs alias — lives here.

use crate::config::{BigInt, Bytes, Config, OptionalStyle, StructStyle, UnknownStyle};
use crate::ir::{Body, Decl, EnumRepr, Field, Prim, Ty, Variant, VariantKind, placeholder_positions};

const INDENT: &str = "  ";

/// Renders one IR type as a TypeScript type expression, e.g. `string | null`.
pub fn emit_ty(ty: &Ty, cfg: &Config) -> String {
    match ty {
        Ty::Prim(p) => emit_prim(p, cfg),
        // A byte array only reaches TypeScript as text if the codec encodes it
        // that way; serde_json itself writes a number array, which is the default.
        Ty::Array(inner)
            if cfg.bytes_style() == Bytes::Str
                && **inner == Ty::Prim(Prim::Int { bits: 8, signed: false }) =>
        {
            "string".to_string()
        }
        Ty::Array(inner) => {
            let rendered = emit_ty(inner, cfg);
            if needs_parens(inner) {
                format!("({rendered})[]")
            } else {
                format!("{rendered}[]")
            }
        }
        Ty::Tuple(items) => {
            let parts: Vec<String> = items.iter().map(|t| emit_ty(t, cfg)).collect();
            format!("[{}]", parts.join(", "))
        }
        Ty::Record { key, value } => {
            format!("Record<{}, {}>", emit_ty(key, cfg), emit_ty(value, cfg))
        }
        Ty::Union(arms) => {
            let parts: Vec<String> = arms.iter().map(|t| emit_ty(t, cfg)).collect();
            parts.join(" | ")
        }
        Ty::Object(fields) => emit_object_inline(fields, cfg),
        Ty::Ref { name, args } if args.is_empty() => name.clone(),
        Ty::Ref { name, args } => {
            let parts: Vec<String> = args.iter().map(|t| emit_ty(t, cfg)).collect();
            format!("{name}<{}>", parts.join(", "))
        }
        Ty::Param(name) => name.clone(),
        Ty::Raw { template, args } => emit_raw(template, args, cfg),
        Ty::Unknown => match cfg.unknown_style() {
            UnknownStyle::Unknown => "unknown".to_string(),
            UnknownStyle::Any => "any".to_string(),
        },
    }
}

fn int_style(b: BigInt) -> String {
    match b {
        BigInt::Number => "number".into(),
        BigInt::Bigint => "bigint".into(),
        BigInt::Str => "string".into(),
    }
}

/// The `readonly ` prefix for a field, or the empty string.
fn ro(f: &Field) -> &'static str {
    if f.readonly { "readonly " } else { "" }
}

fn needs_parens(ty: &Ty) -> bool {
    matches!(ty, Ty::Union(arms) if arms.len() > 1)
}

/// Renders a [`Ty::Raw`] template, substituting `{0}`, `{1}`, ... with the
/// rendered argument at that position. A template with no placeholder emits
/// verbatim, ignoring `args` entirely -- the terminal case, e.g. mapping
/// `chrono::DateTime` to `"string"`. An out-of-range index is left as literal
/// text rather than panicking; lowering already warns about a mismatch like
/// that when it can tell one occurred.
fn emit_raw(template: &str, args: &[Ty], cfg: &Config) -> String {
    let positions = placeholder_positions(template);
    if positions.is_empty() {
        return template.to_string();
    }
    let mut out = String::with_capacity(template.len());
    let mut last = 0;
    for (start, end, idx) in positions {
        out.push_str(&template[last..start]);
        match args.get(idx) {
            Some(arg) => out.push_str(&emit_ty(arg, cfg)),
            None => out.push_str(&template[start..end]),
        }
        last = end;
    }
    out.push_str(&template[last..]);
    out
}

fn emit_prim(p: &Prim, cfg: &Config) -> String {
    match p {
        Prim::Str => "string".into(),
        Prim::Bool => "boolean".into(),
        Prim::Float => "number".into(),
        Prim::Null => "null".into(),
        Prim::Int { bits, .. } if *bits >= 128 => int_style(cfg.int128_style()),
        Prim::Int { bits, .. } if *bits >= 64 => int_style(cfg.int64_style()),
        Prim::Int { .. } => "number".into(),
    }
}

/// A single-line object, used for enum variant payloads.
pub(crate) fn emit_object_inline(fields: &[Field], cfg: &Config) -> String {
    if fields.is_empty() {
        return "Record<string, never>".into();
    }
    let parts: Vec<String> = fields
        .iter()
        .map(|f| {
            let (q, ty) = optionality(f, cfg);
            format!("{}{}{q}: {ty}", ro(f), f.ts_name)
        })
        .collect();
    format!("{{ {} }}", parts.join("; "))
}

/// Returns the `?` marker and the rendered type, applying `OptionalStyle`.
pub(crate) fn optionality(f: &Field, cfg: &Config) -> (&'static str, String) {
    if !f.optional {
        return ("", emit_ty(&f.ty, cfg));
    }
    match cfg.optional() {
        OptionalStyle::Nullable | OptionalStyle::Both => ("?", emit_ty(&f.ty, cfg)),
        OptionalStyle::Question => ("?", emit_ty(&strip_null(&f.ty), cfg)),
    }
}

fn strip_null(ty: &Ty) -> Ty {
    match ty {
        Ty::Union(arms) => {
            let kept: Vec<Ty> = arms
                .iter()
                .filter(|a| **a != Ty::Prim(Prim::Null))
                .cloned()
                .collect();
            match kept.len() {
                0 => Ty::Prim(Prim::Null),
                1 => kept.into_iter().next().unwrap(),
                _ => Ty::Union(kept),
            }
        }
        other => other.clone(),
    }
}

/// Renders doc comment lines as a `/** ... */` TSDoc block, indented by
/// `indent`. Returns an empty string when `docs` is empty.
pub fn tsdoc(docs: &[String], indent: &str) -> String {
    if docs.is_empty() {
        return String::new();
    }
    let mut out = format!("{indent}/**\n");
    for line in docs {
        out.push_str(&format!("{indent} * {line}\n"));
    }
    out.push_str(&format!("{indent} */\n"));
    out
}

fn emit_fields_block(fields: &[&Field], cfg: &Config) -> String {
    let mut out = String::from("{\n");
    for f in fields {
        out.push_str(&tsdoc(&f.docs, INDENT));
        let (q, ty) = optionality(f, cfg);
        out.push_str(&format!("{INDENT}{}{}{q}: {ty};\n", ro(f), f.ts_name));
    }
    out.push('}');
    out
}

fn generics_head(decl: &Decl) -> String {
    if decl.generics.is_empty() {
        String::new()
    } else {
        format!("<{}>", decl.generics.join(", "))
    }
}

/// Renders one declaration as a complete TypeScript statement — an
/// `interface`, `type` alias, or tagged-union `type`, per its [`Body`].
pub fn emit_decl(decl: &Decl, cfg: &Config) -> String {
    let head = tsdoc(&decl.docs, "");
    let name = &decl.ts_name;
    let params = generics_head(decl);

    let body = match &decl.body {
        Body::Struct(fields) => {
            let (flat, plain): (Vec<&Field>, Vec<&Field>) = fields.iter().partition(|f| f.flatten);
            let block = emit_fields_block(&plain, cfg);
            if flat.is_empty() {
                return match cfg.structs() {
                    StructStyle::Interface => {
                        format!("{head}export interface {name}{params} {block}\n")
                    }
                    StructStyle::TypeAlias => {
                        format!("{head}export type {name}{params} = {block};\n")
                    }
                };
            }
            // Flattening produces an intersection, which an interface cannot express.
            let mut parts: Vec<String> = flat.iter().map(|f| emit_ty(&f.ty, cfg)).collect();
            parts.push(block);
            parts.join(" & ")
        }
        Body::Newtype(ty) => emit_ty(ty, cfg),
        Body::Tuple(items) => emit_ty(&Ty::Tuple(items.clone()), cfg),
        Body::Unit => "null".to_string(),
        Body::Enum { .. } => emit_enum(decl, cfg),
    };

    // A multi-arm union body starts with its own newline; emitting "= " there
    // would leave a trailing space at the end of the line.
    let sep = if body.starts_with('\n') { "=" } else { "= " };
    format!("{head}export type {name}{params} {sep}{body};\n")
}

fn emit_enum(decl: &Decl, cfg: &Config) -> String {
    let Body::Enum { repr, variants } = &decl.body else {
        return "never".to_string();
    };

    // The compact single-line string union only works when no variant has a
    // doc comment to put somewhere; a doc comment needs its own line.
    if let EnumRepr::UnitOnly = repr {
        if variants.iter().all(|v| v.docs.is_empty()) {
            let arms: Vec<String> = variants.iter().map(|v| quoted(&v.ts_name)).collect();
            return arms.join(" | ");
        }
    }

    let arms: Vec<(String, String)> =
        variants.iter().map(|v| (tsdoc(&v.docs, INDENT), emit_variant(v, repr, cfg))).collect();
    match arms.len() {
        0 => "never".to_string(),
        // A lone arm sits directly after `=`, where a leading doc comment
        // would collide with that formatting; the type's own doc, emitted by
        // the caller, already covers a single-variant enum.
        1 => arms.into_iter().next().unwrap().1,
        _ => {
            let mut out = String::new();
            for (doc, arm) in &arms {
                out.push('\n');
                out.push_str(doc);
                out.push_str(&format!("{INDENT}| {arm}"));
            }
            out
        }
    }
}

fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn emit_variant(v: &Variant, repr: &EnumRepr, cfg: &Config) -> String {
    let tag = quoted(&v.ts_name);
    // `#[serde(untagged)]` on a single variant wins over the enum's own
    // representation: serde writes the bare payload with no discriminant.
    let repr = if v.untagged { &EnumRepr::Untagged } else { repr };
    match repr {
        EnumRepr::UnitOnly => tag,
        EnumRepr::External => match &v.kind {
            VariantKind::Unit => tag,
            VariantKind::Newtype(t) => format!("{{ {}: {} }}", v.ts_name, emit_ty(t, cfg)),
            VariantKind::Tuple(items) => {
                format!("{{ {}: {} }}", v.ts_name, emit_ty(&Ty::Tuple(items.clone()), cfg))
            }
            VariantKind::Struct(fields) => {
                format!("{{ {}: {} }}", v.ts_name, emit_object_inline(fields, cfg))
            }
        },
        EnumRepr::Internal { tag: key } => match &v.kind {
            VariantKind::Unit => format!("{{ {key}: {tag} }}"),
            VariantKind::Struct(fields) => {
                let mut parts = vec![format!("{key}: {tag}")];
                for f in fields {
                    let (q, ty) = optionality(f, cfg);
                    parts.push(format!("{}{q}: {ty}", f.ts_name));
                }
                format!("{{ {} }}", parts.join("; "))
            }
            // Diagnosed during parsing; render the tag alone rather than a lie.
            VariantKind::Newtype(_) | VariantKind::Tuple(_) => format!("{{ {key}: {tag} }}"),
        },
        EnumRepr::Adjacent { tag: key, content } => {
            let payload = match &v.kind {
                VariantKind::Unit => None,
                VariantKind::Newtype(t) => Some(emit_ty(t, cfg)),
                VariantKind::Tuple(items) => Some(emit_ty(&Ty::Tuple(items.clone()), cfg)),
                VariantKind::Struct(fields) => Some(emit_object_inline(fields, cfg)),
            };
            match payload {
                None => format!("{{ {key}: {tag} }}"),
                Some(p) => format!("{{ {key}: {tag}; {content}: {p} }}"),
            }
        }
        EnumRepr::Untagged => match &v.kind {
            VariantKind::Unit => "null".to_string(),
            VariantKind::Newtype(t) => emit_ty(t, cfg),
            VariantKind::Tuple(items) => emit_ty(&Ty::Tuple(items.clone()), cfg),
            VariantKind::Struct(fields) => emit_object_inline(fields, cfg),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{BigInt, Config, OptionalStyle, StructStyle};
    use crate::ir::{Body, Decl, Field, Prim, SourceLoc, Ty};

    fn field(name: &str, ty: Ty) -> Field {
        Field {
            rust_name: name.into(),
            ts_name: name.into(),
            ty,
            optional: false,
            flatten: false,
            readonly: false,
            inline: false,
            docs: vec![],
        }
    }

    fn decl(body: Body) -> Decl {
        Decl {
            rust_path: "crate::X".into(),
            ts_name: "X".into(),
            docs: vec![],
            generics: vec![],
            body,
            loc: SourceLoc {
                file: "x.rs".into(),
                line: 1,
            },
        }
    }

    #[test]
    fn primitives_render() {
        let c = Config::new();
        assert_eq!(emit_ty(&Ty::Prim(Prim::Str), &c), "string");
        assert_eq!(emit_ty(&Ty::Prim(Prim::Bool), &c), "boolean");
        assert_eq!(emit_ty(&Ty::Prim(Prim::Float), &c), "number");
        assert_eq!(emit_ty(&Ty::Prim(Prim::Null), &c), "null");
        assert_eq!(emit_ty(&Ty::Unknown, &c), "unknown");
        assert_eq!(
            emit_ty(&Ty::Raw { template: "Foo<Bar>".into(), args: vec![] }, &c),
            "Foo<Bar>"
        );
    }

    #[test]
    fn raw_with_no_placeholder_ignores_its_arguments() {
        // The terminal case: a config mapping that drops its generic argument,
        // e.g. chrono::DateTime -> "string" regardless of DateTime<Utc>.
        let c = Config::new();
        let ty = Ty::Raw { template: "string".into(), args: vec![Ty::Prim(Prim::Bool)] };
        assert_eq!(emit_ty(&ty, &c), "string");
    }

    #[test]
    fn raw_substitutes_a_single_placeholder() {
        let c = Config::new();
        let ty = Ty::Raw {
            template: "TsWrapper<{0}>".into(),
            args: vec![Ty::Prim(Prim::Str)],
        };
        assert_eq!(emit_ty(&ty, &c), "TsWrapper<string>");
    }

    #[test]
    fn raw_substitutes_a_placeholder_placed_anywhere_in_the_template() {
        // The "shaped alias" case: the argument isn't wrapped in a name at all.
        let c = Config::new();
        let ty = Ty::Raw { template: "{0} | null".into(), args: vec![Ty::Prim(Prim::Bool)] };
        assert_eq!(emit_ty(&ty, &c), "boolean | null");
    }

    #[test]
    fn raw_substitutes_multiple_placeholders_by_index() {
        let c = Config::new();
        let ty = Ty::Raw {
            template: "[{1}, {0}]".into(),
            args: vec![Ty::Prim(Prim::Bool), Ty::Prim(Prim::Str)],
        };
        assert_eq!(emit_ty(&ty, &c), "[string, boolean]");
    }

    #[test]
    fn raw_leaves_an_out_of_range_placeholder_literal() {
        let c = Config::new();
        let ty = Ty::Raw { template: "{0}<{1}>".into(), args: vec![Ty::Prim(Prim::Str)] };
        assert_eq!(emit_ty(&ty, &c), "string<{1}>");
    }

    #[test]
    fn bigint_policy_applies_only_to_wide_integers() {
        let narrow = Ty::Prim(Prim::Int {
            bits: 32,
            signed: true,
        });
        let wide = Ty::Prim(Prim::Int {
            bits: 64,
            signed: false,
        });
        assert_eq!(
            emit_ty(&narrow, &Config::new().bigint(BigInt::Bigint)),
            "number"
        );
        assert_eq!(emit_ty(&wide, &Config::new()), "number");
        assert_eq!(
            emit_ty(&wide, &Config::new().bigint(BigInt::Bigint)),
            "bigint"
        );
        assert_eq!(emit_ty(&wide, &Config::new().bigint(BigInt::Str)), "string");
    }

    #[test]
    fn containers_render() {
        let c = Config::new();
        assert_eq!(
            emit_ty(&Ty::Array(Box::new(Ty::Prim(Prim::Str))), &c),
            "string[]"
        );
        assert_eq!(
            emit_ty(
                &Ty::Tuple(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Bool)]),
                &c
            ),
            "[string, boolean]"
        );
        assert_eq!(
            emit_ty(
                &Ty::Record {
                    key: Box::new(Ty::Prim(Prim::Str)),
                    value: Box::new(Ty::Prim(Prim::Bool))
                },
                &c
            ),
            "Record<string, boolean>"
        );
        assert_eq!(
            emit_ty(
                &Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Null)]),
                &c
            ),
            "string | null"
        );
    }

    #[test]
    fn array_of_a_union_is_parenthesised() {
        let inner = Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Null)]);
        assert_eq!(
            emit_ty(&Ty::Array(Box::new(inner)), &Config::new()),
            "(string | null)[]"
        );
    }

    #[test]
    fn refs_render_with_generic_arguments() {
        let c = Config::new();
        assert_eq!(
            emit_ty(
                &Ty::Ref {
                    name: "A".into(),
                    args: vec![]
                },
                &c
            ),
            "A"
        );
        assert_eq!(
            emit_ty(
                &Ty::Ref {
                    name: "Page".into(),
                    args: vec![Ty::Prim(Prim::Str)]
                },
                &c
            ),
            "Page<string>"
        );
        assert_eq!(emit_ty(&Ty::Param("T".into()), &c), "T");
    }

    #[test]
    fn named_struct_emits_an_interface_by_default() {
        let d = decl(Body::Struct(vec![field("id", Ty::Prim(Prim::Str))]));
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export interface X {\n  id: string;\n}\n"
        );
    }

    #[test]
    fn struct_style_type_alias_switches_the_form() {
        let d = decl(Body::Struct(vec![field("id", Ty::Prim(Prim::Str))]));
        let c = Config::new().struct_style(StructStyle::TypeAlias);
        assert_eq!(emit_decl(&d, &c), "export type X = {\n  id: string;\n};\n");
    }

    #[test]
    fn optional_style_controls_question_marks_and_null() {
        let mut f = field(
            "v",
            Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Null)]),
        );
        f.optional = true;
        let d = decl(Body::Struct(vec![f]));
        assert!(emit_decl(&d, &Config::new()).contains("v?: string | null;"));
        let q = Config::new().optional_style(OptionalStyle::Question);
        assert!(emit_decl(&d, &q).contains("v?: string;"));
    }

    #[test]
    fn non_optional_nullable_field_keeps_its_key() {
        let f = field(
            "v",
            Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Null)]),
        );
        let d = decl(Body::Struct(vec![f]));
        assert!(emit_decl(&d, &Config::new()).contains("v: string | null;"));
    }

    #[test]
    fn newtype_tuple_and_unit_bodies_are_always_aliases() {
        let c = Config::new();
        assert_eq!(
            emit_decl(&decl(Body::Newtype(Ty::Prim(Prim::Str))), &c),
            "export type X = string;\n"
        );
        assert_eq!(
            emit_decl(
                &decl(Body::Tuple(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Bool)])),
                &c
            ),
            "export type X = [string, boolean];\n"
        );
        assert_eq!(emit_decl(&decl(Body::Unit), &c), "export type X = null;\n");
    }

    #[test]
    fn generics_appear_in_the_declaration_head() {
        let mut d = decl(Body::Struct(vec![field("v", Ty::Param("T".into()))]));
        d.generics = vec!["T".into()];
        assert!(emit_decl(&d, &Config::new()).starts_with("export interface X<T> {"));
    }

    #[test]
    fn doc_comments_become_tsdoc() {
        let mut d = decl(Body::Struct(vec![{
            let mut f = field("v", Ty::Prim(Prim::Str));
            f.docs = vec!["Field doc.".into()];
            f
        }]));
        d.docs = vec!["Type doc.".into()];
        let out = emit_decl(&d, &Config::new());
        assert!(out.starts_with("/**\n * Type doc.\n */\n"));
        assert!(out.contains("  /**\n   * Field doc.\n   */\n  v: string;"));
    }

    #[test]
    fn flattened_fields_become_an_intersection() {
        let mut f = field(
            "base",
            Ty::Ref {
                name: "Base".into(),
                args: vec![],
            },
        );
        f.flatten = true;
        let d = decl(Body::Struct(vec![f, field("v", Ty::Prim(Prim::Str))]));
        let out = emit_decl(&d, &Config::new());
        assert_eq!(out, "export type X = Base & {\n  v: string;\n};\n");
    }
}

#[cfg(test)]
mod enum_tests {
    use super::*;
    use crate::config::Config;
    use crate::ir::{Body, Decl, EnumRepr, Field, Prim, SourceLoc, Ty, Variant, VariantKind};

    fn field(name: &str, ty: Ty) -> Field {
        Field {
            rust_name: name.into(),
            ts_name: name.into(),
            ty,
            optional: false,
            flatten: false,
            readonly: false,
            inline: false,
            docs: vec![],
        }
    }

    fn variant(name: &str, kind: VariantKind) -> Variant {
        Variant { rust_name: name.into(), ts_name: name.into(), docs: vec![], kind, untagged: false }
    }

    fn enum_decl(repr: EnumRepr, variants: Vec<Variant>) -> Decl {
        Decl {
            rust_path: "crate::X".into(),
            ts_name: "X".into(),
            docs: vec![],
            generics: vec![],
            body: Body::Enum { repr, variants },
            loc: SourceLoc { file: "x.rs".into(), line: 1 },
        }
    }

    fn sample_variants() -> Vec<Variant> {
        vec![
            variant("A", VariantKind::Unit),
            variant(
                "B",
                VariantKind::Struct(vec![field("x", Ty::Prim(Prim::Int { bits: 32, signed: true }))]),
            ),
            variant("C", VariantKind::Newtype(Ty::Prim(Prim::Str))),
        ]
    }

    #[test]
    fn externally_tagged_enum() {
        let d = enum_decl(EnumRepr::External, sample_variants());
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export type X =\n  | \"A\"\n  | { B: { x: number } }\n  | { C: string };\n"
        );
    }

    #[test]
    fn all_unit_enum_is_a_string_union() {
        let d = enum_decl(
            EnumRepr::UnitOnly,
            vec![variant("Active", VariantKind::Unit), variant("Banned", VariantKind::Unit)],
        );
        assert_eq!(emit_decl(&d, &Config::new()), "export type X = \"Active\" | \"Banned\";\n");
    }

    #[test]
    fn internally_tagged_enum_inlines_struct_fields() {
        let d = enum_decl(
            EnumRepr::Internal { tag: "t".into() },
            vec![
                variant("A", VariantKind::Unit),
                variant("B", VariantKind::Struct(vec![field("x", Ty::Prim(Prim::Bool))])),
            ],
        );
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export type X =\n  | { t: \"A\" }\n  | { t: \"B\"; x: boolean };\n"
        );
    }

    #[test]
    fn adjacently_tagged_enum_nests_the_payload() {
        let d =
            enum_decl(EnumRepr::Adjacent { tag: "t".into(), content: "c".into() }, sample_variants());
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export type X =\n  | { t: \"A\" }\n  | { t: \"B\"; c: { x: number } }\n  | { t: \"C\"; c: string };\n"
        );
    }

    #[test]
    fn untagged_enum_has_no_discriminant() {
        let d = enum_decl(EnumRepr::Untagged, sample_variants());
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export type X =\n  | null\n  | { x: number }\n  | string;\n"
        );
    }

    #[test]
    fn tuple_variant_payload_is_an_array() {
        let d = enum_decl(
            EnumRepr::External,
            vec![variant("T", VariantKind::Tuple(vec![Ty::Prim(Prim::Bool), Ty::Prim(Prim::Str)]))],
        );
        assert_eq!(emit_decl(&d, &Config::new()), "export type X = { T: [boolean, string] };\n");
    }

    #[test]
    fn variant_doc_comments_are_emitted_above_their_arm() {
        let mut a = variant("A", VariantKind::Unit);
        a.docs = vec!["Doc for A.".into()];
        let d = enum_decl(EnumRepr::External, vec![a, variant("B", VariantKind::Unit)]);
        assert_eq!(
            emit_decl(&d, &Config::new()),
            "export type X =\n  /**\n   * Doc for A.\n   */\n  | \"A\"\n  | \"B\";\n"
        );
    }

    #[test]
    fn all_unit_enum_with_a_doc_falls_back_to_the_multiline_form() {
        let mut active = variant("Active", VariantKind::Unit);
        active.docs = vec!["Can log in.".into()];
        let d = enum_decl(EnumRepr::UnitOnly, vec![active, variant("Banned", VariantKind::Unit)]);
        let out = emit_decl(&d, &Config::new());
        assert!(out.contains("/**\n   * Can log in.\n   */\n  | \"Active\""), "{out}");
        assert!(out.contains("| \"Banned\""), "{out}");
    }

    #[test]
    fn a_single_variant_enum_drops_its_variant_doc_without_corrupting_formatting() {
        // A lone arm has nowhere clean to put its own doc comment; the type's
        // own doc, from #[derive(Ts)] on the enum itself, covers this case.
        let mut only = variant("Only", VariantKind::Unit);
        only.docs = vec!["Ignored here.".into()];
        let d = enum_decl(EnumRepr::External, vec![only]);
        assert_eq!(emit_decl(&d, &Config::new()), "export type X = \"Only\";\n");
    }
}

#[cfg(test)]
mod readonly_tests {
    use super::*;
    use crate::config::Config;
    use crate::ir::{Body, Decl, Field, Prim, SourceLoc, Ty};

    fn ro_field(name: &str, readonly: bool) -> Field {
        Field {
            rust_name: name.into(),
            ts_name: name.into(),
            ty: Ty::Prim(Prim::Str),
            optional: false,
            flatten: false,
            readonly,
            inline: false,
            docs: vec![],
        }
    }

    fn decl(body: Body) -> Decl {
        Decl {
            rust_path: "crate::X".into(),
            ts_name: "X".into(),
            docs: vec![],
            generics: vec![],
            body,
            loc: SourceLoc { file: "x.rs".into(), line: 1 },
        }
    }

    #[test]
    fn readonly_fields_are_marked_and_others_are_not() {
        let d = decl(Body::Struct(vec![ro_field("a", true), ro_field("b", false)]));
        let out = emit_decl(&d, &Config::new());
        assert!(out.contains("  readonly a: string;"), "got: {out}");
        assert!(out.contains("  b: string;") && !out.contains("readonly b"), "got: {out}");
    }

    #[test]
    fn readonly_survives_into_inline_objects() {
        // Enum struct variants render inline; readonly must not be dropped there.
        let inline = emit_object_inline(&[ro_field("a", true)], &Config::new());
        assert_eq!(inline, "{ readonly a: string }");
    }
}
