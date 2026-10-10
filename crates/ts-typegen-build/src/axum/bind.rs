//! Classifying a handler's parameters by extractor role: which one carries
//! the path captures, which carries the query string, and which carries the
//! request body. Each parameter's type is read by its final path segment and
//! looked up in [`Axum::extractor_for`]; anything the config doesn't
//! recognize is an error rather than a silent skip.

use std::collections::HashMap;

use crate::attrs::RenameRule;
use crate::axum::config::{Axum, Extractor};
use crate::axum::path::Segment;
use crate::axum::routes::Overrides;
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Body, Field, Prim, Registry, RouteBody, RouteParam, SourceLoc, Ty};
use crate::lower::{LowerCtx, lower_type};

/// How a [`PathExtractor`]'s pattern names its captured value(s).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathBinding {
    /// A single capture bound to one identifier, e.g. `Path(id): Path<i64>`.
    One(String),
    /// Captures destructured positionally out of a tuple, e.g.
    /// `Path((id, slug)): Path<(i64, String)>`.
    Tuple(Vec<String>),
    /// A pattern this module can't decompose into names: a struct
    /// destructure, a wildcard `_`, or anything else.
    Opaque,
}

/// A handler's `Path<..>` extractor. The type is kept unlowered -- Task 9
/// needs the raw `syn::Type` to zip parameter names against the route
/// template's captures, which this module has no visibility into.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathExtractor {
    /// How the extractor's pattern names its captured value(s).
    pub binding: PathBinding,
    /// The extractor's type as written, e.g. `Path<i64>`.
    pub ty: syn::Type,
}

/// A handler's parameters, classified by the role they play in the
/// generated client.
#[derive(Debug, Clone)]
pub struct Extracted {
    /// The `Path<..>` extractor, if the handler has one.
    pub path: Option<PathExtractor>,
    /// The query extractor's binding name and lowered type, if the handler
    /// has one.
    pub query: Option<(String, Ty)>,
    /// The body extractor's binding name and shape, if the handler has one.
    pub body: Option<(String, RouteBody)>,
}

/// Classifies every parameter of `sig` by extractor role. Each parameter's
/// type is read by its final path segment, after unwrapping one layer of
/// `Option<..>` or `Result<.., _>`, and looked up in `cfg.extractor_for`. A
/// segment the config doesn't recognize raises an error naming the
/// parameter; a second body or query extractor raises an error naming both
/// bindings and keeps the first.
pub fn classify(
    sig: &syn::Signature,
    cfg: &Axum,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Extracted {
    let mut out = Extracted { path: None, query: None, body: None };

    for arg in &sig.inputs {
        let syn::FnArg::Typed(pat_type) = arg else { continue };
        let effective = unwrap_option_or_result(&pat_type.ty);
        let Some(segment) = final_segment(effective) else { continue };
        let Some(extractor) = cfg.extractor_for(&segment) else {
            diags.push(
                Diagnostic::new(
                    loc.clone(),
                    format!("handler parameter uses extractor `{segment}`, which isn't recognized"),
                )
                .with_help(format!(
                    "register it with `Axum::extractor(\"{segment}\", ...)`, \
                     or ignore it if it already contributes nothing"
                )),
            );
            continue;
        };

        match extractor {
            Extractor::Ignore => {}

            Extractor::Path => {
                let binding = path_binding(&pat_type.pat);
                if let Some(existing) = &out.path {
                    diags.push(
                        Diagnostic::new(
                            loc.clone(),
                            format!(
                                "handler takes two path extractors, `{}` and `{}`",
                                describe_path_binding(&existing.binding),
                                describe_path_binding(&binding),
                            ),
                        )
                        .with_help("a route has one path template; remove one"),
                    );
                    continue;
                }
                out.path = Some(PathExtractor { binding, ty: effective.clone() });
            }

            Extractor::Query => {
                let name = binding_name(&pat_type.pat).unwrap_or_else(|| "query".to_string());
                if let Some((first, _)) = &out.query {
                    diags.push(
                        Diagnostic::new(
                            loc.clone(),
                            format!("handler takes two query extractors, `{first}` and `{name}`"),
                        )
                        .with_help("axum can only bind one query extractor; remove one"),
                    );
                    continue;
                }
                let ty = match first_generic_arg(effective) {
                    Some(inner) => lower_type(inner, ctx, loc, diags),
                    None => Ty::Unknown,
                };
                out.query = Some((name, ty));
            }

            Extractor::Body => {
                let name = binding_name(&pat_type.pat).unwrap_or_else(|| "body".to_string());
                if let Some((first, _)) = &out.body {
                    diags.push(
                        Diagnostic::new(
                            loc.clone(),
                            format!("handler takes two body extractors, `{first}` and `{name}`"),
                        )
                        .with_help("axum can only read the request body once; remove one"),
                    );
                    continue;
                }
                let body = route_body_for(&segment, effective, ctx, loc, diags);
                out.body = Some((name, body));
            }
        }
    }

    out
}

/// The [`RouteBody`] shape for a `Body`-role extractor: `Multipart` and
/// `String` are fixed shapes with no generic argument to lower; anything
/// else lowers its generic argument when it has one (`Json<T>`, `Form<T>`,
/// ...), and otherwise reads as a raw, unparsed body (`Bytes`, `Body`, ...).
fn route_body_for(
    segment: &str,
    ty: &syn::Type,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> RouteBody {
    match segment {
        "Multipart" => RouteBody::FormData,
        "String" => RouteBody::Text,
        _ => match first_generic_arg(ty) {
            Some(inner) => RouteBody::Typed(lower_type(inner, ctx, loc, diags)),
            None => RouteBody::Raw,
        },
    }
}

/// The binding name for a body or query extractor: `Json(scheme)` and
/// similar single-element tuple-struct patterns give `scheme`; a plain
/// `m: Multipart` gives `m`. Anything else (a destructured struct, `_`)
/// can't be named this way.
fn binding_name(pat: &syn::Pat) -> Option<String> {
    match pat {
        syn::Pat::Ident(i) => Some(i.ident.to_string()),
        syn::Pat::TupleStruct(ts) if ts.elems.len() == 1 => match &ts.elems[0] {
            syn::Pat::Ident(i) => Some(i.ident.to_string()),
            _ => None,
        },
        _ => None,
    }
}

/// The [`PathBinding`] for a `Path<..>` extractor's pattern: a single
/// `Path(id)` capture, a tuple-destructured `Path((id, slug))`, or anything
/// else that can't be confidently decomposed into names.
fn path_binding(pat: &syn::Pat) -> PathBinding {
    if let syn::Pat::TupleStruct(ts) = pat {
        if ts.elems.len() == 1 {
            match &ts.elems[0] {
                syn::Pat::Ident(i) => return PathBinding::One(i.ident.to_string()),
                syn::Pat::Tuple(t) => {
                    let names: Option<Vec<String>> = t
                        .elems
                        .iter()
                        .map(|e| match e {
                            syn::Pat::Ident(i) => Some(i.ident.to_string()),
                            _ => None,
                        })
                        .collect();
                    if let Some(names) = names {
                        return PathBinding::Tuple(names);
                    }
                }
                _ => {}
            }
        }
    }
    PathBinding::Opaque
}

/// A short, readable name for a [`PathBinding`], for naming it in a
/// diagnostic: `id` for `One`, `(id, slug)` for `Tuple`, and a fixed
/// placeholder for `Opaque`, which has no name to show.
fn describe_path_binding(b: &PathBinding) -> String {
    match b {
        PathBinding::One(name) => name.clone(),
        PathBinding::Tuple(names) => format!("({})", names.join(", ")),
        PathBinding::Opaque => "<opaque>".to_string(),
    }
}

/// Strips a reference, paren, or group wrapper to get at the type underneath.
fn peel(ty: &syn::Type) -> &syn::Type {
    match ty {
        syn::Type::Reference(r) => peel(&r.elem),
        syn::Type::Paren(p) => peel(&p.elem),
        syn::Type::Group(g) => peel(&g.elem),
        _ => ty,
    }
}

/// Unwraps one layer of `Option<..>` or `Result<.., _>` -- the `Ok` type, for
/// `Result` -- so an extractor wrapped in either is still classified by the
/// extractor's own segment. Only one layer is stripped; a type wrapped in
/// both is not expected and isn't unwrapped twice.
fn unwrap_option_or_result(ty: &syn::Type) -> &syn::Type {
    let peeled = peel(ty);
    let syn::Type::Path(p) = peeled else { return peeled };
    let Some(seg) = p.path.segments.last() else { return peeled };
    if seg.ident != "Option" && seg.ident != "Result" {
        return peeled;
    }
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else { return peeled };
    match ab.args.first() {
        Some(syn::GenericArgument::Type(inner)) => peel(inner),
        _ => peeled,
    }
}

/// The type's final path segment's name, e.g. `Json` for `Json<NewScheme>`.
fn final_segment(ty: &syn::Type) -> Option<String> {
    match peel(ty) {
        syn::Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        _ => None,
    }
}

/// The first generic type argument of a path type, e.g. `NewScheme` for
/// `Json<NewScheme>`. `None` for a type with no generic arguments at all,
/// such as `Bytes` or `Multipart`.
fn first_generic_arg(ty: &syn::Type) -> Option<&syn::Type> {
    let syn::Type::Path(p) = peel(ty) else { return None };
    let seg = p.path.segments.last()?;
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else { return None };
    ab.args.iter().find_map(|a| match a {
        syn::GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// Zips a route template's captures against its handler's `Path<..>`
/// extractor, producing one [`RouteParam`] per capture in template order.
///
/// With no `Path` extractor at all, every capture widens to `string |
/// number` and the route gets a single warning naming all of them -- axum
/// still binds the route, it just gives the handler nothing typed to read
/// the capture as. With a count mismatch -- too few or too many values for
/// the captures, including a `Path<Struct>` whose field count doesn't match
/// -- one error names both counts and the route is dropped (an empty
/// `Vec`) rather than emitted half-formed.
pub fn zip_params(
    segs: &[Segment],
    ex: Option<&PathExtractor>,
    ov: &Overrides,
    reg: &Registry,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    let captures = capture_specs(segs);

    let Some(ex) = ex else {
        return widen_without_extractor(&captures, ov, loc, diags);
    };

    let Some(inner) = first_generic_arg(&ex.ty) else {
        return zip_scalar(&captures, Ty::Unknown, one_binding_name(&ex.binding), ov, loc, diags);
    };

    if let syn::Type::Tuple(t) = peel(inner) {
        let elem_types: Vec<Ty> = t.elems.iter().map(|e| lower_type(e, ctx, loc, diags)).collect();
        return zip_tuple(&captures, elem_types, ov, loc, diags);
    }

    if let Some(fields) = struct_fields_for(inner, reg) {
        return zip_struct(&captures, fields, ov, loc, diags);
    }

    let ty = lower_type(inner, ctx, loc, diags);
    zip_scalar(&captures, ty, one_binding_name(&ex.binding), ov, loc, diags)
}

/// Every [`Segment::Capture`] in a template, in order, paired with its
/// wildcard flag -- [`crate::axum::path::capture_names`] drops the flag, and
/// zipping needs it to carry over onto the resulting [`RouteParam`].
fn capture_specs(segs: &[Segment]) -> Vec<(String, bool)> {
    segs.iter()
        .filter_map(|seg| match seg {
            Segment::Capture { name, wildcard } => Some((name.clone(), *wildcard)),
            Segment::Literal(_) => None,
        })
        .collect()
}

/// The bound name for a `Path<..>` extractor whose pattern is a single
/// identifier, e.g. `id` for `Path(id): Path<i64>`. `None` for a tuple or
/// opaque binding, which have no single name to offer.
fn one_binding_name(binding: &PathBinding) -> Option<&str> {
    match binding {
        PathBinding::One(name) => Some(name.as_str()),
        PathBinding::Tuple(_) | PathBinding::Opaque => None,
    }
}

/// A registered struct's fields, if `ty`'s final path segment names a
/// [`Body::Struct`] declaration in `reg` -- the same final-segment matching
/// `Config::map_type` uses, since the generic argument as written may be a
/// bare name or a path qualified by whatever the handler's module imported.
fn struct_fields_for<'a>(ty: &syn::Type, reg: &'a Registry) -> Option<&'a [Field]> {
    let name = final_segment(ty)?;
    reg.decls().iter().find_map(|decl| {
        if decl.rust_path.rsplit("::").next().unwrap_or(&decl.rust_path) != name {
            return None;
        }
        match &decl.body {
            Body::Struct(fields) => Some(fields.as_slice()),
            _ => None,
        }
    })
}

/// Zips captures positionally against a `Path<(A, B, ...)>` tuple's element
/// types. Names always come from the template, never from the binding's
/// tuple-element identifiers -- those are typically throwaway pattern names
/// (`Path((a, b))`) with no TS-facing meaning of their own.
fn zip_tuple(
    captures: &[(String, bool)],
    elem_types: Vec<Ty>,
    ov: &Overrides,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    let arity = elem_types.len();
    if captures.len() != arity {
        return arity_mismatch(captures.len(), arity, loc, diags);
    }
    captures
        .iter()
        .zip(elem_types)
        .map(|((name, wildcard), ty)| RouteParam {
            name: resolve_name(name, None, ov),
            ty: resolve_ty(name, ov, ty),
            wildcard: *wildcard,
        })
        .collect()
}

/// Zips a single capture against a `Path<T>` scalar's type. `binding` is the
/// extractor's bound identifier, if it has one plain enough to name.
fn zip_scalar(
    captures: &[(String, bool)],
    ty: Ty,
    binding: Option<&str>,
    ov: &Overrides,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    if captures.len() != 1 {
        return arity_mismatch(captures.len(), 1, loc, diags);
    }
    let (name, wildcard) = &captures[0];
    vec![RouteParam {
        name: resolve_name(name, binding, ov),
        ty: resolve_ty(name, ov, ty),
        wildcard: *wildcard,
    }]
}

/// Zips captures against a `Path<Struct>`'s own fields, matched by name --
/// the struct's field names are the capture-to-param key, not the
/// extractor's binding pattern, which only names the whole struct value.
fn zip_struct(
    captures: &[(String, bool)],
    fields: &[Field],
    ov: &Overrides,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    if captures.len() != fields.len() {
        return arity_mismatch(captures.len(), fields.len(), loc, diags);
    }
    let mut by_ts_name: HashMap<&str, &Field> =
        fields.iter().map(|f| (f.ts_name.as_str(), f)).collect();
    let mut out = Vec::with_capacity(captures.len());
    for (name, wildcard) in captures {
        let Some(field) = by_ts_name.remove(name.as_str()) else {
            return struct_name_mismatch(captures, fields, loc, diags);
        };
        out.push(RouteParam {
            name: resolve_name(name, None, ov),
            ty: resolve_ty(name, ov, field.ty.clone()),
            wildcard: *wildcard,
        });
    }
    out
}

/// Every capture widened to `string | number`, for a handler with no `Path`
/// extractor at all -- legal axum, since an unread capture still matches at
/// runtime, so this is one warning naming every capture rather than an
/// error.
fn widen_without_extractor(
    captures: &[(String, bool)],
    ov: &Overrides,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    if !captures.is_empty() {
        let names: Vec<&str> = captures.iter().map(|(name, _)| name.as_str()).collect();
        diags.push(
            Diagnostic::warning(
                loc.clone(),
                format!(
                    "path captures {} but the handler has no `Path` extractor; \
                     treating {} as `string | number`",
                    names.join(", "),
                    if names.len() == 1 { "it" } else { "them" },
                ),
            )
            .with_help(
                "add a `Path<..>` parameter to the handler, or narrow the type with \
                 a `params(name(type = \"...\"))` override",
            ),
        );
    }
    let widened = Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Float)]);
    captures
        .iter()
        .map(|(name, wildcard)| RouteParam {
            name: resolve_name(name, None, ov),
            ty: resolve_ty(name, ov, widened.clone()),
            wildcard: *wildcard,
        })
        .collect()
}

/// The name a capture's [`RouteParam`] is emitted under: a `params(name(rename
/// = ..))` override first, then the extractor binding's own identifier when
/// it has one, then the template's capture name as the last resort --
/// whichever wins is camelCased, since that's the casing a generated client
/// call site expects regardless of what the Rust or template side wrote.
fn resolve_name(capture: &str, binding: Option<&str>, ov: &Overrides) -> String {
    let raw = ov.params.get(capture).and_then(|p| p.rename.as_deref()).or(binding).unwrap_or(capture);
    RenameRule::Camel.apply_to_field(raw)
}

/// A capture's [`RouteParam::ty`]: a `params(name(type = ..))` override
/// replaces whatever was inferred, verbatim and unlowered -- the override is
/// author-supplied TypeScript text, not a Rust type to lower.
fn resolve_ty(capture: &str, ov: &Overrides, inferred: Ty) -> Ty {
    match ov.params.get(capture).and_then(|p| p.ty.as_deref()) {
        Some(template) => Ty::Raw { template: template.to_string(), args: Vec::new() },
        None => inferred,
    }
}

/// One error naming both the template's capture count and the extractor's
/// arity, dropping the route (an empty `Vec`) rather than emitting it
/// half-formed -- axum itself rejects this mismatch at runtime.
fn arity_mismatch(
    captures: usize,
    arity: usize,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    diags.push(
        Diagnostic::new(
            loc.clone(),
            format!("path has {captures} capture(s) but `Path` extracts {arity}"),
        )
        .with_help("axum rejects a count mismatch at runtime as WrongNumberOfParameters"),
    );
    Vec::new()
}

/// A `Path<Struct>`'s fields don't match the template's captures by name --
/// same arity, perhaps, but matching by name found a capture with no field to
/// pair it with. Both sets are named so the mismatch is fixable without
/// guessing which side is wrong.
fn struct_name_mismatch(
    captures: &[(String, bool)],
    fields: &[Field],
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<RouteParam> {
    let capture_names: Vec<&str> = captures.iter().map(|(name, _)| name.as_str()).collect();
    let field_names: Vec<&str> = fields.iter().map(|f| f.ts_name.as_str()).collect();
    diags.push(
        Diagnostic::new(
            loc.clone(),
            format!(
                "path captures [{}] don't match `Path` struct fields [{}]",
                capture_names.join(", "),
                field_names.join(", "),
            ),
        )
        .with_help("name the struct's fields the same as the path template's captures"),
    );
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::routes::ParamOverride;

    fn sig(src: &str) -> syn::Signature {
        syn::parse_str::<syn::ItemFn>(src).expect("fn must parse").sig
    }

    fn run(src: &str, cfg: &Axum) -> (Extracted, Diagnostics) {
        let mut d = Diagnostics::default();
        let loc = SourceLoc { file: "h.rs".into(), line: 1 };
        let e = classify(&sig(src), cfg, &LowerCtx::default(), &loc, &mut d);
        (e, d)
    }

    #[test]
    fn state_is_ignored_and_json_becomes_the_body_under_its_binding_name() {
        let (e, d) = run(
            "async fn h(State(ApiState { db, .. }): State<ApiState>, Json(scheme): Json<NewScheme>) {}",
            &Axum::new(),
        );
        assert!(!d.has_errors());
        let (name, body) = e.body.expect("body");
        assert_eq!(name, "scheme");
        assert!(matches!(body, RouteBody::Typed(_)));
    }

    #[test]
    fn query_is_captured_under_its_binding_name() {
        let (e, _) = run("async fn h(Query(payload): Query<SearchQuery>) {}", &Axum::new());
        assert_eq!(e.query.expect("query").0, "payload");
    }

    #[test]
    fn multipart_and_bytes_are_fixed_body_shapes() {
        let (m, _) = run("async fn h(m: Multipart) {}", &Axum::new());
        assert!(matches!(m.body.unwrap().1, RouteBody::FormData));
        let (b, _) = run("async fn h(b: Bytes) {}", &Axum::new());
        assert!(matches!(b.body.unwrap().1, RouteBody::Raw));
    }

    #[test]
    fn an_unknown_extractor_is_an_error_naming_the_parameter() {
        let (_, d) = run("async fn h(MysteryWrapper(x): MysteryWrapper<Thing>) {}", &Axum::new());
        assert!(d.has_errors());
        let m = &d.iter().next().unwrap().message;
        assert!(m.contains("MysteryWrapper"), "got: {m}");
    }

    #[test]
    fn a_configured_extractor_silences_the_error() {
        let cfg = Axum::new().extractor("AuthSession", Extractor::Ignore);
        let (e, d) = run("async fn h(AuthSession(s): AuthSession) {}", &cfg);
        assert!(!d.has_errors());
        assert!(e.body.is_none() && e.query.is_none() && e.path.is_none());
    }

    #[test]
    fn two_body_extractors_error_naming_both() {
        let (_, d) = run("async fn h(Json(a): Json<A>, Form(b): Form<B>) {}", &Axum::new());
        assert!(d.has_errors());
        let m = &d.iter().next().unwrap().message;
        assert!(m.contains('a') && m.contains('b'), "both bindings must be named, got: {m}");
    }

    #[test]
    fn an_optional_body_is_still_a_body() {
        let (e, d) = run("async fn h(body: Option<Json<NewScheme>>) {}", &Axum::new());
        assert!(!d.has_errors());
        assert!(e.body.is_some());
    }

    #[test]
    fn two_path_extractors_error_naming_both_and_keep_the_first() {
        let (e, d) =
            run("async fn h(Path(id): Path<i64>, Path(slug): Path<String>) {}", &Axum::new());
        assert!(d.has_errors());
        let m = &d.iter().next().unwrap().message;
        assert!(m.contains("id") && m.contains("slug"), "both bindings must be named, got: {m}");
        assert_eq!(e.path.unwrap().binding, PathBinding::One("id".to_string()));
    }

    fn zip_diag(template: &str, handler: &str) -> (Vec<RouteParam>, Diagnostics) {
        let mut d = Diagnostics::default();
        let loc = SourceLoc { file: "h.rs".into(), line: 1 };
        let segs = crate::axum::path::parse_template(template, &loc, &mut d).expect("template");
        let ex = classify(&sig(handler), &Axum::new(), &LowerCtx::default(), &loc, &mut d);
        let params = zip_params(
            &segs,
            ex.path.as_ref(),
            &Overrides::default(),
            &Registry::default(),
            &LowerCtx::default(),
            &loc,
            &mut d,
        );
        (params, d)
    }

    fn zip(template: &str, handler: &str) -> Vec<RouteParam> {
        let (p, d) = zip_diag(template, handler);
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        p
    }

    fn zip_with_override(template: &str, handler: &str, param: &str, ts: &str) -> Vec<RouteParam> {
        let mut ov = Overrides::default();
        ov.params.insert(param.to_string(), ParamOverride { ty: Some(ts.into()), rename: None });
        let mut d = Diagnostics::default();
        let loc = SourceLoc { file: "h.rs".into(), line: 1 };
        let segs = crate::axum::path::parse_template(template, &loc, &mut d).expect("template");
        let ex = classify(&sig(handler), &Axum::new(), &LowerCtx::default(), &loc, &mut d);
        zip_params(&segs, ex.path.as_ref(), &ov, &Registry::default(), &LowerCtx::default(), &loc, &mut d)
    }

    #[test]
    fn one_capture_and_one_scalar_path_zip_by_name() {
        let p = zip("/a/{id}", "async fn h(Path(id): Path<i64>) {}");
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].name, "id");
        assert!(matches!(p[0].ty, Ty::Prim(Prim::Int { .. })));
    }

    #[test]
    fn a_tuple_path_zips_positionally_and_names_come_from_the_template() {
        let p = zip("/a/{id}/b/{slug}", "async fn h(Path((a, b)): Path<(i64, String)>) {}");
        assert_eq!(p.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["id", "slug"]);
        assert!(matches!(p[1].ty, Ty::Prim(Prim::Str)));
    }

    #[test]
    fn a_binding_name_wins_over_the_template_name() {
        let p = zip("/a/{id}", "async fn h(Path(patient_id): Path<i64>) {}");
        assert_eq!(p[0].name, "patientId");
    }

    #[test]
    fn too_many_tuple_elements_for_the_template_is_an_error() {
        let (p, d) = zip_diag("/a/{id}", "async fn h(Path((a, b)): Path<(i64, String)>) {}");
        assert!(d.has_errors());
        assert!(d.iter().next().unwrap().message.contains("1 capture"), "arity must be named");
        assert!(p.is_empty());
    }

    #[test]
    fn too_few_tuple_elements_for_the_template_is_also_an_error() {
        let (_, d) = zip_diag("/a/{id}/b/{slug}", "async fn h(Path(id): Path<i64>) {}");
        assert!(d.has_errors());
    }

    #[test]
    fn a_capture_with_no_path_extractor_widens_and_warns() {
        let (p, d) = zip_diag("/a/{id}", "async fn h() {}");
        assert!(!d.has_errors(), "legal axum must not fail the build");
        assert_eq!(d.len(), 1);
        assert_eq!(p[0].name, "id");
        assert_eq!(p[0].ty, Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Float)]));
    }

    #[test]
    fn a_wildcard_capture_is_a_string_parameter() {
        let p = zip("/a/{*rest}", "async fn h(Path(rest): Path<String>) {}");
        assert!(p[0].wildcard);
        assert_eq!(p[0].name, "rest");
    }

    #[test]
    fn a_param_override_replaces_the_inferred_type() {
        let p = zip_with_override("/a/{id}", "async fn h() {}", "id", "number");
        assert!(matches!(&p[0].ty, Ty::Raw { template, .. } if template == "number"));
    }
}
