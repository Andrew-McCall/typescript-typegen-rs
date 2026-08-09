//! Rendering IR into TypeScript source text.
//!
//! This module knows nothing about Rust. All policy that depends on `Config` —
//! integer width, optionality style, interface vs alias — lives here.

use crate::config::{BigInt, Bytes, Config, OptionalStyle, StructStyle};
use crate::ir::{Body, Decl, EnumRepr, Field, Prim, Ty, Variant, VariantKind};

const INDENT: &str = "  ";

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
        Ty::Raw(raw) => raw.clone(),
        Ty::Unknown => "unknown".to_string(),
    }
}

/// The `readonly ` prefix for a field, or the empty string.
fn ro(f: &Field) -> &'static str {
    if f.readonly { "readonly " } else { "" }
}

fn needs_parens(ty: &Ty) -> bool {
    matches!(ty, Ty::Union(arms) if arms.len() > 1)
}

fn emit_prim(p: &Prim, cfg: &Config) -> String {
    match p {
        Prim::Str => "string".into(),
        Prim::Bool => "boolean".into(),
        Prim::Float => "number".into(),
        Prim::Null => "null".into(),
        Prim::Int { bits, .. } if *bits >= 64 => match cfg.bigint_style() {
            BigInt::Number => "number".into(),
            BigInt::Bigint => "bigint".into(),
            BigInt::Str => "string".into(),
        },
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

    if let EnumRepr::UnitOnly = repr {
        let arms: Vec<String> = variants.iter().map(|v| quoted(&v.ts_name)).collect();
        return arms.join(" | ");
    }

    let arms: Vec<String> = variants.iter().map(|v| emit_variant(v, repr, cfg)).collect();
    match arms.len() {
        0 => "never".to_string(),
        1 => arms.into_iter().next().unwrap(),
        _ => format!("\n{INDENT}| {}", arms.join(&format!("\n{INDENT}| "))),
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
        assert_eq!(emit_ty(&Ty::Raw("Foo<Bar>".into()), &c), "Foo<Bar>");
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
