//! Turning `syn` items into IR declarations.

use syn::spanned::Spanned;

use std::collections::HashMap;
use std::path::Path;

use crate::attrs::{Attrs, RenameRule, doc_lines, parse_attrs};
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Body, Decl, EnumRepr, Field, SourceLoc, Ty, Variant, VariantKind};
use crate::lower::{LowerCtx, lower_type};

pub fn has_ts_derive(attrs: &[syn::Attribute]) -> bool {
    let mut found = false;
    for attr in attrs.iter().filter(|a| a.path().is_ident("derive")) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.segments.last().is_some_and(|s| s.ident == "Ts") {
                found = true;
            }
            Ok(())
        });
    }
    found
}

/// Merges fields that landed on the same TypeScript name into one field whose
/// type is the union of theirs.
///
/// Two Rust fields can reach the same JSON key: `type_` and `r#type` both
/// camelCase to `type`. serde accepts that and emits duplicate keys, but a
/// TypeScript object type with a repeated key is a `tsc` error, so the binding
/// has to collapse them. A union is the honest collapse — the key holds one of
/// those types, and which one depends on the consumer's parser, so claiming
/// either alone would be a guess. Rare enough to warn about rather than reject.
fn merge_field_collisions(
    fields: Vec<Field>,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<Field> {
    let mut order: Vec<String> = Vec::with_capacity(fields.len());
    let mut groups: HashMap<String, Vec<Field>> = HashMap::with_capacity(fields.len());
    for f in fields {
        if !groups.contains_key(&f.ts_name) {
            order.push(f.ts_name.clone());
        }
        groups.entry(f.ts_name.clone()).or_default().push(f);
    }

    order
        .into_iter()
        .map(|name| {
            let mut group = groups.remove(&name).expect("name came from the group keys");
            if group.len() == 1 {
                return group.pop().expect("length checked");
            }

            let rust_names: Vec<String> =
                group.iter().map(|f| format!("`{}`", f.rust_name)).collect();
            let (listed, quantifier) = match rust_names.as_slice() {
                [a, b] => (format!("{a} and {b}"), "both"),
                rest => (rest.join(", "), "all"),
            };
            diags.push(
                Diagnostic::warning(
                    loc.clone(),
                    format!("fields {listed} {quantifier} map to the TypeScript name `{name}`"),
                )
                .with_help(
                    "the binding uses a union of their types; rename one with \
                     #[ts(rename = \"...\")] to be explicit",
                ),
            );

            // Distinct types only: two fields of the same type collapse to that
            // type rather than to a pointless `T | T`.
            let mut tys: Vec<Ty> = Vec::with_capacity(group.len());
            for f in &group {
                if !tys.contains(&f.ty) {
                    tys.push(f.ty.clone());
                }
            }
            let ty = if tys.len() == 1 { tys.remove(0) } else { Ty::Union(tys) };

            let first = group.first().expect("non-empty");
            Field {
                rust_name: first.rust_name.clone(),
                ts_name: name,
                ty,
                // The key is absent only if every contributor omits it.
                optional: group.iter().all(|f| f.optional),
                flatten: group.iter().any(|f| f.flatten),
                readonly: group.iter().any(|f| f.readonly),
                inline: group.iter().any(|f| f.inline),
                docs: first.docs.clone(),
            }
        })
        .collect()
}

/// The same clash, one level up: two variants sharing a discriminant value make
/// the union arms indistinguishable.
fn check_variant_collisions(variants: &[Variant], loc: &SourceLoc, diags: &mut Diagnostics) {
    let mut seen: HashMap<&str, &str> = HashMap::with_capacity(variants.len());
    for v in variants {
        if let Some(previous) = seen.insert(&v.ts_name, &v.rust_name) {
            diags.push(
                Diagnostic::warning(
                    loc.clone(),
                    format!(
                        "variants `{previous}` and `{}` both map to the TypeScript name `{}`",
                        v.rust_name, v.ts_name
                    ),
                )
                .with_help("rename one with #[ts(rename = \"...\")]"),
            );
        }
    }
}

/// Strips the `r#` from a raw identifier.
///
/// serde unraws before applying rename rules, so a field written `r#type`
/// serialises as `type`. syn hands us the prefix intact, so dropping it here
/// keeps every downstream name (and the rename rules) matching serde.
fn unraw(ident: &syn::Ident) -> String {
    let s = ident.to_string();
    s.strip_prefix("r#").map(str::to_owned).unwrap_or(s)
}

fn loc_of(file: &Path, span: proc_macro2::Span) -> SourceLoc {
    SourceLoc {
        file: file.to_path_buf(),
        line: span.start().line,
    }
}

pub fn parse_item(
    item: &syn::DeriveInput,
    module_path: &str,
    file: &Path,
    base: &LowerCtx,
    diags: &mut Diagnostics,
) -> Option<Decl> {
    let loc = loc_of(file, item.ident.span());
    let attrs = parse_attrs(&item.attrs);
    if attrs.skip {
        return None;
    }

    let mut ctx = base.clone();
    for p in &item.generics.params {
        match p {
            syn::GenericParam::Type(t) => {
                ctx.params.insert(t.ident.to_string());
            }
            syn::GenericParam::Const(c) => {
                diags.push(
                    Diagnostic::new(
                        loc.clone(),
                        format!("const generic `{}` is not supported", c.ident),
                    )
                    .with_help("const generics cannot be resolved without type evaluation"),
                );
            }
            syn::GenericParam::Lifetime(_) => {}
        }
    }
    // Declaration order, not the HashSet's iteration order, drives the emitted
    // generic list — `Page<K, V>` must not come out as `Page<V, K>`.
    let generics: Vec<String> = item
        .generics
        .params
        .iter()
        .filter_map(|p| match p {
            syn::GenericParam::Type(t) => Some(t.ident.to_string()),
            _ => None,
        })
        .collect();

    let ts_name = attrs
        .rename
        .clone()
        .unwrap_or_else(|| unraw(&item.ident));
    check_custom_serialisation(item, file, diags);

    let body = match &item.data {
        syn::Data::Struct(s) => struct_body(s, &attrs, &ctx, &loc, diags),
        syn::Data::Enum(e) => enum_body(e, &attrs, &ctx, file, diags),
        syn::Data::Union(_) => {
            diags.push(Diagnostic::new(loc.clone(), "unions are not supported"));
            return None;
        }
    };

    Some(Decl {
        rust_path: format!("{module_path}::{}", item.ident),
        ts_name,
        docs: doc_lines(&item.attrs),
        generics,
        body,
        loc,
    })
}

/// `#[serde(transparent)]` serialises as the struct's single field, so the type
/// is that field's type — not an object wrapping it.
fn transparent_body(
    s: &syn::DataStruct,
    attrs: &Attrs,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Body {
    let kept: Vec<&syn::Field> =
        s.fields.iter().filter(|f| !parse_attrs(&f.attrs).skip).collect();
    match kept.as_slice() {
        [f] => Body::Newtype(field_ty(f, &parse_attrs(&f.attrs), ctx)),
        _ => {
            diags.push(
                Diagnostic::new(
                    loc.clone(),
                    format!(
                        "#[serde(transparent)] needs exactly one non-skipped field, found {}",
                        kept.len()
                    ),
                )
                .with_help("serde rejects this too; remove transparent or skip the extra fields"),
            );
            // Fall back to the ordinary shape rather than inventing one.
            match &s.fields {
                syn::Fields::Named(n) => {
                    Body::Struct(named_fields(n, attrs.rename_all, ctx, attrs.readonly))
                }
                syn::Fields::Unnamed(u) => Body::Tuple(unnamed_types(u, ctx)),
                syn::Fields::Unit => Body::Unit,
            }
        }
    }
}

/// `#[serde(with)]` / `#[serde(serialize_with)]` hand the wire format to code we
/// cannot inspect, so the Rust type stops describing the JSON. Emitting the
/// Rust-derived type anyway would be a silent lie; require an explicit override.
fn check_custom_serialisation(item: &syn::DeriveInput, file: &Path, diags: &mut Diagnostics) {
    let mut check = |f: &syn::Field| {
        let fa = parse_attrs(&f.attrs);
        if fa.custom_ser && fa.ts_type.is_none() && !fa.skip {
            let name = f.ident.as_ref().map(|i| i.to_string()).unwrap_or_else(|| "_".into());
            diags.push(
                Diagnostic::new(
                    loc_of(file, f.ty.span()),
                    format!(
                        "field `{name}` uses serde `with`/`serialize_with`, so its \
                         wire type cannot be derived from its Rust type"
                    ),
                )
                .with_help("give the shape explicitly with #[ts(type = \"...\")]"),
            );
        }
    };
    match &item.data {
        syn::Data::Struct(s) => s.fields.iter().for_each(&mut check),
        syn::Data::Enum(e) => e.variants.iter().flat_map(|v| v.fields.iter()).for_each(&mut check),
        syn::Data::Union(_) => {}
    }
}

fn field_ty(f: &syn::Field, fa: &Attrs, ctx: &LowerCtx) -> Ty {
    match &fa.ts_type {
        Some(raw) => Ty::Raw(raw.clone()),
        None => lower_type(&f.ty, ctx),
    }
}

fn named_fields(
    fields: &syn::FieldsNamed,
    rule: Option<RenameRule>,
    ctx: &LowerCtx,
    inherited_readonly: bool,
) -> Vec<Field> {
    fields
        .named
        .iter()
        .filter_map(|f| {
            let fa = parse_attrs(&f.attrs);
            if fa.skip {
                return None;
            }
            let rust_name = unraw(f.ident.as_ref()?);
            let ts_name = fa.rename.clone().unwrap_or_else(|| match rule {
                Some(r) => r.apply_to_field(&rust_name),
                None => rust_name.clone(),
            });
            Some(Field {
                rust_name,
                ts_name,
                ty: field_ty(f, &fa, ctx),
                optional: fa.optional,
                flatten: fa.flatten,
                readonly: fa.readonly || inherited_readonly,
                inline: fa.inline,
                docs: doc_lines(&f.attrs),
            })
        })
        .collect()
}

fn unnamed_types(fields: &syn::FieldsUnnamed, ctx: &LowerCtx) -> Vec<Ty> {
    fields
        .unnamed
        .iter()
        .filter_map(|f| {
            let fa = parse_attrs(&f.attrs);
            if fa.skip {
                return None;
            }
            Some(field_ty(f, &fa, ctx))
        })
        .collect()
}

fn struct_body(
    s: &syn::DataStruct,
    attrs: &Attrs,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Body {
    if attrs.transparent {
        return transparent_body(s, attrs, ctx, loc, diags);
    }
    match &s.fields {
        syn::Fields::Named(n) => {
            let fields = named_fields(n, attrs.rename_all, ctx, attrs.readonly);
            Body::Struct(merge_field_collisions(fields, loc, diags))
        }
        syn::Fields::Unnamed(u) => {
            let mut tys = unnamed_types(u, ctx);
            if tys.len() == 1 {
                Body::Newtype(tys.remove(0))
            } else {
                Body::Tuple(tys)
            }
        }
        syn::Fields::Unit => Body::Unit,
    }
}

fn enum_body(
    e: &syn::DataEnum,
    attrs: &Attrs,
    ctx: &LowerCtx,
    file: &Path,
    diags: &mut Diagnostics,
) -> Body {
    let variants: Vec<Variant> = e
        .variants
        .iter()
        .filter_map(|v| {
            let va = parse_attrs(&v.attrs);
            if va.skip {
                return None;
            }
            let rust_name = unraw(&v.ident);
            let ts_name = va.rename.clone().unwrap_or_else(|| match attrs.rename_all {
                Some(r) => r.apply_to_variant(&rust_name),
                None => rust_name.clone(),
            });
            let kind = match &v.fields {
                syn::Fields::Unit => VariantKind::Unit,
                syn::Fields::Named(n) => {
                    // A variant-level rename_all overrides the container default.
                    let rule = va.rename_all.or(attrs.rename_all_fields);
                    let fields = named_fields(n, rule, ctx, attrs.readonly);
                    let loc = loc_of(file, v.ident.span());
                    VariantKind::Struct(merge_field_collisions(fields, &loc, diags))
                }
                syn::Fields::Unnamed(u) => {
                    let mut tys = unnamed_types(u, ctx);
                    if tys.len() == 1 {
                        VariantKind::Newtype(tys.remove(0))
                    } else {
                        VariantKind::Tuple(tys)
                    }
                }
            };
            Some(Variant {
                rust_name,
                ts_name,
                docs: doc_lines(&v.attrs),
                kind,
            })
        })
        .collect();

    check_variant_collisions(&variants, &loc_of(file, e.variants.first().map(|v| v.ident.span()).unwrap_or_else(proc_macro2::Span::call_site)), diags);

    let repr = if attrs.untagged {
        EnumRepr::Untagged
    } else {
        match (&attrs.tag, &attrs.content) {
            (Some(t), Some(c)) => EnumRepr::Adjacent {
                tag: t.clone(),
                content: c.clone(),
            },
            (Some(t), None) => EnumRepr::Internal { tag: t.clone() },
            _ if variants.iter().all(|v| matches!(v.kind, VariantKind::Unit)) => EnumRepr::UnitOnly,
            _ => EnumRepr::External,
        }
    };

    if let EnumRepr::Internal { .. } = &repr {
        for (v, syn_v) in variants.iter().zip(e.variants.iter()) {
            let bad = match &v.kind {
                VariantKind::Tuple(_) => true,
                VariantKind::Newtype(t) => {
                    !matches!(t, Ty::Ref { .. } | Ty::Object(_) | Ty::Record { .. })
                }
                _ => false,
            };
            if bad {
                diags.push(
                    Diagnostic::new(
                        loc_of(file, syn_v.ident.span()),
                        format!(
                            "internally-tagged enum variant `{}` is a newtype over a non-map type",
                            v.rust_name
                        ),
                    )
                    .with_help(
                        "serde cannot represent this; use an adjacently tagged enum instead",
                    ),
                );
            }
        }
    }

    Body::Enum { repr, variants }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Body, EnumRepr, Prim, Ty, VariantKind};

    fn parse(src: &str) -> (Option<Decl>, Diagnostics) {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        let mut diags = Diagnostics::default();
        let decl = parse_item(
            &item,
            "crate",
            Path::new("t.rs"),
            &LowerCtx::default(),
            &mut diags,
        );
        (decl, diags)
    }

    fn ok(src: &str) -> Decl {
        let (d, diags) = parse(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        d.expect("expected a decl")
    }

    #[test]
    fn detects_the_ts_derive() {
        let with: syn::DeriveInput = syn::parse_str("#[derive(Serialize, Ts)] struct S;").unwrap();
        let without: syn::DeriveInput = syn::parse_str("#[derive(Serialize)] struct S;").unwrap();
        assert!(has_ts_derive(&with.attrs));
        assert!(!has_ts_derive(&without.attrs));
    }

    #[test]
    fn named_struct_applies_rename_all_and_skips_fields() {
        let d = ok("#[derive(Ts)] #[serde(rename_all = \"camelCase\")] \
                    struct User { user_id: u64, #[serde(skip)] secret: String }");
        assert_eq!(d.ts_name, "User");
        assert_eq!(d.rust_path, "crate::User");
        let Body::Struct(fields) = d.body else {
            panic!("expected struct")
        };
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].ts_name, "userId");
        assert_eq!(
            fields[0].ty,
            Ty::Prim(Prim::Int {
                bits: 64,
                signed: false
            })
        );
    }

    #[test]
    fn ts_rename_renames_the_type_itself() {
        assert_eq!(
            ok("#[derive(Ts)] #[ts(rename = \"Account\")] struct User;").ts_name,
            "Account"
        );
    }

    #[test]
    fn ts_type_short_circuits_lowering() {
        let d = ok("#[derive(Ts)] struct S { #[ts(type = \"string\")] v: Whatever }");
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].ty, Ty::Raw("string".into()));
    }

    #[test]
    fn struct_shapes() {
        assert!(matches!(ok("#[derive(Ts)] struct S;").body, Body::Unit));
        assert!(matches!(
            ok("#[derive(Ts)] struct S(String);").body,
            Body::Newtype(_)
        ));
        assert!(matches!(
            ok("#[derive(Ts)] struct S(String, bool);").body,
            Body::Tuple(_)
        ));
    }

    #[test]
    fn generics_are_collected_and_lifetimes_dropped() {
        let d = ok("#[derive(Ts)] struct Page<'a, T> { items: Vec<T>, name: &'a str }");
        assert_eq!(d.generics, vec!["T"]);
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].ty, Ty::Array(Box::new(Ty::Param("T".into()))));
    }

    #[test]
    fn enum_reprs() {
        let ext = ok("#[derive(Ts)] enum E { A, B { x: i32 } }");
        let Body::Enum { repr, .. } = &ext.body else {
            panic!()
        };
        assert_eq!(repr, &EnumRepr::External);

        let unit = ok("#[derive(Ts)] enum E { A, B }");
        let Body::Enum { repr, .. } = &unit.body else {
            panic!()
        };
        assert_eq!(repr, &EnumRepr::UnitOnly);

        let tagged = ok("#[derive(Ts)] #[serde(tag = \"t\")] enum E { A, B }");
        let Body::Enum { repr, .. } = &tagged.body else {
            panic!()
        };
        assert_eq!(repr, &EnumRepr::Internal { tag: "t".into() });

        let adj = ok("#[derive(Ts)] #[serde(tag = \"t\", content = \"c\")] enum E { A, B(u8) }");
        let Body::Enum { repr, .. } = &adj.body else {
            panic!()
        };
        assert_eq!(
            repr,
            &EnumRepr::Adjacent {
                tag: "t".into(),
                content: "c".into()
            }
        );

        let un = ok("#[derive(Ts)] #[serde(untagged)] enum E { A(u8) }");
        let Body::Enum { repr, .. } = &un.body else {
            panic!()
        };
        assert_eq!(repr, &EnumRepr::Untagged);
    }

    #[test]
    fn variant_kinds_and_renaming() {
        let d = ok("#[derive(Ts)] #[serde(rename_all = \"snake_case\")] \
                    enum E { UnitOne, New(u8), Tup(u8, bool), Str { x: u8 } }");
        let Body::Enum { variants, .. } = d.body else {
            panic!()
        };
        assert_eq!(variants[0].ts_name, "unit_one");
        assert!(matches!(variants[0].kind, VariantKind::Unit));
        assert!(matches!(variants[1].kind, VariantKind::Newtype(_)));
        assert!(matches!(variants[2].kind, VariantKind::Tuple(_)));
        assert!(matches!(variants[3].kind, VariantKind::Struct(_)));
    }

    #[test]
    fn internally_tagged_non_map_newtype_is_an_error() {
        let (_, diags) = parse("#[derive(Ts)] #[serde(tag = \"t\")] enum E { A(u8) }");
        assert!(!diags.is_empty());
        assert!(
            diags
                .iter()
                .any(|d| d.message.contains("internally-tagged"))
        );
    }

    #[test]
    fn const_generics_are_an_error() {
        let (_, diags) = parse("#[derive(Ts)] struct S<const N: usize> { v: [u8; N] }");
        assert!(diags.iter().any(|d| d.message.contains("const generic")));
    }

    #[test]
    fn doc_comments_are_captured_on_type_and_field() {
        let d = ok("#[derive(Ts)] /// Type doc.\nstruct S { /// Field doc.\n v: u8 }");
        assert_eq!(d.docs, vec!["Type doc."]);
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].docs, vec!["Field doc."]);
    }
}

#[cfg(test)]
mod attribute_behaviour_tests {
    use super::*;
    use crate::ir::{Body, Prim, Ty};

    fn parse(src: &str) -> (Option<Decl>, Diagnostics) {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        let mut diags = Diagnostics::default();
        let decl = parse_item(&item, "crate", Path::new("t.rs"), &LowerCtx::default(), &mut diags);
        (decl, diags)
    }

    fn ok(src: &str) -> Decl {
        let (d, diags) = parse(src);
        assert!(diags.is_empty(), "unexpected diagnostics: {diags:?}");
        d.expect("expected a decl")
    }

    #[test]
    fn transparent_struct_emits_the_inner_type_directly() {
        // serde(transparent) serialises as the single field, not as an object.
        let d = ok("#[derive(Ts)] #[serde(transparent)] struct Meters { v: u32 }");
        assert_eq!(d.body, Body::Newtype(Ty::Prim(Prim::Int { bits: 32, signed: false })));
    }

    #[test]
    fn transparent_newtype_is_unchanged() {
        let d = ok("#[derive(Ts)] #[serde(transparent)] struct Name(String);");
        assert_eq!(d.body, Body::Newtype(Ty::Prim(Prim::Str)));
    }

    #[test]
    fn transparent_with_several_fields_is_a_diagnostic() {
        let (_, diags) = parse("#[derive(Ts)] #[serde(transparent)] struct S { a: u8, b: u8 }");
        assert!(
            diags.iter().any(|d| d.message.contains("transparent")),
            "serde rejects this too; we must not silently pick one field"
        );
    }

    #[test]
    fn custom_serialisation_without_an_override_is_a_diagnostic() {
        let (_, diags) =
            parse("#[derive(Ts)] struct S { #[serde(serialize_with = \"f\")] v: u32 }");
        let d = diags.iter().next().expect("expected a diagnostic");
        assert!(d.message.contains("serialize_with"), "got: {}", d.message);
        assert!(d.help.as_ref().unwrap().contains("#[ts(type"));
    }

    #[test]
    fn custom_serialisation_with_a_ts_type_override_is_accepted() {
        let d = ok("#[derive(Ts)] struct S { #[serde(with = \"m\")] #[ts(type = \"string\")] v: u32 }");
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].ty, Ty::Raw("string".into()));
    }

    #[test]
    fn variant_level_rename_all_renames_that_variants_fields() {
        let d = ok("#[derive(Ts)] enum E { \
                    #[serde(rename_all = \"camelCase\")] A { long_name: u8 }, \
                    B { long_name: u8 } }");
        let Body::Enum { variants, .. } = d.body else { panic!() };
        let VariantKind::Struct(a) = &variants[0].kind else { panic!() };
        let VariantKind::Struct(b) = &variants[1].kind else { panic!() };
        assert_eq!(a[0].ts_name, "longName", "variant-level rename_all not applied");
        assert_eq!(b[0].ts_name, "long_name", "must not leak to sibling variants");
    }

    #[test]
    fn raw_identifiers_lose_their_prefix_like_serde_does() {
        // serde serialises `r#type` as `type`; syn hands us the `r#` intact.
        let d = ok("#[derive(Ts)] struct S { r#type: u8, r#match: u8 }");
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].ts_name, "type");
        assert_eq!(f[1].ts_name, "match");
        assert_eq!(f[0].rust_name, "type");
    }

    #[test]
    fn raw_identifiers_are_unrawed_before_rename_rules_apply() {
        let d = ok("#[derive(Ts)] #[serde(rename_all = \"camelCase\")] \
                    struct S { r#type: u8 }");
        let Body::Struct(f) = d.body else { panic!() };
        assert_eq!(f[0].ts_name, "type", "must not become r#Type or similar");
    }

    #[test]
    fn raw_variants_and_type_names_are_unrawed_too() {
        let d = ok("#[derive(Ts)] enum E { r#match, Other }");
        let Body::Enum { variants, .. } = d.body else { panic!() };
        assert_eq!(variants[0].ts_name, "match");
    }

    #[test]
    fn readonly_and_inline_reach_the_field() {
        let d = ok("#[derive(Ts)] struct S { #[ts(readonly)] a: u8, #[ts(inline)] b: Other }");
        let Body::Struct(f) = d.body else { panic!() };
        assert!(f[0].readonly);
        assert!(!f[0].inline);
        assert!(f[1].inline);
        assert!(!f[1].readonly);
    }

    #[test]
    fn container_readonly_applies_to_every_field() {
        let d = ok("#[derive(Ts)] #[ts(readonly)] struct S { a: u8, b: u8 }");
        let Body::Struct(f) = d.body else { panic!() };
        assert!(f.iter().all(|x| x.readonly), "container readonly must reach all fields");
    }
}

#[cfg(test)]
mod collision_tests {
    use super::*;

    fn parse(src: &str) -> (Option<Decl>, Diagnostics) {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        let mut diags = Diagnostics::default();
        let decl = parse_item(&item, "crate", Path::new("t.rs"), &LowerCtx::default(), &mut diags);
        (decl, diags)
    }

    /// Collisions are warnings, never errors: the build must still succeed.
    fn collision_messages(src: &str) -> Vec<String> {
        let (_, diags) = parse(src);
        assert!(!diags.has_errors(), "a name collision must not fail the build");
        diags
            .iter()
            .map(|d| d.message.clone())
            .filter(|m| m.contains("all map to") || m.contains("both map to"))
            .collect()
    }

    #[test]
    fn two_fields_renamed_to_the_same_key_is_an_error() {
        // serde emits {"same":1,"same":2}; JSON.parse keeps only the last.
        let msgs = collision_messages(
            "#[derive(Ts)] struct S { #[serde(rename = \"same\")] a: u8, \
             #[serde(rename = \"same\")] b: u8 }",
        );
        assert_eq!(msgs.len(), 1, "expected exactly one collision, got {msgs:?}");
        assert!(msgs[0].contains("`same`"), "{}", msgs[0]);
    }

    #[test]
    fn a_raw_identifier_colliding_with_its_escaped_twin_is_caught() {
        // Both camelCase to `type` — the case the fidelity suite tripped over.
        let msgs = collision_messages(
            "#[derive(Ts)] #[serde(rename_all = \"camelCase\")] \
             struct S { type_: u8, r#type: u8 }",
        );
        assert_eq!(msgs.len(), 1, "got {msgs:?}");
    }

    #[test]
    fn colliding_enum_variants_are_caught() {
        let msgs = collision_messages(
            "#[derive(Ts)] enum E { #[serde(rename = \"x\")] A, #[serde(rename = \"x\")] B }",
        );
        assert_eq!(msgs.len(), 1, "got {msgs:?}");
        assert!(msgs[0].contains("variants"), "{}", msgs[0]);
    }

    #[test]
    fn colliding_fields_inside_a_struct_variant_are_caught() {
        let msgs = collision_messages(
            "#[derive(Ts)] enum E { V { #[serde(rename = \"y\")] a: u8, \
             #[serde(rename = \"y\")] b: u8 } }",
        );
        assert_eq!(msgs.len(), 1, "got {msgs:?}");
    }

    #[test]
    fn distinct_names_produce_no_diagnostic() {
        assert!(collision_messages("#[derive(Ts)] struct S { a: u8, b: u8 }").is_empty());
        assert!(collision_messages("#[derive(Ts)] enum E { A, B }").is_empty());
        assert!(
            collision_messages(
                "#[derive(Ts)] #[serde(rename_all = \"camelCase\")] \
                 struct S { user_id: u8, user_name: u8 }"
            )
            .is_empty()
        );
    }

    #[test]
    fn a_skipped_field_cannot_collide() {
        // It never reaches the output, so it cannot clash with anything.
        assert!(
            collision_messages(
                "#[derive(Ts)] struct S { #[serde(rename = \"same\")] a: u8, \
                 #[serde(skip)] #[serde(rename = \"same\")] b: u8 }"
            )
            .is_empty()
        );
    }

    #[test]
    fn separate_variants_may_reuse_field_names() {
        // Each variant is its own object; only same-level clashes matter.
        assert!(
            collision_messages("#[derive(Ts)] enum E { A { x: u8 }, B { x: u8 } }").is_empty()
        );
    }
}

#[cfg(test)]
mod collision_output_tests {
    use super::*;
    use crate::ir::Prim;

    fn body_of(src: &str) -> Body {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        let mut diags = Diagnostics::default();
        let d = parse_item(&item, "crate", Path::new("t.rs"), &LowerCtx::default(), &mut diags);
        assert!(!diags.has_errors(), "collisions must not fail the build");
        d.expect("decl").body
    }

    #[test]
    fn colliding_fields_collapse_to_one_key_with_a_union_type() {
        // A repeated key in a TS object type is a tsc error, so the output must
        // carry the name once. The union keeps both possibilities.
        let Body::Struct(f) = body_of(
            "#[derive(Ts)] struct S { #[serde(rename = \"v\")] a: u8, \
             #[serde(rename = \"v\")] b: String }",
        ) else {
            panic!("expected struct")
        };
        assert_eq!(f.len(), 1, "duplicate key would be invalid TypeScript");
        assert_eq!(f[0].ts_name, "v");
        assert_eq!(
            f[0].ty,
            Ty::Union(vec![Ty::Prim(Prim::Int { bits: 8, signed: false }), Ty::Prim(Prim::Str)])
        );
    }

    #[test]
    fn identical_types_collapse_without_a_pointless_union() {
        let Body::Struct(f) = body_of(
            "#[derive(Ts)] #[serde(rename_all = \"camelCase\")] \
             struct S { type_: u8, r#type: u8 }",
        ) else {
            panic!()
        };
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].ty, Ty::Prim(Prim::Int { bits: 8, signed: false }), "expected u8, not u8|u8");
    }

    #[test]
    fn the_merged_key_is_optional_only_if_every_contributor_is() {
        let Body::Struct(f) = body_of(
            "#[derive(Ts)] struct S { #[serde(rename = \"v\")] a: u8, \
             #[serde(rename = \"v\", skip_serializing_if = \"x\")] b: u8 }",
        ) else {
            panic!()
        };
        assert!(!f[0].optional, "one field always writes the key, so it is not optional");
    }

    #[test]
    fn three_way_collisions_merge_too() {
        let Body::Struct(f) = body_of(
            "#[derive(Ts)] struct S { #[serde(rename = \"v\")] a: u8, \
             #[serde(rename = \"v\")] b: bool, #[serde(rename = \"v\")] c: String }",
        ) else {
            panic!()
        };
        assert_eq!(f.len(), 1);
        let Ty::Union(arms) = &f[0].ty else { panic!("expected a union") };
        assert_eq!(arms.len(), 3);
    }
}
