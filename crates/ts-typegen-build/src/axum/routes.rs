//! Reading route metadata off a handler fn's attributes.
//!
//! A handler is discovered either through the built-in `#[ts_route(...)]` or
//! through a foreign attribute registered with [`Axum::route_attr`], e.g.
//! `#[api(Route = "...", Method = "...")]`. Both are read the same way here:
//! [`Axum::route_attrs`] already carries `ts_route` by default, so matching
//! `attr.path()` against each configured [`RouteAttr`] name covers both.

use std::collections::HashMap;

use crate::axum::config::{Axum, RouteAttr};
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::SourceLoc;
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
    /// Where the attribute was found.
    pub loc: SourceLoc,
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
