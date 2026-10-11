//! Reading route metadata off a handler fn's attributes.
//!
//! A handler is discovered either through the built-in `#[ts_route(...)]` or
//! through a foreign attribute registered with [`Axum::route_attr`], e.g.
//! `#[api(Route = "...", Method = "...")]`. Both are read the same way here:
//! [`Axum::route_attrs`] already carries `ts_route` by default, so matching
//! `attr.path()` against each configured [`RouteAttr`] name covers both.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::attrs::RenameRule;
use crate::axum::bind;
use crate::axum::config::{Axum, Client, Discover, QueryArrays, RouteAttr};
use crate::axum::path;
use crate::axum::router::facts_from_routers;
use crate::config::Config;
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Body, Method, Registry, Route, RouteBody, RouteParam, SourceLoc, Ty};
use crate::lower::LowerCtx;
use crate::parse::loc_of;
use crate::scan::FnItem;

/// A `params(name(...))` override for one path or query parameter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParamOverride {
    /// `type = "..."` — the TypeScript type to use instead of the one
    /// inferred from the parameter's Rust type.
    pub ty: Option<String>,
    /// `rename = "..."` — the name this parameter is emitted under in the
    /// generated client, instead of its capture name.
    pub rename: Option<String>,
}

/// Every override a route attribute can carry, independent of which
/// attribute syntax (built-in or foreign) wrote it.
#[derive(Debug, Clone, Default)]
pub struct Overrides {
    /// `fn_name = "..."` — overrides the generated client function's name.
    pub fn_name: Option<String>,
    /// The configured response-type key, e.g. `response = "..."` or
    /// `Response = "..."`. Also readable as `response(type = "...")`.
    pub response: Option<String>,
    /// The configured request-body-type key, e.g. `request = "..."`. Also
    /// readable as `request(type = "...")`.
    pub body: Option<String>,
    /// `query_key = "..."` — overrides the key a non-path, non-body
    /// parameter is read under in the generated client's query object.
    pub query_key: Option<String>,
    /// `params(name(type = "...", rename = "..."))` — per-parameter
    /// overrides, keyed by capture or parameter name.
    pub params: HashMap<String, ParamOverride>,
    /// `skip` — hides this route from generation entirely.
    pub skip: bool,
    /// `prefix` / `prefix = false` — whether the configured
    /// [`Axum::prefix`] applies to this route. `None` defers to the global
    /// default.
    pub prefix: Option<bool>,
}

/// Route metadata read off one handler fn's attributes.
#[derive(Debug, Clone)]
pub struct RouteFact {
    /// The HTTP method, if the attribute declared one.
    pub method: Option<String>,
    /// The route path template, if the attribute declared one.
    pub path: Option<String>,
    /// Every override carried alongside path and method.
    pub overrides: Overrides,
    /// Where the attribute, or the router registration, was found.
    pub loc: SourceLoc,
    /// The resolved handler's bare name and enclosing module path, set only
    /// on a router-sourced fact -- an attribute-sourced fact's handler is
    /// already the `FnItem` [`fact_from_attrs`] was called on, so it never
    /// needs to name it separately.
    pub handler: Option<(String, String)>,
}

/// Reads route metadata off `f`, trying each of `cfg`'s configured
/// [`RouteAttr`]s in turn. Returns `None` when `f` carries none of them at
/// all. A matching attribute with no parenthesized arguments (a bare
/// `#[ts_route]`) still produces a `RouteFact`, just one with every field
/// left at its default.
pub fn fact_from_attrs(f: &FnItem, cfg: &Axum, diags: &mut Diagnostics) -> Option<RouteFact> {
    let loc = loc_of(&f.file, f.item.sig.ident.span());
    for route_attr in cfg.route_attrs() {
        let matches: Vec<&syn::Attribute> =
            f.item.attrs.iter().filter(|a| a.path().is_ident(route_attr.name())).collect();
        if matches.is_empty() {
            continue;
        }
        let mut fact = RouteFact {
            method: None,
            path: None,
            overrides: Overrides::default(),
            loc: loc.clone(),
            handler: None,
        };
        for attr in matches {
            let _ = attr.parse_nested_meta(|meta| {
                apply_meta(&mut fact, route_attr, &meta, &loc, diags);
                Ok(())
            });
        }
        return Some(fact);
    }
    None
}

fn apply_meta(
    fact: &mut RouteFact,
    route_attr: &RouteAttr,
    meta: &syn::meta::ParseNestedMeta<'_>,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) {
    let key = meta.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();

    if key == route_attr.path_key_name() {
        if let Some(v) = literal_value(meta) {
            set_once(&mut fact.path, "path", v, loc, diags);
        }
        return;
    }
    if key == route_attr.method_key_name() {
        if let Some(v) = literal_value(meta) {
            set_once(&mut fact.method, "method", v, loc, diags);
        }
        return;
    }
    if key == route_attr.request_key_name() {
        if let Some(v) = type_like_value(meta) {
            set_once(&mut fact.overrides.body, "request", v, loc, diags);
        }
        return;
    }
    if key == route_attr.response_key_name() {
        if let Some(v) = type_like_value(meta) {
            set_once(&mut fact.overrides.response, "response", v, loc, diags);
        }
        return;
    }

    match key.as_str() {
        "fn_name" => {
            if let Some(v) = literal_value(meta) {
                set_once(&mut fact.overrides.fn_name, "fn_name", v, loc, diags);
            }
        }
        "query_key" => {
            if let Some(v) = literal_value(meta) {
                set_once(&mut fact.overrides.query_key, "query_key", v, loc, diags);
            }
        }
        "skip" => {
            if value_of(meta).is_some() {
                fact.overrides.skip = true;
            }
        }
        "prefix" => {
            let on = meta
                .value()
                .ok()
                .and_then(|v| v.parse::<syn::LitBool>().ok())
                .map(|b| b.value)
                .unwrap_or(true);
            set_once(&mut fact.overrides.prefix, "prefix", on, loc, diags);
        }
        "params" => {
            let _ = meta.parse_nested_meta(|param_meta| {
                apply_param_meta(&mut fact.overrides.params, &param_meta, loc, diags);
                Ok(())
            });
        }
        _ => {}
    }
}

fn apply_param_meta(
    params: &mut HashMap<String, ParamOverride>,
    param_meta: &syn::meta::ParseNestedMeta<'_>,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) {
    let Some(name) = param_meta.path.get_ident().map(|i| i.to_string()) else {
        return;
    };
    let entry = params.entry(name).or_default();
    let _ = param_meta.parse_nested_meta(|field_meta| {
        let field_key = field_meta.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
        match field_key.as_str() {
            "type" => {
                if let Some(v) = type_like_value(&field_meta) {
                    set_once(&mut entry.ty, "params.type", v, loc, diags);
                }
            }
            "rename" => {
                if let Some(v) = literal_value(&field_meta) {
                    set_once(&mut entry.rename, "params.rename", v, loc, diags);
                }
            }
            _ => {}
        }
        Ok(())
    });
}

/// `key = "..."` only — the right shape for path templates, HTTP methods,
/// and names, none of which are ever written as a bare Rust path.
fn literal_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Option<String> {
    meta.value().ok()?.parse::<syn::LitStr>().ok().map(|lit| lit.value())
}

/// `key = "..."` or `key = SomePath` — a string literal taken verbatim, or a
/// bare Rust path reduced to its last segment, the same final-segment
/// matching `map_type` uses. Fits type-shaped values like `response` and
/// `request`, where `Response = PatientView` names a type without quoting it.
fn literal_or_path_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Option<String> {
    let value = meta.value().ok()?;
    if let Ok(lit) = value.parse::<syn::LitStr>() {
        return Some(lit.value());
    }
    let path: syn::Path = value.parse().ok()?;
    path.segments.last().map(|s| s.ident.to_string())
}

/// Either override spelling for a type-shaped key: the shorthand
/// `key = "..."` / `key = Path`, or the nested `key(type = "...")`.
fn type_like_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Option<String> {
    if let Some(v) = literal_or_path_value(meta) {
        return Some(v);
    }
    let mut nested = None;
    let _ = meta.parse_nested_meta(|inner| {
        if inner.path.is_ident("type") {
            nested = literal_or_path_value(&inner);
        }
        Ok(())
    });
    nested
}

fn value_of(meta: &syn::meta::ParseNestedMeta<'_>) -> Option<String> {
    if let Ok(lit) = meta.value().and_then(|v| v.parse::<syn::LitStr>()) {
        return Some(lit.value());
    }
    meta.path.get_ident().map(|i| i.to_string())
}

fn set_once<T>(slot: &mut Option<T>, key: &str, value: T, loc: &SourceLoc, diags: &mut Diagnostics) {
    if slot.is_some() {
        diags.push(
            Diagnostic::new(loc.clone(), format!("`{key}` is overridden twice on this route"))
                .with_help("a route can only set this once; remove the duplicate"),
        );
        return;
    }
    *slot = Some(value);
}

/// Turns every discovered `FnItem` into a [`Route`] and inserts it into
/// `reg`.
///
/// Each fn's [`RouteFact`] comes from up to two independent sources: its own
/// attributes, and (when [`Axum::routers_enabled`]) a fact
/// [`facts_from_routers`] resolved for it from a real `Router` builder
/// chain elsewhere in the scan. With both, [`merge_router_fact`] fills in
/// whichever piece the attribute left unset and errors on a genuine
/// disagreement; with only the router's, it is used on its own only under
/// [`Discover::All`] -- [`Discover::Annotated`] still requires the
/// attribute to select what's exposed. A fn with neither is skipped, as is
/// one whose fact is marked `skip`.
///
/// From there, each fn goes through, in order: computing its `group` from
/// its file's directory; resolving its declared path and method, with the
/// configured prefix applied; classifying its extractors and zipping its
/// path captures; excluding a streaming response with a warning instead of
/// inferring one; inferring its response and error types; checking the
/// `method` key reservation on an `any` route; recording its query-array
/// convention; and finally building and inserting the `Route`, diagnosing a
/// clash with either an existing route of the same method and path, or an
/// existing route with the same name in the same group.
pub fn collect_routes(
    fns: &[FnItem],
    reg: &mut Registry,
    cfg: &Config,
    ctx: &LowerCtx,
    diags: &mut Diagnostics,
) {
    let axum = cfg.axum_config().expect("axum configured");
    let router_facts =
        if axum.routers_enabled() { facts_from_routers(fns, axum, diags) } else { Vec::new() };
    let mut claimed: HashMap<(Vec<String>, String), SourceLoc> = HashMap::new();

    for f in fns {
        let attr_fact = fact_from_attrs(f, axum, diags);
        let router_fact = router_facts.iter().find(|rf| handler_matches(rf, f));

        let fact = match (attr_fact, router_fact) {
            (None, None) => continue,
            (None, Some(rf)) if axum.discovery() == Discover::All => rf.clone(),
            (None, Some(_)) => continue,
            (Some(af), None) => af,
            (Some(af), Some(rf)) => match merge_router_fact(af, rf, axum, diags) {
                Some(merged) => merged,
                None => continue,
            },
        };
        if fact.overrides.skip {
            continue;
        }

        let Some(group) = route_group(&f.file, axum, cfg.scans(), &fact.loc, diags) else { continue };

        let (Some(declared_path), Some(method_str)) = (&fact.path, &fact.method) else {
            if axum.routers_enabled() {
                diags.push(
                    Diagnostic::new(
                        fact.loc.clone(),
                        format!("`{}` has no path or method to build a route from", f.item.sig.ident),
                    )
                    .with_help(
                        "give #[ts_route] its own path and method, or register this handler \
                         in a router chain scanned via .routers(true)",
                    ),
                );
            }
            continue;
        };
        let Some(method) = parse_method(method_str) else {
            diags.push(Diagnostic::new(
                fact.loc.clone(),
                format!("route method `{method_str}` is not a method axum recognizes"),
            ));
            continue;
        };

        let effective_path = apply_prefix(declared_path, &fact.overrides, axum);
        let Some(segs) = path::parse_template(&effective_path, &fact.loc, diags) else { continue };

        let extracted = bind::classify(&f.item.sig, axum, ctx, &fact.loc, diags);
        let params =
            bind::zip_params(&segs, extracted.path.as_ref(), &fact.overrides, reg, ctx, &fact.loc, diags);

        if is_streaming_response(&f.item.sig) {
            diags.push(Diagnostic::warning(
                fact.loc.clone(),
                format!(
                    "`{}` streams its response and is excluded from the generated client",
                    f.item.sig.ident
                ),
            ));
            continue;
        }

        let Some((response, error)) =
            bind::infer_response(&f.item.sig, &fact.overrides, axum, reg, ctx, &fact.loc, diags)
        else {
            continue;
        };

        if method == Method::Any && names_the_reserved_method_key(&params, &extracted) {
            diags.push(
                Diagnostic::new(
                    fact.loc.clone(),
                    "`method` is reserved on an `any` route's argument object",
                )
                .with_help(
                    "rename the path capture, query, or body binding named `method` \
                     so it doesn't collide with the reserved key",
                ),
            );
            continue;
        }

        let query_arrays = query_arrays_for(&extracted, axum, reg, &fact.loc, diags);

        // An explicit `request = "..."` names the type callers should send, which is
        // usually the real domain type (`NewPatient`) rather than the literal body
        // extractor's type (a validation-friendly wire form like `NewPatientSerde`,
        // often invisible to the source scan the same way `#[ts_route]`'s own
        // generated newtypes are) -- same override the response gets, applied to
        // whichever typed body extractor was actually found, if any.
        let body = extracted.body.map(|(_, b)| b);
        let body = match (&fact.overrides.body, body) {
            (Some(s), Some(RouteBody::Typed(_))) => {
                Some(RouteBody::Typed(bind::type_override(s, ctx, &fact.loc, diags)))
            }
            (_, b) => b,
        };

        let ts_name = fact
            .overrides
            .fn_name
            .clone()
            .unwrap_or_else(|| RenameRule::Camel.apply_to_field(&f.item.sig.ident.to_string()));

        let key_group = match axum.client_style() {
            Client::Flat => Vec::new(),
            _ => group.clone(),
        };
        let key = (key_group, ts_name.clone());
        if let Some(existing) = claimed.get(&key) {
            diags.push(
                Diagnostic::new(
                    fact.loc.clone(),
                    format!("duplicate route name `{ts_name}` in group [{}]", group.join(".")),
                )
                .with_help(format!(
                    "already declared at {}:{}; use #[ts_route(fn_name = \"...\")] to rename one of them",
                    existing.file.display(),
                    existing.line
                )),
            );
            continue;
        }

        let route = Route {
            method,
            path: path::render(&segs),
            ts_name,
            group,
            params,
            query: extracted.query.map(|(_, ty)| ty),
            body,
            response,
            error,
            query_arrays,
            docs: Vec::new(),
            loc: fact.loc.clone(),
        };

        claimed.insert(key, route.loc.clone());

        if let Err(clash) = reg.insert_route(route) {
            diags.push(
                Diagnostic::new(clash.incoming.clone(), format!("duplicate route `{}`", clash.key))
                    .with_help(format!(
                        "already declared at {}:{}; use #[ts_route(fn_name = \"...\")] to rename one \
                         of them, or change its path or method so it doesn't collide",
                        clash.existing.file.display(),
                        clash.existing.line
                    )),
            );
        }
    }
}

/// Whether a router-sourced fact's resolved handler identity names `f`: its
/// bare fn name, and its enclosing module's full path -- the same
/// comparison [`crate::axum::router`] uses to decide which scanned `fn` a
/// `.route(...)` handler reference means. Comparing the full path, not just
/// its last segment, is what keeps two same-named fns in different modules
/// that happen to share a last segment from both matching one fact.
fn handler_matches(rf: &RouteFact, f: &FnItem) -> bool {
    let Some((name, module)) = &rf.handler else { return false };
    f.item.sig.ident == name.as_str() && f.module_path == *module
}

/// The path `collect_routes` ends up building a route from: `declared_path`
/// untouched when `overrides.prefix == Some(false)` opts this route out, or
/// when no prefix is configured for it at all, otherwise `declared_path`
/// with the configured [`Axum::prefix`] prepended.
fn apply_prefix(declared_path: &str, overrides: &Overrides, axum: &Axum) -> String {
    match overrides.prefix {
        Some(false) => declared_path.to_string(),
        _ => match axum.prefix_for(declared_path) {
            Some(prefix) => format!("/{}{}", prefix.trim_matches('/'), declared_path),
            None => declared_path.to_string(),
        },
    }
}

/// Combines an attribute fact with the router-sourced fact independently
/// resolved for the same handler: a piece missing from the attribute (most
/// often its path and method, on a bare `#[ts_route]`) is filled in from the
/// router; a piece both sides declare is kept only when they agree. Two
/// sources naming different methods or paths for the same handler is an
/// error naming both sites -- this drift check is the reason to run both
/// sources at all, so it is treated the same as any other fatal mismatch:
/// the route is dropped (`None`) rather than guessing which side is right.
///
/// A router-sourced path is already composed through whatever real
/// `.nest(...)` chain reached it, while the attribute's is always the bare
/// value as written -- comparing them straight would see a configured
/// prefix as drift every time. So the attribute's path is run through the
/// same [`apply_prefix`] `collect_routes` applies downstream before either
/// side is compared; on agreement, `attr.path` is left at its original,
/// un-prefixed value so that downstream step still runs, instead of being
/// overwritten with the router's already-prefixed one and prefixed twice.
fn merge_router_fact(mut attr: RouteFact, router: &RouteFact, axum: &Axum, diags: &mut Diagnostics) -> Option<RouteFact> {
    let mut conflict = None;

    match (&attr.method, &router.method) {
        (Some(a), Some(r)) if !a.eq_ignore_ascii_case(r) => conflict = Some(("method", a.clone(), r.clone())),
        (None, Some(r)) => attr.method = Some(r.clone()),
        _ => {}
    }
    match (&attr.path, &router.path) {
        (Some(a), Some(r)) => {
            let normalized = apply_prefix(a, &attr.overrides, axum);
            if normalized.trim_matches('/') != r.trim_matches('/') {
                conflict = conflict.or(Some(("path", a.clone(), r.clone())));
            }
        }
        (None, Some(r)) => attr.path = Some(r.clone()),
        _ => {}
    }

    if let Some((field, a, r)) = conflict {
        diags.push(
            Diagnostic::new(
                attr.loc.clone(),
                format!(
                    "`{field}` disagrees between the #[ts_route] attribute (`{a}`) and the \
                     router registration (`{r}`)"
                ),
            )
            .with_help(format!("also registered at {}:{}", router.loc.file.display(), router.loc.line)),
        );
        return None;
    }
    Some(attr)
}

/// The HTTP method an attribute's `method = "..."` value names, matched
/// case-insensitively. `None` for anything axum doesn't recognize.
fn parse_method(s: &str) -> Option<Method> {
    Some(match s.to_ascii_uppercase().as_str() {
        "GET" => Method::Get,
        "POST" => Method::Post,
        "PUT" => Method::Put,
        "PATCH" => Method::Patch,
        "DELETE" => Method::Delete,
        "HEAD" => Method::Head,
        "OPTIONS" => Method::Options,
        "TRACE" => Method::Trace,
        "ANY" => Method::Any,
        _ => return None,
    })
}

/// The route's `group`: the handler file's parent directory, relative to
/// whichever configured scan root it falls under, with a configured
/// `Client::Modules { strip }` segment dropped and every remaining
/// component camelCased.
///
/// `apply_to_field` only treats `_` as a word separator, matching serde's
/// own assumption that its input is already `snake_case` -- a directory
/// name is just as likely to be `kebab-case`, so a literal `-` is folded
/// into `_` first. `None` when a segment can't become a legal TypeScript
/// member name even after that -- a leading digit survives any casing --
/// and the caller treats that as a fatal error for this route.
fn route_group(
    file: &Path,
    cfg: &Axum,
    scans: &[PathBuf],
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Option<Vec<String>> {
    let parent = file.parent().unwrap_or(file);
    let mut rel: Vec<String> = scans
        .iter()
        .find(|root| parent.starts_with(root.as_path()))
        .and_then(|root| parent.strip_prefix(root).ok())
        .map(|p| {
            p.components()
                .filter_map(|c| match c {
                    std::path::Component::Normal(s) => s.to_str().map(str::to_string),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();

    if let Client::Modules { strip: Some(s) } = cfg.client_style() {
        if rel.first().map(String::as_str) == Some(s.as_str()) {
            rel.remove(0);
        }
    }

    let mut group = Vec::with_capacity(rel.len());
    for raw in &rel {
        let cased = RenameRule::Camel.apply_to_field(&raw.replace('-', "_"));
        let starts_ok = cased.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
        if !starts_ok {
            diags.push(
                Diagnostic::new(
                    loc.clone(),
                    format!("directory `{raw}` can't become a TypeScript identifier"),
                )
                .with_help(
                    "rename the directory, or move the handler so its group comes from a \
                     different path",
                ),
            );
            return None;
        }
        group.push(cased);
    }
    Some(group)
}

/// Whether a handler's (possibly `Result`-wrapped) return type is a
/// streaming extractor -- `Sse<_>` or `WebSocketUpgrade` -- which excludes
/// the route from the client instead of going through [`bind::infer_response`].
/// Unwraps the return type the same way `infer_response` does internally,
/// via its own `split_result`/`unwrap_response_body`/`final_segment`.
fn is_streaming_response(sig: &syn::Signature) -> bool {
    let syn::ReturnType::Type(_, ty) = &sig.output else { return false };
    let (ok, _) = bind::split_result(ty);
    match bind::unwrap_response_body(&ok) {
        bind::Unwrapped::Void => false,
        bind::Unwrapped::Type(resolved) => {
            matches!(bind::final_segment(&resolved).as_deref(), Some("Sse") | Some("WebSocketUpgrade"))
        }
    }
}

/// Whether `"method"` -- the reserved key on an `any` route's argument
/// object -- is claimed by a path capture, the query binding, or the body
/// binding.
fn names_the_reserved_method_key(params: &[RouteParam], extracted: &bind::Extracted) -> bool {
    params.iter().any(|p| p.name == "method")
        || extracted.query.as_ref().is_some_and(|(name, _)| name == "method")
        || extracted.body.as_ref().is_some_and(|(name, _)| name == "method")
}

/// The route's [`Route::query_arrays`] convention: `None` unless
/// the query extractor's type resolves to a registered struct with at least
/// one `Vec<_>`-shaped field -- `serde_urlencoded` doesn't round-trip an
/// array the way JSON would, so that divergence needs either a warning or a
/// configured convention.
fn query_arrays_for(
    extracted: &bind::Extracted,
    cfg: &Axum,
    reg: &Registry,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Option<String> {
    let Some((_, Ty::Ref { name, .. })) = &extracted.query else { return None };
    let decl = reg.lookup(name)?;
    let Body::Struct(fields) = &decl.body else { return None };

    let array_fields: Vec<&str> = fields
        .iter()
        .filter(|f| matches!(f.ty, Ty::Array(_)))
        .map(|f| f.ts_name.as_str())
        .collect();
    if array_fields.is_empty() {
        return None;
    }

    match cfg.query_arrays_style() {
        QueryArrays::Warn => {
            diags.push(Diagnostic::warning(
                loc.clone(),
                format!(
                    "query field(s) {} are array-typed; serde_urlencoded doesn't round-trip \
                     these the way JSON would",
                    array_fields.join(", ")
                ),
            ));
            None
        }
        QueryArrays::Csv => Some("csv".to_string()),
        QueryArrays::Repeated => Some("repeated".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fn_item(src: &str) -> crate::scan::FnItem {
        crate::scan::FnItem {
            item: syn::parse_str(src).expect("fn must parse"),
            module_path: "crate::handlers::patient".into(),
            file: "h.rs".into(),
        }
    }

    fn facts(src: &str, cfg: &Axum) -> (Option<RouteFact>, Diagnostics) {
        let mut d = Diagnostics::default();
        let f = fact_from_attrs(&fn_item(src), cfg, &mut d);
        (f, d)
    }

    fn with_api_attr() -> Axum {
        Axum::new().route_attr(
            RouteAttr::new("api")
                .path_key("Route")
                .method_key("Method")
                .request_key("Request")
                .response_key("Response"),
        )
    }

    #[test]
    fn an_unannotated_fn_produces_no_fact() {
        let (fact, _) = facts("async fn plain() {}", &Axum::new());
        assert!(fact.is_none());
    }

    #[test]
    fn a_foreign_attribute_yields_method_path_and_response() {
        let (fact, d) = facts(
            r#"#[api(Route = "/patient/view/{id}", Method = "GET", Response = "Vec<PatientView>")]
               async fn get_patient() {}"#,
            &with_api_attr(),
        );
        let fact = fact.expect("fact");
        assert!(!d.has_errors());
        assert_eq!(fact.method.as_deref(), Some("GET"));
        assert_eq!(fact.path.as_deref(), Some("/patient/view/{id}"));
        assert_eq!(fact.overrides.response.as_deref(), Some("Vec<PatientView>"));
    }

    #[test]
    fn a_bare_path_value_is_read_like_a_string_literal() {
        let (fact, _) = facts(
            r#"#[api(Route = "/a", Method = "GET", Response = PatientView)] async fn h() {}"#,
            &with_api_attr(),
        );
        assert_eq!(fact.unwrap().overrides.response.as_deref(), Some("PatientView"));
    }

    #[test]
    fn a_bare_ts_route_has_no_method_or_path() {
        let (fact, d) = facts("#[ts_route] async fn h() {}", &Axum::new());
        let fact = fact.expect("bare attribute still marks the route");
        assert!(!d.has_errors());
        assert!(fact.method.is_none() && fact.path.is_none());
    }

    #[test]
    fn both_override_spellings_are_accepted() {
        let (shorthand, _) = facts(r#"#[ts_route(response = "Blob")] async fn h() {}"#, &Axum::new());
        let (nested, _) =
            facts(r#"#[ts_route(response(type = "Blob"))] async fn h() {}"#, &Axum::new());
        assert_eq!(shorthand.unwrap().overrides.response.as_deref(), Some("Blob"));
        assert_eq!(nested.unwrap().overrides.response.as_deref(), Some("Blob"));
    }

    #[test]
    fn overriding_one_piece_in_both_spellings_is_an_error() {
        let (_, d) = facts(
            r#"#[ts_route(response = "A", response(type = "B"))] async fn h() {}"#,
            &Axum::new(),
        );
        assert!(d.has_errors());
        assert!(d.iter().any(|x| x.message.contains("response")));
    }

    #[test]
    fn request_is_an_alias_for_body_and_params_carry_nested_overrides() {
        let (fact, _) = facts(
            r#"#[ts_route(request = "NewScheme", params(id(rename = "patientId")))] async fn h() {}"#,
            &Axum::new(),
        );
        let o = fact.unwrap().overrides;
        assert_eq!(o.body.as_deref(), Some("NewScheme"));
        assert_eq!(o.params["id"].rename.as_deref(), Some("patientId"));
    }

    #[test]
    fn skip_marks_the_route_hidden() {
        let (fact, _) = facts("#[ts_route(skip)] async fn h() {}", &Axum::new());
        assert!(fact.unwrap().overrides.skip);
    }
}

#[cfg(test)]
mod collect_routes_tests {
    use super::*;
    use crate::axum::config::{Client, QueryArrays};
    use crate::config::Config;
    use crate::ir::Route;
    use crate::lower::LowerCtx;
    use crate::scan::scan_roots;

    /// Writes `files` (path relative to the crate's `src`, contents) into a
    /// temp crate and scans it.
    ///
    /// `scan.rs`'s `Item::Mod` arm resolves `mod NAME;` only by looking for a
    /// file literally named `NAME.rs` or `NAME/mod.rs` next to the current
    /// file -- it never reads `#[path = "..."]`, so a fixture can't reach an
    /// arbitrary directory through a `mod` chain. It also can't build a
    /// literal nested `pub mod` chain, because several directories used below
    /// (`patient-notes`, `2fa`) aren't legal Rust identifiers and `mod`
    /// syntax requires one. Instead, each file's own parent directory is
    /// used as its own scan root: with no `lib.rs`/`main.rs` present,
    /// `entry_points` picks up every `.rs` file found directly inside a
    /// directory with no `mod` declaration needed at all, so `FnItem.file`
    /// ends up carrying the exact directory names written here.
    fn collect(files: &[(&str, &str)], axum: Axum) -> (Vec<Route>, Diagnostics, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();

        let mut roots: Vec<std::path::PathBuf> = Vec::new();
        for (rel, body) in files {
            let path = src.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
            let parent = path.parent().unwrap().to_path_buf();
            if !roots.contains(&parent) {
                roots.push(parent);
            }
        }

        let cfg = Config::new().scan(&src).output(dir.path().join("bindings")).axum(axum);
        let mut diags = Diagnostics::default();
        let scan = scan_roots(&roots, &LowerCtx::default(), &mut diags);
        let (mut reg, fns) = scan.split();
        collect_routes(&fns, &mut reg, &cfg, &LowerCtx::default(), &mut diags);
        (reg.routes().to_vec(), diags, dir)
    }

    fn modules(strip: &str) -> Axum {
        Axum::new().client(Client::Modules { strip: Some(strip.to_string()) })
    }

    fn only(files: &[(&str, &str)], axum: Axum) -> Route {
        let (routes, d, _dir) = collect(files, axum);
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        routes.into_iter().next().expect("one route")
    }

    const GET_PATIENT: &str = r#"#[ts_route(path = "/patient/{id}", method = "GET")]
pub async fn get_patient(Path(id): Path<i64>) -> Json<PatientView> { todo!() }"#;

    #[test]
    fn a_group_comes_from_the_directory_with_the_strip_segment_removed() {
        let r = only(&[("handlers/patient_view/get_patient.rs", GET_PATIENT)], modules("handlers"));
        assert_eq!(r.group, vec!["patientView"]);
        assert_eq!(r.ts_name, "getPatient");
    }

    #[test]
    fn a_handler_in_the_stripped_root_has_no_group() {
        let r = only(&[("handlers/monitor.rs", GET_PATIENT)], modules("handlers"));
        assert!(r.group.is_empty());
    }

    #[test]
    fn a_directory_that_is_not_a_valid_identifier_is_camelcased_into_one() {
        let r = only(&[("handlers/patient-notes/get_patient.rs", GET_PATIENT)], modules("handlers"));
        assert_eq!(r.group, vec!["patientNotes"]);
    }

    #[test]
    fn a_group_that_cannot_become_an_identifier_is_an_error() {
        let (routes, d, _dir) =
            collect(&[("handlers/2fa/get_patient.rs", GET_PATIENT)], modules("handlers"));
        assert!(routes.is_empty());
        assert!(d.has_errors());
        assert!(d.iter().next().unwrap().message.contains("2fa"));
    }

    #[test]
    fn the_configured_prefix_is_applied_except_to_exempt_paths() {
        let axum = modules("handlers").prefix("v2").prefix_exempt(["/auth/*"]);
        let prefixed = only(&[("handlers/p/get_patient.rs", GET_PATIENT)], axum.clone());
        assert_eq!(prefixed.path, "/v2/patient/{id}");

        let exempt_src = r#"#[ts_route(path = "/auth/refresh", method = "POST")]
pub async fn refresh() -> Json<PatientView> { todo!() }"#;
        let exempt = only(&[("handlers/auth/refresh.rs", exempt_src)], axum);
        assert_eq!(exempt.path, "/auth/refresh");
    }

    #[test]
    fn a_per_route_prefix_opt_out_wins_over_the_config() {
        let src = r#"#[ts_route(path = "/hello", method = "GET", prefix = false)]
pub async fn hello() -> Json<PatientView> { todo!() }"#;
        let r = only(&[("handlers/h/hello.rs", src)], modules("handlers").prefix("v2"));
        assert_eq!(r.path, "/hello");
    }

    #[test]
    fn annotated_discovery_skips_unannotated_handlers_and_skip_hides_a_route() {
        let hidden = r#"#[ts_route(path = "/c", method = "GET", skip)]
pub async fn c() -> Json<PatientView> { todo!() }"#;
        let (routes, _d, _dir) = collect(
            &[
                ("handlers/a/get_patient.rs", GET_PATIENT),
                ("handlers/b/plain.rs", "pub async fn plain() {}"),
                ("handlers/c/c.rs", hidden),
            ],
            modules("handlers"),
        );
        assert_eq!(routes.iter().map(|r| r.ts_name.as_str()).collect::<Vec<_>>(), vec!["getPatient"]);
    }

    #[test]
    fn two_handlers_colliding_in_one_group_is_an_error_naming_both() {
        let other = r#"#[ts_route(path = "/other/{id}", method = "GET")]
pub async fn get_patient(Path(id): Path<i64>) -> Json<PatientView> { todo!() }"#;
        let (_, d, _dir) = collect(
            &[("handlers/p/a.rs", GET_PATIENT), ("handlers/p/b.rs", other)],
            modules("handlers"),
        );
        assert!(d.has_errors());
        assert!(d.iter().any(|x| x.help.as_deref().unwrap_or_default().contains("rename")));
    }

    #[test]
    fn two_handlers_colliding_under_flat_mode_is_also_an_error() {
        let other = r#"#[ts_route(path = "/other/{id}", method = "GET")]
pub async fn get_patient(Path(id): Path<i64>) -> Json<PatientView> { todo!() }"#;
        let (_, d, _dir) = collect(
            &[("handlers/a/x.rs", GET_PATIENT), ("handlers/b/y.rs", other)],
            Axum::new().client(Client::Flat),
        );
        assert!(d.has_errors());
        assert!(d.iter().any(|x| x.help.as_deref().unwrap_or_default().contains("rename")));
    }

    #[test]
    fn a_streaming_response_route_is_excluded_with_a_warning() {
        let src = r#"#[ts_route(path = "/events", method = "GET")]
pub async fn events() -> Sse<EventStream> { todo!() }"#;
        let (routes, d, _dir) = collect(&[("handlers/e/events.rs", src)], modules("handlers"));
        assert!(routes.is_empty());
        assert!(!d.has_errors(), "a streaming route must not fail the build");
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn a_vec_field_in_a_query_type_warns_about_the_urlencoded_divergence() {
        let src = r#"#[derive(ts_typegen::Ts)]
pub struct Filter { pub ids: Vec<i64> }

#[ts_route(path = "/search", method = "GET")]
pub async fn search(Query(filter): Query<Filter>) -> Json<PatientView> { todo!() }"#;
        let (routes, d, _dir) = collect(&[("handlers/s/search.rs", src)], modules("handlers"));
        assert_eq!(routes.len(), 1, "the route is still generated");
        assert!(!d.has_errors());
        assert!(d.iter().any(|x| x.message.contains("ids")), "the field must be named");
    }

    #[test]
    fn a_capture_named_method_on_an_any_route_is_an_error() {
        let src = r#"#[ts_route(path = "/proxy/{method}", method = "ANY")]
pub async fn proxy(Path(method): Path<String>) -> Json<PatientView> { todo!() }"#;
        let (routes, d, _dir) = collect(&[("handlers/p/proxy.rs", src)], modules("handlers"));
        assert!(routes.is_empty());
        assert!(d.has_errors(), "`method` is the reserved key on an ANY route");
        assert!(d.iter().next().unwrap().help.as_deref().unwrap().contains("rename"));
    }

    #[test]
    fn a_request_override_replaces_the_bodys_literal_wire_type() {
        // `NewPatientSerde` stands in for a validation-friendly wire form a macro
        // generates at expansion time -- invisible to the source scan, and not
        // `#[derive(Ts)]`'d here either, on purpose: the override must win without
        // needing the literal extractor type to resolve to anything.
        let src = r#"#[derive(ts_typegen::Ts)]
pub struct NewPatient { pub name: String }

#[ts_route(path = "/patient", method = "POST", request = "NewPatient")]
pub async fn create_patient(Json(p): Json<NewPatientSerde>) -> Json<NewPatient> { todo!() }"#;
        let r = only(&[("handlers/p/create_patient.rs", src)], modules("handlers"));
        match r.body {
            Some(RouteBody::Typed(Ty::Ref { name, .. })) => assert_eq!(name, "NewPatient"),
            other => panic!("expected a Typed(Ref(\"NewPatient\")) body, got {other:?}"),
        }
    }

    #[test]
    fn a_request_override_does_not_apply_to_a_fixed_body_shape() {
        // `request = "..."` names a JSON/Form payload's type; it has nothing to say
        // about a `Multipart` body, which has no type to override in the first place.
        let src = r#"#[derive(ts_typegen::Ts)]
pub struct NewPatient { pub name: String }

#[ts_route(path = "/upload", method = "POST", request = "NewPatient")]
pub async fn upload(m: Multipart) -> Json<NewPatient> { todo!() }"#;
        let r = only(&[("handlers/u/upload.rs", src)], modules("handlers"));
        assert!(matches!(r.body, Some(RouteBody::FormData)));
    }

    #[test]
    fn csv_query_arrays_record_the_convention_instead_of_warning() {
        let src = r#"#[derive(ts_typegen::Ts)]
pub struct Filter { pub ids: Vec<i64> }

#[ts_route(path = "/search", method = "GET")]
pub async fn search(Query(filter): Query<Filter>) -> Json<PatientView> { todo!() }"#;
        let axum = modules("handlers").query_arrays(QueryArrays::Csv);
        let (routes, d, _dir) = collect(&[("handlers/s/search.rs", src)], axum);
        assert_eq!(routes.len(), 1);
        assert!(d.is_empty(), "Csv settles it, so nothing is reported: {:?}", d.iter().collect::<Vec<_>>());
    }
}

/// Covers [`merge_router_fact`]'s drift check between the two fact sources:
/// an attribute's declared path and method against a router-resolved fact
/// for the same handler.
#[cfg(test)]
mod collect_both_tests {
    use super::*;
    use crate::config::Config;
    use crate::ir::Route;
    use crate::lower::LowerCtx;
    use crate::scan::scan_roots;

    /// Writes `attr_src` and `router_src` into one file and scans it with
    /// routers enabled and, when given, a configured prefix.
    fn collect_both_with_prefix_diag(
        attr_src: &str,
        router_src: &str,
        prefix: Option<&str>,
    ) -> (Vec<Route>, Diagnostics) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("app.rs"), format!("{attr_src}\n{router_src}\n")).unwrap();

        let mut axum = Axum::new().routers(true);
        if let Some(p) = prefix {
            axum = axum.prefix(p);
        }
        let cfg = Config::new().scan(&src).output(dir.path().join("bindings")).axum(axum);
        let mut diags = Diagnostics::default();
        let scan = scan_roots(std::slice::from_ref(&src), &LowerCtx::default(), &mut diags);
        let (mut reg, fns) = scan.split();
        collect_routes(&fns, &mut reg, &cfg, &LowerCtx::default(), &mut diags);
        (reg.routes().to_vec(), diags)
    }

    fn collect_both_diag(attr_src: &str, router_src: &str) -> (Vec<Route>, Diagnostics) {
        collect_both_with_prefix_diag(attr_src, router_src, None)
    }

    fn collect_both(attr_src: &str, router_src: &str) -> Vec<Route> {
        let (routes, d) = collect_both_diag(attr_src, router_src);
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        routes
    }

    fn collect_both_with_prefix(attr_src: &str, router_src: &str, prefix: &str) -> Vec<Route> {
        let (routes, d) = collect_both_with_prefix_diag(attr_src, router_src, Some(prefix));
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        routes
    }

    #[test]
    fn agreeing_sources_produce_one_route() {
        let routes = collect_both(
            r#"#[ts_route(path = "/a", method = "GET")] async fn h() -> Json<A> {}"#,
            "fn app() -> Router { Router::new().route(\"/a\", get(h)) }",
        );
        assert_eq!(routes.len(), 1);
    }

    #[test]
    fn disagreeing_sources_are_an_error_naming_both_sites() {
        let (routes, d) = collect_both_diag(
            r#"#[ts_route(path = "/a", method = "GET")] async fn h() -> Json<A> {}"#,
            "fn app() -> Router { Router::new().route(\"/b\", get(h)) }",
        );
        assert!(routes.is_empty());
        assert!(d.has_errors());
        let m = &d.iter().next().unwrap().message;
        assert!(m.contains("/a") && m.contains("/b"), "both paths must appear: {m}");
    }

    #[test]
    fn the_prefix_is_normalised_before_comparing() {
        let routes = collect_both_with_prefix(
            r#"#[ts_route(path = "/a", method = "GET")] async fn h() -> Json<A> {}"#,
            "fn inner() -> Router { Router::new().route(\"/a\", get(h)) }
fn app() -> Router { Router::new().nest(\"/v2\", inner()) }",
            "v2",
        );
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0].path, "/v2/a");
    }
}
