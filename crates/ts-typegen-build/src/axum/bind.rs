//! Classifying a handler's parameters by extractor role: which one carries
//! the path captures, which carries the query string, and which carries the
//! request body. Each parameter's type is read by its final path segment and
//! looked up in [`Axum::extractor_for`]; anything the config doesn't
//! recognize is an error rather than a silent skip.

use crate::axum::config::{Axum, Extractor};
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{RouteBody, SourceLoc, Ty};
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
