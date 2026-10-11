//! Emitting the generated TypeScript client: the sender contract in
//! `sender.ts`, and the per-route type map in `routes.ts`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use crate::axum::config::Client;
use crate::axum::path::{self, Segment};
use crate::config::Config;
use crate::diag::Diagnostics;
use crate::emit::emit_ty;
use crate::ir::{Field, Method, Prim, Registry, Route, RouteBody, RouteParam, SourceLoc, Ty};
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

/// A [`Field`] standing in for one argument-object key, carrying nothing a
/// handler-struct field would (no optionality, no flattening, no docs).
fn plain_field(name: &str, ty: Ty) -> Field {
    Field {
        rust_name: name.to_string(),
        ts_name: name.to_string(),
        ty,
        optional: false,
        flatten: false,
        readonly: false,
        inline: false,
        docs: vec![],
    }
}

fn render_params(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>) -> String {
    if route.params.is_empty() {
        return "never".to_string();
    }
    let fields: Vec<Field> =
        route.params.iter().map(|p| plain_field(&p.name, p.ty.clone())).collect();
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

/// Renders the generated TypeScript client: `client.ts` plus one
/// `client/<group>.ts` per non-empty [`Route::group`], or (under
/// [`Client::Flat`]) a single flat `client.ts` with no group files at all.
/// A missing axum section renders the same as `Client::None` -- the
/// per-group layout -- since whether to call this at all is the caller's
/// decision, not this function's.
pub fn render_client(reg: &Registry, cfg: &Config) -> BTreeMap<String, String> {
    let flat = cfg.axum_config().is_some_and(|a| matches!(a.client_style(), Client::Flat));
    if flat {
        render_flat(reg, cfg)
    } else {
        render_modules(reg, cfg)
    }
}

fn render_flat(reg: &Registry, cfg: &Config) -> BTreeMap<String, String> {
    let mut refs = BTreeSet::new();
    let members: Vec<String> =
        reg.routes().iter().map(|r| render_route_member(r, cfg, &mut refs, "    ")).collect();
    let any_route = reg.routes().iter().any(|r| r.method == Method::Any);

    let mut out = banner(cfg);
    out.push_str(&sender_import("./", any_route, cfg));
    out.push_str(&sender_reexport(cfg));
    for name in &refs {
        out.push_str(&format!("import type {{ {name} }} from \"./{name}{}\";\n", cfg.extension()));
    }
    out.push('\n');
    out.push_str(&root_factory(&[], &members));

    let mut files = BTreeMap::new();
    files.insert("client.ts".to_string(), out);
    files
}

fn render_modules(reg: &Registry, cfg: &Config) -> BTreeMap<String, String> {
    let mut grouped: BTreeMap<Vec<String>, Vec<&Route>> = BTreeMap::new();
    let mut ungrouped: Vec<&Route> = Vec::new();
    for route in reg.routes() {
        if route.group.is_empty() {
            ungrouped.push(route);
        } else {
            grouped.entry(route.group.clone()).or_default().push(route);
        }
    }

    let mut files = BTreeMap::new();
    // (the group file's path under client/, the factory's exported name). A
    // group deeper than one level is untested by this plan; the leaf
    // component stands in for both the factory name and the root's member
    // key, which is simple and matches every tested case exactly, but two
    // different parents sharing a leaf name would collide at the root.
    let mut groups: Vec<(String, String)> = Vec::new();
    for (group, routes) in &grouped {
        let name = group.last().cloned().unwrap_or_default();
        let join = group.join("/");
        files.insert(format!("client/{join}.ts"), render_group_file(group, routes, &name, cfg));
        groups.push((join, name));
    }

    let mut refs = BTreeSet::new();
    let members: Vec<String> =
        ungrouped.iter().map(|r| render_route_member(r, cfg, &mut refs, "    ")).collect();
    let any_route = ungrouped.iter().any(|r| r.method == Method::Any);

    let mut out = banner(cfg);
    out.push_str(&sender_import("./", any_route, cfg));
    out.push_str(&sender_reexport(cfg));
    for (join, name) in &groups {
        out.push_str(&format!("import {{ {name} }} from \"./client/{join}{}\";\n", cfg.extension()));
    }
    for name in &refs {
        out.push_str(&format!("import type {{ {name} }} from \"./{name}{}\";\n", cfg.extension()));
    }
    out.push('\n');

    let group_lines: Vec<String> =
        groups.iter().map(|(_, name)| format!("    {name}: {name}(s),\n")).collect();
    out.push_str(&root_factory(&group_lines, &members));

    files.insert("client.ts".to_string(), out);
    files
}

/// `export function createClient...` plus the derived `Client` type alias,
/// its `return {{ ... }}` body built from the group-assembly lines (empty
/// under `Client::Flat`) followed by every inlined route member.
fn root_factory(group_lines: &[String], members: &[String]) -> String {
    let mut out = String::from("export function createClient<O = never>(s: Sender<O>) {\n");
    out.push_str("  return {\n");
    for line in group_lines {
        out.push_str(line);
    }
    for line in members {
        out.push_str(line);
    }
    out.push_str("  };\n");
    out.push_str("}\n");
    out.push_str("export type Client<O = never> = ReturnType<typeof createClient<O>>;\n");
    out
}

/// One group's file: the banner, its imports, and
/// `export const <name> = <O,>(s: Sender<O>) => ({{ ... }});`.
fn render_group_file(group: &[String], routes: &[&Route], name: &str, cfg: &Config) -> String {
    let mut refs = BTreeSet::new();
    let members: Vec<String> =
        routes.iter().map(|r| render_route_member(r, cfg, &mut refs, "  ")).collect();
    let any_route = routes.iter().any(|r| r.method == Method::Any);
    // One level for `client/`, plus one more per nested group component --
    // `client/patientView.ts` is one level under the output root, so one
    // `../` reaches it; `client/a/b.ts` needs two.
    let up = "../".repeat(group.len());

    let mut out = banner(cfg);
    out.push_str(&sender_import(&up, any_route, cfg));
    for ref_name in &refs {
        out.push_str(&format!(
            "import type {{ {ref_name} }} from \"{up}{ref_name}{}\";\n",
            cfg.extension()
        ));
    }
    out.push('\n');
    out.push_str(&format!("export const {name} = <O,>(s: Sender<O>) => ({{\n"));
    for line in &members {
        out.push_str(line);
    }
    out.push_str("});\n");
    out
}

/// The `Sender` import line every group or root factory file needs; widened
/// to also carry `Method` when the file inlines an `any`-method route, whose
/// argument object references that sender-contract type by name.
fn sender_import(prefix: &str, any_route: bool, cfg: &Config) -> String {
    let names = if any_route { "Sender, Method" } else { "Sender" };
    format!("import type {{ {names} }} from \"{prefix}sender{}\";\n", cfg.extension())
}

/// Re-exports the rest of the sender contract from the root client file, so a
/// consumer only needs one import for `createClient` and the types its
/// result is shaped from, instead of also reaching into `sender.ts` by hand.
fn sender_reexport(cfg: &Config) -> String {
    format!("export type {{ Sender, RouteResult }} from \"./sender{}\";\n", cfg.extension())
}

/// One route as a factory member: `name: (args, opts?: O) => s.send<R, E>({{
/// ... }}, opts),`, indented by `indent`. `args` is omitted entirely when the
/// route takes no method/params/body/query at all.
fn render_route_member(route: &Route, cfg: &Config, refs: &mut BTreeSet<String>, indent: &str) -> String {
    let fields = arg_fields(route);
    for f in &fields {
        collect_ty_refs(&f.ty, refs);
    }
    let arg_list = if fields.is_empty() {
        String::new()
    } else {
        let keys = fields.iter().map(|f| f.ts_name.clone()).collect::<Vec<_>>().join(", ");
        let ty = emit_ty(&Ty::Object(fields), cfg);
        format!("{{ {keys} }}: {ty}, ")
    };

    collect_ty_refs(&route.response, refs);
    let response = emit_ty(&route.response, cfg);
    let error_ty = route.error.clone().unwrap_or(Ty::Unknown);
    collect_ty_refs(&error_ty, refs);
    let error = emit_ty(&error_ty, cfg);

    let method_field = if route.method == Method::Any {
        "method".to_string()
    } else {
        format!("\"{}\"", route.method.as_str())
    };
    let path_expr = render_path_literal(&route.path, &route.params);

    let mut out = format!(
        "{indent}{}: ({arg_list}opts?: O) =>\n{indent}  s.send<{response}, {error}>({{\n",
        route.ts_name
    );
    out.push_str(&format!("{indent}    method: {method_field},\n"));
    out.push_str(&format!("{indent}    route: \"{}\",\n", route.path));
    out.push_str(&format!("{indent}    path: {path_expr},\n"));
    if route.query.is_some() {
        // A concrete query type (e.g. `CheckoutQuery`) has no index signature, so it
        // isn't assignable -- or even directly castable -- to `Record<string, unknown>`
        // without going through `unknown` first.
        out.push_str(&format!(
            "{indent}    query: query as unknown as Record<string, unknown>,\n"
        ));
    }
    if route.body.is_some() {
        out.push_str(&format!("{indent}    body: body,\n"));
    }
    out.push_str(&format!("{indent}  }}, opts),\n"));
    out
}

/// The argument-object fields for one route, in order: the reserved
/// `method` key for an `any` route, then every path param by its own name,
/// then `body`, then `query` -- the fixed literal names `Route` stores no
/// original binding name for, per the plan's IR.
fn arg_fields(route: &Route) -> Vec<Field> {
    let mut fields = Vec::new();
    if route.method == Method::Any {
        fields.push(plain_field("method", Ty::Raw { template: "Method".to_string(), args: vec![] }));
    }
    for p in &route.params {
        fields.push(plain_field(&p.name, p.ty.clone()));
    }
    if let Some(body) = &route.body {
        fields.push(plain_field("body", body_ty(body)));
    }
    if let Some(query) = &route.query {
        fields.push(plain_field("query", query.clone()));
    }
    fields
}

/// The argument-object type for a route's body extractor. `FormData` and
/// `BodyInit` are global TS lib types, never declarations this crate emits,
/// so neither needs an import -- the same reasoning [`render_body`] above
/// documents for the `routes.ts` type map.
fn body_ty(body: &RouteBody) -> Ty {
    match body {
        RouteBody::Typed(ty) => ty.clone(),
        RouteBody::FormData => Ty::Raw { template: "FormData".to_string(), args: vec![] },
        RouteBody::Raw => Ty::Raw { template: "BodyInit".to_string(), args: vec![] },
        RouteBody::Text => Ty::Prim(Prim::Str),
    }
}

/// The client call's `path:` value: a backtick TS template literal built by
/// walking `route_path`'s parsed segments -- a literal contributes its
/// [`path::escape_ts_literal`]-escaped text, a capture contributes
/// `${name}`, zipped positionally against `params` rather than read off the
/// template itself: a `Path<Struct>` or a renamed binding gives a capture a
/// JS-facing name that differs from its raw template text, and that
/// resolved name is what the argument object actually destructures. Route
/// assembly only ever produces a `Route` with as many `params` as the
/// template has captures, so the zip always lines up. A route whose path
/// fails to re-parse here -- never true for one that passed through route
/// discovery, only reachable from a hand-built `Route` bypassing it --
/// falls back to treating the whole string as one literal segment, so it
/// still renders instead of panicking.
fn render_path_literal(route_path: &str, params: &[RouteParam]) -> String {
    let loc = SourceLoc { file: PathBuf::new(), line: 0 };
    let mut diags = Diagnostics::default();
    let segs = path::parse_template(route_path, &loc, &mut diags)
        .unwrap_or_else(|| vec![Segment::Literal(route_path.to_string())]);
    let mut names = params.iter().map(|p| p.name.as_str());
    let parts: Vec<String> = segs
        .iter()
        .map(|seg| match seg {
            Segment::Literal(text) => path::escape_ts_literal(text),
            Segment::Capture { .. } => format!("${{{}}}", names.next().unwrap_or_default()),
        })
        .collect();
    format!("`{}`", parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::config::{Axum, Client};
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

    fn registry_with_two_groups() -> Registry {
        let mut a = route(Method::Get, "/v2/patient/view/{id}", &["patientView"], "getPatient");
        a.params = vec![RouteParam {
            name: "id".into(),
            ty: Ty::Prim(Prim::Int { bits: 64, signed: true }),
            wildcard: false,
        }];
        a.response = Ty::Ref { name: "PatientView".into(), args: vec![] };

        let mut b = route(Method::Get, "/v2/other/{id}", &["other"], "getPatient");
        b.params = vec![RouteParam {
            name: "id".into(),
            ty: Ty::Prim(Prim::Int { bits: 64, signed: true }),
            wildcard: false,
        }];
        b.response = Ty::Ref { name: "PatientView".into(), args: vec![] };

        registry_of(vec![a, b])
    }

    fn registry_with_path(path: &str) -> Registry {
        registry_of(vec![route(Method::Get, path, &[], "getThing")])
    }

    fn registry_with_any_route() -> Registry {
        registry_of(vec![route(Method::Any, "/v2/proxy", &[], "proxyAny")])
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

    #[test]
    fn a_group_module_takes_a_sender_and_interpolates_the_path() {
        let files = render_client(&registry_with_get_patient(), &Config::new());
        let m = &files["client/patientView.ts"];
        assert!(m.contains("export const patientView = <O,>(s: Sender<O>) => ({"));
        assert!(m.contains("getPatient: ({ id }: { id: number }, opts?: O) =>"));
        assert!(m.contains("path: `/v2/patient/view/${id}`"), "got: {m}");
        assert!(m.contains(r#"route: "/v2/patient/view/{id}""#));
    }

    #[test]
    fn a_query_argument_is_cast_through_unknown_to_satisfy_the_sender_contract() {
        // `RouteRequest.query` is `Record<string, unknown>`; a concrete query type
        // like `Filter` has no index signature, so TypeScript rejects both a plain
        // assignment and a single `as` between them -- only going through `unknown`
        // first typechecks.
        let files = render_client(&registry_with_csv_query(), &Config::new());
        let m = &files["client/search.ts"];
        assert!(
            m.contains("query: query as unknown as Record<string, unknown>,"),
            "got: {m}"
        );
    }

    #[test]
    fn a_renamed_capture_interpolates_its_resolved_name_not_the_template_text() {
        let mut r = route(Method::Get, "/v2/order/{order_id}", &["order"], "getOrder");
        r.params = vec![RouteParam {
            name: "orderId".into(),
            ty: Ty::Prim(Prim::Int { bits: 64, signed: true }),
            wildcard: false,
        }];
        r.response = Ty::Ref { name: "OrderView".into(), args: vec![] };
        let files = render_client(&registry_of(vec![r]), &Config::new());
        let m = &files["client/order.ts"];
        assert!(m.contains("getOrder: ({ orderId }: { orderId: number }, opts?: O) =>"));
        assert!(m.contains("path: `/v2/order/${orderId}`"), "got: {m}");
    }

    #[test]
    fn a_route_with_no_arguments_takes_only_options() {
        let files = render_client(&registry_with_bare_get(), &Config::new());
        assert!(files["client.ts"].contains("getSchemes: (opts?: O) =>"), "got: {}", files["client.ts"]);
    }

    #[test]
    fn the_root_module_assembles_every_group() {
        let files = render_client(&registry_with_two_groups(), &Config::new());
        let root = &files["client.ts"];
        assert!(root.contains("export function createClient<O = never>(s: Sender<O>) {"));
        assert!(root.contains("patientView: patientView(s),"));
        assert!(root.contains("export type Client<O = never> = ReturnType<typeof createClient<O>>;"));
    }

    #[test]
    fn the_root_module_reexports_the_sender_contract() {
        let grouped = render_client(&registry_with_two_groups(), &Config::new());
        assert!(grouped["client.ts"].contains("export type { Sender, RouteResult } from \"./sender\";"));

        let cfg = Config::new().axum(Axum::new().client(Client::Flat));
        let flat = render_client(&registry_with_bare_get(), &cfg);
        assert!(flat["client.ts"].contains("export type { Sender, RouteResult } from \"./sender\";"));
    }

    #[test]
    fn a_backtick_or_dollar_brace_in_a_path_is_escaped() {
        let files = render_client(&registry_with_path("/a/`x`/${y}"), &Config::new());
        let m = files.values().find(|v| v.contains("path:")).unwrap();
        assert!(m.contains("\\`x\\`"), "backtick must be escaped: {m}");
        assert!(m.contains("\\${y}"), "template hole must be escaped: {m}");
    }

    #[test]
    fn an_any_route_takes_the_method_as_an_argument_key() {
        let files = render_client(&registry_with_any_route(), &Config::new());
        let m = files.values().find(|v| v.contains("method:")).unwrap();
        assert!(m.contains("{ method }: { method: Method }"), "got: {m}");
    }

    #[test]
    fn imports_honour_the_configured_extension_and_depth() {
        let cfg = Config::new().import_extension(".js");
        let files = render_client(&registry_with_get_patient(), &cfg);
        assert!(files["client/patientView.ts"].contains("from \"../PatientView.js\""));
        assert!(files["client/patientView.ts"].contains("from \"../sender.js\""));
    }

    #[test]
    fn flat_mode_puts_every_function_on_one_object_and_emits_no_group_files() {
        let cfg = Config::new().axum(Axum::new().client(Client::Flat));
        let files = render_client(&registry_with_two_groups(), &cfg);
        assert_eq!(files.keys().collect::<Vec<_>>(), vec!["client.ts"]);
        assert_eq!(files["client.ts"].matches("getPatient:").count(), 2);
    }
}
