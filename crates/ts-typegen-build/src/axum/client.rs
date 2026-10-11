//! Emitting the generated TypeScript client: the sender contract in
//! `sender.ts`, and the per-route type map in `routes.ts`.

use std::collections::BTreeSet;

use crate::config::Config;
use crate::emit::emit_ty;
use crate::ir::{Field, Registry, Route, RouteBody, Ty};
use crate::render::MARKER;

/// Mirrors [`crate::render`]'s private `banner`: the marker line plus any
/// configured header, which every generated file opens with.
fn banner(cfg: &Config) -> String {
    let mut out = String::from(MARKER);
    out.push('\n');
    let header = cfg.header_lines();
    if !header.is_empty() {
        out.push_str(header.trim_end_matches('\n'));
        out.push('\n');
    }
    out
}

/// The fixed sender contract every generated client imports: the `Method`
/// union, the `RouteRequest`/`RouteResult` shapes, and the `Sender`
/// interface a project implements against its own HTTP stack. This text is
/// config-independent -- no `Route` ever changes it.
const SENDER_CONTRACT: &str = r#"export type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD" | "OPTIONS";

export interface RouteRequest<R = unknown, E = unknown> {
  readonly method: Method;
  readonly path: string;
  readonly route: string;
  readonly query?: Record<string, unknown>;
  readonly body?: unknown;
  readonly __response?: R;
  readonly __error?: E;
}

export type RouteResult<T, E> =
  | { readonly ok: true; readonly data: T; readonly status: number }
  | { readonly ok: false; readonly errors: E; readonly status: number };

export interface Sender<O = never> {
  send<R, E>(req: RouteRequest<R, E>, opts?: O): Promise<RouteResult<R, E>>;
}
"#;

/// Renders `bindings/sender.ts`: the generated-file banner plus the literal
/// sender contract.
pub fn render_sender(cfg: &Config) -> String {
    let mut out = banner(cfg);
    out.push_str(SENDER_CONTRACT);
    out
}

/// Renders `bindings/routes.ts`: the IR as a type map keyed by
/// `"METHOD /template"`, one entry per route with its params, query, body,
/// response and error pieces.
pub fn render_routes(reg: &Registry, cfg: &Config) -> String {
    let mut refs = BTreeSet::new();
    let mut entries = Vec::new();

    for route in reg.routes() {
        entries.push(render_entry(route, cfg, &mut refs));
    }

    let mut imports = Vec::new();
    for name in &refs {
        imports.push(format!(
            "import type {{ {name} }} from \"./{name}{}\";",
            cfg.extension()
        ));
    }

    let mut out = banner(cfg);
    if !imports.is_empty() {
        out.push_str(&imports.join("\n"));
        out.push_str("\n\n");
    }
    out.push_str("export type Routes = {\n");
    for entry in &entries {
        out.push_str(entry);
    }
    out.push_str("};\n");
    out
}

fn render_entry(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    let key = format!("{} {}", route.method.as_str(), route.path);
    let params = render_params(route, cfg, refs);
    let query = render_query(route, cfg, refs);
    let body = render_body(route, cfg, refs);
    let response = render_response(route, cfg, refs);
    let error = render_error(route, cfg, refs);

    let mut out = format!("  \"{key}\": {{\n");
    out.push_str(&format!("    params: {params};\n"));
    out.push_str(&format!("    query: {query};\n"));
    out.push_str(&format!("    body: {body};\n"));
    out.push_str(&format!("    response: {response};\n"));
    out.push_str(&format!("    error: {error};\n"));
    if let Some(arrays) = &route.query_arrays {
        out.push_str(&format!("    queryArrays: \"{arrays}\";\n"));
    }
    out.push_str("  };\n");
    out
}

fn render_params(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    if route.params.is_empty() {
        return "never".to_string();
    }
    let fields: Vec<Field> = route
        .params
        .iter()
        .map(|p| Field {
            rust_name: p.name.clone(),
            ts_name: p.name.clone(),
            ty: p.ty.clone(),
            optional: false,
            flatten: false,
            readonly: false,
            inline: false,
            docs: vec![],
        })
        .collect();
    let ty = Ty::Object(fields);
    collect_ty_refs(&ty, refs);
    emit_ty(&ty, cfg)
}

fn render_query(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    match &route.query {
        None => "never".to_string(),
        Some(ty) => {
            collect_ty_refs(ty, refs);
            emit_ty(ty, cfg)
        }
    }
}

/// `Multipart` extractors carry `FormData`, `Bytes`/`Body` carry `BodyInit`,
/// and `String` carries `string` -- none of these three is a declared type,
/// so none ever needs an import.
fn render_body(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    match &route.body {
        None => "never".to_string(),
        Some(RouteBody::Typed(ty)) => {
            collect_ty_refs(ty, refs);
            emit_ty(ty, cfg)
        }
        Some(RouteBody::FormData) => "FormData".to_string(),
        Some(RouteBody::Raw) => "BodyInit".to_string(),
        Some(RouteBody::Text) => "string".to_string(),
    }
}

fn render_response(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    collect_ty_refs(&route.response, refs);
    emit_ty(&route.response, cfg)
}

fn render_error(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    match &route.error {
        None => "never".to_string(),
        Some(ty) => {
            collect_ty_refs(ty, refs);
            emit_ty(ty, cfg)
        }
    }
}

/// Walks a bare [`Ty`] for every [`Ty::Ref`] name it contains, the same
/// traversal [`crate::render`]'s `collect_ty_refs` does for a declaration
/// body. A route's own anonymous params object is never itself a `Ty::Ref`,
/// so it never ends up demanding an import -- only real references nested
/// inside it do.
fn collect_ty_refs(ty: &Ty, out: &mut BTreeSet<String>) {
    match ty {
        Ty::Ref { name, args } => {
            out.insert(name.clone());
            args.iter().for_each(|a| collect_ty_refs(a, out));
        }
        Ty::Array(inner) => collect_ty_refs(inner, out),
        Ty::Tuple(items) | Ty::Union(items) => items.iter().for_each(|t| collect_ty_refs(t, out)),
        Ty::Record { key, value } => {
            collect_ty_refs(key, out);
            collect_ty_refs(value, out);
        }
        Ty::Object(fields) => fields.iter().for_each(|f| collect_ty_refs(&f.ty, out)),
        Ty::Raw { args, .. } => args.iter().for_each(|a| collect_ty_refs(a, out)),
        Ty::Prim(_) | Ty::Param(_) | Ty::Unknown => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::ir::{Method, Prim, RouteParam, SourceLoc};

    fn route(method: Method, path: &str, group: &[&str], ts_name: &str) -> Route {
        Route {
            method,
            path: path.to_string(),
            ts_name: ts_name.to_string(),
            group: group.iter().map(|s| s.to_string()).collect(),
            params: vec![],
            query: None,
            body: None,
            response: Ty::Prim(Prim::Null),
            error: None,
            query_arrays: None,
            docs: vec![],
            loc: SourceLoc { file: "h.rs".into(), line: 1 },
        }
    }

    fn registry_of(routes: Vec<Route>) -> Registry {
        let mut reg = Registry::default();
        for r in routes {
            reg.insert_route(r).expect("fixture routes must not clash");
        }
        reg
    }

    fn registry_with_get_patient() -> Registry {
        let mut r = route(Method::Get, "/v2/patient/view/{id}", &["patientView"], "getPatient");
        r.params = vec![RouteParam {
            name: "id".into(),
            ty: Ty::Prim(Prim::Int { bits: 64, signed: true }),
            wildcard: false,
        }];
        r.response = Ty::Ref { name: "PatientView".into(), args: vec![] };
        registry_of(vec![r])
    }

    fn registry_with_bare_get() -> Registry {
        registry_of(vec![route(Method::Get, "/v2/schemes", &[], "getSchemes")])
    }

    fn registry_with_csv_query() -> Registry {
        let mut r = route(Method::Get, "/v2/search", &["search"], "search");
        r.query = Some(Ty::Ref { name: "Filter".into(), args: vec![] });
        r.query_arrays = Some("csv".into());
        registry_of(vec![r])
    }

    #[test]
    fn the_sender_file_is_marked_generated_and_defines_the_contract() {
        let ts = render_sender(&Config::new());
        assert!(ts.starts_with(crate::render::MARKER));
        assert!(ts.contains("export interface Sender<O = never> {"));
        assert!(ts.contains("send<R, E>(req: RouteRequest<R, E>, opts?: O): Promise<RouteResult<R, E>>;"));
        assert!(ts.contains("readonly ok: true"));
        assert!(ts.contains("readonly errors: E"));
    }

    #[test]
    fn the_route_map_is_keyed_by_method_and_template() {
        let ts = render_routes(&registry_with_get_patient(), &Config::new());
        assert!(ts.contains(r#""GET /v2/patient/view/{id}": {"#), "got: {ts}");
        assert!(ts.contains("params: { id: number }"));
        assert!(ts.contains("response: PatientView"));
        assert!(ts.contains("import type { PatientView } from \"./PatientView\";"));
    }

    #[test]
    fn a_route_with_no_pieces_records_them_as_never() {
        let ts = render_routes(&registry_with_bare_get(), &Config::new());
        assert!(ts.contains("params: never"));
        assert!(ts.contains("body: never"));
    }

    #[test]
    fn a_csv_query_array_is_recorded_on_the_route() {
        let ts = render_routes(&registry_with_csv_query(), &Config::new());
        assert!(ts.contains("queryArrays: \"csv\""), "got: {ts}");
    }
}
