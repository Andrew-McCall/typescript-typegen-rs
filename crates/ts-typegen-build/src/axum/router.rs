//! Discovering routes by walking real `Router::new().route(...)` builder
//! chains, as a second, independent source alongside [`crate::axum::routes`]'s
//! attribute-based discovery.
//!
//! A handler or a nested router is referenced in source as a bare or
//! module-qualified `syn::Path` (`get_patient`, or
//! `handlers::patient::get_patient`); resolving one against every scanned
//! `fn` sharing its name, preferring the longest matching module suffix, is
//! what lets `handlers::patient_view::get_patient` and a same-named sibling
//! in `handlers::patient` both resolve unambiguously.

use std::collections::HashSet;
use std::path::PathBuf;

use syn::spanned::Spanned;

use crate::axum::bind::final_segment;
use crate::axum::config::Axum;
use crate::axum::routes::{Overrides, RouteFact};
use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::SourceLoc;
use crate::parse::loc_of;
use crate::scan::FnItem;

const HTTP_VERBS: &[&str] = &["get", "post", "put", "patch", "delete", "head", "options", "trace"];

/// Walks every `fn` in `fns` whose return type's final segment is `Router`,
/// folding its builder chain into one [`RouteFact`] per `.route(...)`
/// method, each carrying the resolved handler's bare name and module.
///
/// A router-returning `fn` that is itself reached through another one's
/// `.nest`/`.merge` is walked only as part of that reachable chain, not
/// independently too -- otherwise the same handler would surface once at its
/// own router's un-prefixed path and again at the prefix composed through
/// whichever router nests it. A pure cycle, where every candidate is
/// reachable only from another candidate, has no such entry point; the
/// first one found is walked anyway so the cycle is still diagnosed.
pub fn facts_from_routers(fns: &[FnItem], cfg: &Axum, diags: &mut Diagnostics) -> Vec<RouteFact> {
    let router_fns: Vec<&FnItem> = fns.iter().filter(|f| returns_router(&f.item.sig)).collect();
    let referenced = referenced_router_fns(&router_fns, fns);

    let mut entries: Vec<&FnItem> =
        router_fns.iter().copied().filter(|f| !referenced.contains(&fn_identity(f))).collect();
    if entries.is_empty() {
        entries = router_fns.into_iter().take(1).collect();
    }

    let env = RouterEnv { fns, cfg };
    let mut out = Vec::new();
    for f in entries {
        let Some(body) = trailing_expr(&f.item) else { continue };
        let mut state = WalkState { visited: HashSet::new(), diags, out: &mut out };
        state.visited.insert(fn_identity(f));
        walk(body, "", &env, f, &mut state);
    }
    out
}

/// The parts of a router walk that never change once it starts: every
/// scanned `fn`, to resolve a handler or sub-router reference against, and
/// the configured options that decide how an unfollowable expression is
/// reported.
struct RouterEnv<'a> {
    fns: &'a [FnItem],
    cfg: &'a Axum,
}

/// The parts of a router walk that accumulate as it proceeds: which fns are
/// on the current call stack (to catch a cycle), where diagnostics go, and
/// where a finished [`RouteFact`] goes.
struct WalkState<'a> {
    visited: HashSet<String>,
    diags: &'a mut Diagnostics,
    out: &'a mut Vec<RouteFact>,
}

/// Every router-returning fn directly or transitively reached through
/// another router-returning fn's `.nest`/`.merge`, computed with a throwaway
/// [`Diagnostics`] sink -- resolution ambiguity here is only used to decide
/// walk entry points, and is reported for real once the entry's own walk
/// reaches the same call.
fn referenced_router_fns(router_fns: &[&FnItem], fns: &[FnItem]) -> HashSet<String> {
    let mut referenced = HashSet::new();
    let mut seen = HashSet::new();
    for f in router_fns {
        if let Some(body) = trailing_expr(&f.item) {
            mark_refs(body, fns, &mut referenced, &mut seen);
        }
    }
    referenced
}

fn mark_refs(expr: &syn::Expr, fns: &[FnItem], referenced: &mut HashSet<String>, seen: &mut HashSet<String>) {
    let syn::Expr::MethodCall(mc) = expr else { return };
    match mc.method.to_string().as_str() {
        "nest" if mc.args.len() == 2 => {
            mark_refs(&mc.receiver, fns, referenced, seen);
            mark_target(&mc.args[1], fns, referenced, seen);
        }
        "merge" if mc.args.len() == 1 => {
            mark_refs(&mc.receiver, fns, referenced, seen);
            mark_target(&mc.args[0], fns, referenced, seen);
        }
        _ => mark_refs(&mc.receiver, fns, referenced, seen),
    }
}

fn mark_target(expr: &syn::Expr, fns: &[FnItem], referenced: &mut HashSet<String>, seen: &mut HashSet<String>) {
    let syn::Expr::Call(call) = peel_passthrough(expr) else { return };
    if !call.args.is_empty() {
        return;
    }
    let syn::Expr::Path(p) = call.func.as_ref() else { return };
    let mut scratch = Diagnostics::default();
    let loc = SourceLoc { file: PathBuf::new(), line: 0 };
    let Some(target) = resolve_router_fn(&p.path, fns, &loc, &mut scratch) else { return };
    let key = fn_identity(target);
    referenced.insert(key.clone());
    if !seen.insert(key) {
        return;
    }
    if let Some(body) = trailing_expr(&target.item) {
        mark_refs(body, fns, referenced, seen);
    }
}

/// Recursively folds one builder-chain expression into `out`, composing
/// `prefix` through `.nest`, following `.merge` and a resolved sub-router
/// call, and passing straight through anything else this module doesn't
/// specifically recognize. `current` is whichever `fn` owns `expr`'s source
/// span -- the original top-level entry, or the target of a followed
/// `.nest`/`.merge` call once the walk has recursed into its body.
fn walk(expr: &syn::Expr, prefix: &str, env: &RouterEnv, current: &FnItem, state: &mut WalkState) {
    match peel_passthrough(expr) {
        syn::Expr::MethodCall(mc) => match mc.method.to_string().as_str() {
            "route" => {
                walk(&mc.receiver, prefix, env, current, state);
                if mc.args.len() != 2 {
                    return;
                }
                walk_route(&mc.args[0], &mc.args[1], prefix, env, current, state);
            }
            "nest" => {
                walk(&mc.receiver, prefix, env, current, state);
                if mc.args.len() != 2 {
                    return;
                }
                let loc = loc_of(&current.file, mc.args[0].span());
                match literal_str(&mc.args[0]) {
                    Some(lit) => {
                        let composed = format!("{prefix}{lit}");
                        walk(&mc.args[1], &composed, env, current, state);
                    }
                    None => unfollowable(env.cfg, state.diags, loc, "a non-literal prefix in `.nest`"),
                }
            }
            "merge" => {
                walk(&mc.receiver, prefix, env, current, state);
                if mc.args.len() == 1 {
                    walk(&mc.args[0], prefix, env, current, state);
                }
            }
            // peel_passthrough already strips every method call that isn't one of
            // the three above; this arm only exists so a future passthrough name
            // added there without a matching case here warns instead of panics.
            _ => walk(&mc.receiver, prefix, env, current, state),
        },
        base @ syn::Expr::Call(call) => walk_call(call, base, prefix, env, current, state),
        base => unfollowable(
            env.cfg,
            state.diags,
            loc_of(&current.file, base.span()),
            "an expression that cannot be followed",
        ),
    }
}

/// Strips `.layer(...)`, `.route_layer(...)`, `.with_state(...)`, `.fallback(...)`,
/// and any other method call this module doesn't specifically interpret, down to
/// the expression underneath. `walk` uses this to find the real operation under
/// an arbitrary stack of such wrappers; [`mark_target`] uses the same helper so
/// the duplicate-fact pre-pass sees through exactly the same wrappers the real
/// walk does -- `.nest("/api", routes().with_state(s))` must mark `routes` as
/// referenced the same way `.nest("/api", routes())` does.
fn peel_passthrough(expr: &syn::Expr) -> &syn::Expr {
    match expr {
        syn::Expr::MethodCall(mc) if !matches!(mc.method.to_string().as_str(), "route" | "nest" | "merge") => {
            peel_passthrough(&mc.receiver)
        }
        _ => expr,
    }
}

fn walk_route(
    path_arg: &syn::Expr,
    handler_arg: &syn::Expr,
    prefix: &str,
    env: &RouterEnv,
    current: &FnItem,
    state: &mut WalkState,
) {
    let loc = loc_of(&current.file, path_arg.span());
    let Some(lit) = literal_str(path_arg) else {
        unfollowable(env.cfg, state.diags, loc, "a non-literal path in `.route`");
        return;
    };
    let full_path = format!("{prefix}{lit}");

    for (verb, handler_expr) in extract_verbs(handler_arg, env.cfg, &loc, state.diags) {
        let syn::Expr::Path(p) = handler_expr else {
            unfollowable(env.cfg, state.diags, loc.clone(), "a route handler that is not a plain function reference");
            continue;
        };
        let Some(handler) = resolve_handler(&p.path, env.fns, &loc, state.diags) else { continue };
        state.out.push(RouteFact {
            method: Some(verb),
            path: Some(full_path.clone()),
            overrides: Overrides::default(),
            loc: loc.clone(),
            handler: Some(handler),
        });
    }
}

fn walk_call(call: &syn::ExprCall, expr: &syn::Expr, prefix: &str, env: &RouterEnv, current: &FnItem, state: &mut WalkState) {
    let syn::Expr::Path(p) = call.func.as_ref() else {
        unfollowable(
            env.cfg,
            state.diags,
            loc_of(&current.file, expr.span()),
            "a call through a non-path expression",
        );
        return;
    };
    if is_router_new(&p.path) && call.args.is_empty() {
        return;
    }
    let loc = loc_of(&current.file, expr.span());
    if !call.args.is_empty() {
        unfollowable(env.cfg, state.diags, loc, "a call with arguments");
        return;
    }
    match resolve_router_fn(&p.path, env.fns, &loc, state.diags) {
        Some(target) => {
            let key = fn_identity(target);
            if !state.visited.insert(key.clone()) {
                state.diags.push(Diagnostic::new(
                    loc,
                    format!(
                        "cycle detected between router fns `{}` and `{}`",
                        current.item.sig.ident, target.item.sig.ident
                    ),
                ));
                return;
            }
            if let Some(body) = trailing_expr(&target.item) {
                walk(body, prefix, env, target, state);
            }
            state.visited.remove(&key);
        }
        None => unfollowable(
            env.cfg,
            state.diags,
            loc,
            &format!("a call to an unresolvable fn `{}`", path_to_string(&p.path)),
        ),
    }
}

fn is_router_new(path: &syn::Path) -> bool {
    path.segments.last().is_some_and(|s| s.ident == "new")
}

fn unfollowable(cfg: &Axum, diags: &mut Diagnostics, loc: SourceLoc, why: &str) {
    let msg = format!("router discovery cannot follow {why}");
    if cfg.strict_routes_enabled() {
        diags.push(Diagnostic::new(loc, msg));
    } else {
        diags.push(Diagnostic::warning(loc, msg));
    }
}

fn literal_str(expr: &syn::Expr) -> Option<String> {
    match expr {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => Some(s.value()),
        _ => None,
    }
}

/// Flattens a `.route`'s handler argument into one `(METHOD, handler-expr)`
/// pair per verb: `get(h)` yields one pair, `get(list).post(create)` yields
/// two (itself a `MethodCall` wrapping a `Call`, recursed the same way as a
/// router chain), `any(h)` yields `"ANY"`, and `on(a | b, h)` yields one pair
/// per `MethodFilter` constant in the disjunction.
fn extract_verbs<'e>(
    expr: &'e syn::Expr,
    cfg: &Axum,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<(String, &'e syn::Expr)> {
    let (verb, args, receiver) = match expr {
        syn::Expr::MethodCall(mc) => (mc.method.to_string(), &mc.args, Some(mc.receiver.as_ref())),
        syn::Expr::Call(call) => {
            let syn::Expr::Path(p) = call.func.as_ref() else { return Vec::new() };
            let name = p.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
            (name, &call.args, None)
        }
        _ => return Vec::new(),
    };

    let mut out = Vec::new();
    if let Some(r) = receiver {
        out.extend(extract_verbs(r, cfg, loc, diags));
    }

    match verb.as_str() {
        "any" if args.len() == 1 => out.push(("ANY".to_string(), &args[0])),
        "on" if args.len() == 2 => {
            for m in method_filter_methods(&args[0], cfg, loc, diags) {
                out.push((m, &args[1]));
            }
        }
        v if HTTP_VERBS.contains(&v) && args.len() == 1 => out.push((v.to_ascii_uppercase(), &args[0])),
        _ => {}
    }
    out
}

/// Flattens an `.on(...)` filter's `MethodFilter::A | MethodFilter::B | ...`
/// disjunction into one uppercased method name per constant. Anything that
/// isn't a literal `MethodFilter::X` path, or a `|` of such paths, warns
/// (escalated to an error under `strict_routes`) instead of guessing.
fn method_filter_methods(expr: &syn::Expr, cfg: &Axum, loc: &SourceLoc, diags: &mut Diagnostics) -> Vec<String> {
    match expr {
        syn::Expr::Binary(b) if matches!(b.op, syn::BinOp::BitOr(_)) => {
            let mut left = method_filter_methods(&b.left, cfg, loc, diags);
            left.extend(method_filter_methods(&b.right, cfg, loc, diags));
            left
        }
        syn::Expr::Path(p) if p.path.segments.len() >= 2 => {
            let segs = &p.path.segments;
            if segs[segs.len() - 2].ident == "MethodFilter" {
                vec![segs.last().unwrap().ident.to_string().to_ascii_uppercase()]
            } else {
                unfollowable(cfg, diags, loc.clone(), "an `.on(...)` filter that is not a `MethodFilter` constant");
                Vec::new()
            }
        }
        _ => {
            unfollowable(
                cfg,
                diags,
                loc.clone(),
                "an `.on(...)` filter that is not a literal disjunction of `MethodFilter` constants",
            );
            Vec::new()
        }
    }
}

/// Resolves a handler reference to its bare name and enclosing module's
/// full path -- not just its last segment, which two scanned fns sharing a
/// name and a module's last segment (but not its full path) could otherwise
/// collide on in [`crate::axum::routes::handler_matches`]. A reference
/// matching no scanned `fn` at all is not an error -- none of this exists to
/// validate the handler, only to name it -- so it falls back to the
/// reference's own written qualifier path, or an empty one when it has none.
/// Two or more scanned candidates tied for the longest matching module
/// suffix is the only failure case, since there is then no way to tell which
/// one the reference means.
fn resolve_handler(path: &syn::Path, fns: &[FnItem], loc: &SourceLoc, diags: &mut Diagnostics) -> Option<(String, String)> {
    let segs = path_segments(path);
    let name = segs.last()?.clone();
    let qualifiers = &segs[..segs.len() - 1];
    let candidates = find_candidates(&name, fns, false);
    match pick_longest_suffix(qualifiers, candidates) {
        Ok(Some(f)) => Some((name, f.module_path.clone())),
        Ok(None) => Some((name, qualifiers.join("::"))),
        Err(winners) => {
            diags.push(ambiguous_diag(&name, &winners, loc));
            None
        }
    }
}

/// Resolves a `.nest`/`.merge` call target to the scanned `Router`-returning
/// `fn` it refers to, by the same longest-suffix rule. Unlike
/// [`resolve_handler`], matching no candidate here is a real failure --
/// there is no body to recurse into -- so it is left to the caller to treat
/// as unfollowable.
fn resolve_router_fn<'a>(path: &syn::Path, fns: &'a [FnItem], loc: &SourceLoc, diags: &mut Diagnostics) -> Option<&'a FnItem> {
    let segs = path_segments(path);
    let name = segs.last()?.clone();
    let qualifiers = &segs[..segs.len() - 1];
    let candidates = find_candidates(&name, fns, true);
    match pick_longest_suffix(qualifiers, candidates) {
        Ok(found) => found,
        Err(winners) => {
            diags.push(ambiguous_diag(&name, &winners, loc));
            None
        }
    }
}

fn find_candidates<'a>(name: &str, fns: &'a [FnItem], router_only: bool) -> Vec<&'a FnItem> {
    fns.iter()
        .filter(|f| f.item.sig.ident == name)
        .filter(|f| !router_only || returns_router(&f.item.sig))
        .collect()
}

/// `Ok(None)` with no candidate at all; `Ok(Some(f))` with a unique winner
/// (trivially the only candidate, or the one with a strictly longer
/// trailing-module match than every other); `Err(tied)` when two or more
/// share the longest match -- including every candidate when `qualifiers` is
/// empty, since a bare reference's match length against any module is
/// always zero.
fn pick_longest_suffix<'a>(qualifiers: &[String], candidates: Vec<&'a FnItem>) -> Result<Option<&'a FnItem>, Vec<&'a FnItem>> {
    match candidates.len() {
        0 => Ok(None),
        1 => Ok(Some(candidates[0])),
        _ => {
            let max = candidates.iter().map(|f| suffix_match_len(qualifiers, &f.module_path)).max().unwrap();
            let winners: Vec<&FnItem> =
                candidates.into_iter().filter(|f| suffix_match_len(qualifiers, &f.module_path) == max).collect();
            if winners.len() == 1 { Ok(Some(winners[0])) } else { Err(winners) }
        }
    }
}

fn suffix_match_len(qualifiers: &[String], module_path: &str) -> usize {
    let mod_segs: Vec<&str> = module_path.split("::").collect();
    qualifiers.iter().rev().zip(mod_segs.iter().rev()).take_while(|(q, m)| q.as_str() == **m).count()
}

fn ambiguous_diag(name: &str, winners: &[&FnItem], loc: &SourceLoc) -> Diagnostic {
    let listing: Vec<String> = winners.iter().map(|f| f.file.display().to_string()).collect();
    Diagnostic::new(loc.clone(), format!("`{name}` is ambiguous between {}", listing.join(", ")))
        .with_help("qualify the reference with enough of its module path to tell them apart")
}

fn returns_router(sig: &syn::Signature) -> bool {
    let syn::ReturnType::Type(_, ty) = &sig.output else { return false };
    final_segment(ty).as_deref() == Some("Router")
}

/// The function body's trailing expression -- the common case this module
/// supports is a block whose last statement has no trailing semicolon and is
/// the whole chain. A block ending in anything else (a `let`, a
/// semicolon-terminated statement) has no trailing expression to walk.
fn trailing_expr(item: &syn::ItemFn) -> Option<&syn::Expr> {
    match item.block.stmts.last()? {
        syn::Stmt::Expr(e, None) => Some(e),
        _ => None,
    }
}

fn fn_identity(f: &FnItem) -> String {
    format!("{}::{}", f.module_path, f.item.sig.ident)
}

fn path_segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

fn path_to_string(path: &syn::Path) -> String {
    path_segments(path).join("::")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::axum::routes::collect_routes;
    use crate::config::Config;
    use crate::ir::{Method, Route};
    use crate::lower::LowerCtx;
    use crate::scan::scan_roots;

    fn facts_diag_src_with(src: &str, extra_handler_paths: &[&str], axum: Axum) -> (Vec<RouteFact>, Diagnostics) {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("src");
        std::fs::create_dir_all(&base).unwrap();

        let mut roots = vec![base.clone()];
        std::fs::write(base.join("app.rs"), src).unwrap();

        for rel in extra_handler_paths {
            let path = base.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let fn_name = path.file_stem().unwrap().to_str().unwrap();
            std::fs::write(&path, format!("pub async fn {fn_name}() {{}}\n")).unwrap();
            let parent = path.parent().unwrap().to_path_buf();
            if !roots.contains(&parent) {
                roots.push(parent);
            }
        }

        let mut diags = Diagnostics::default();
        let scan = scan_roots(&roots, &LowerCtx::default(), &mut diags);
        let (_, fns) = scan.split();
        let facts = facts_from_routers(&fns, &axum, &mut diags);
        (facts, diags)
    }

    fn facts_diag_src(src: &str, extra_handler_paths: &[&str]) -> (Vec<RouteFact>, Diagnostics) {
        facts_diag_src_with(src, extra_handler_paths, Axum::new().routers(true))
    }

    fn facts_diag_with_fns(src: &str, extra_handler_paths: &[&str]) -> (Vec<RouteFact>, Diagnostics) {
        facts_diag_src(src, extra_handler_paths)
    }

    fn facts_diag(src: &str) -> (Vec<RouteFact>, Diagnostics) {
        facts_diag_src(src, &[])
    }

    fn facts_diag_strict(src: &str) -> (Vec<RouteFact>, Diagnostics) {
        facts_diag_src_with(src, &[], Axum::new().routers(true).strict_routes(true))
    }

    fn facts_in_module(src: &str) -> Vec<RouteFact> {
        let (f, d) = facts_diag_src(src, &[]);
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        f
    }

    fn facts(expr: &str) -> Vec<RouteFact> {
        facts_in_module(&format!("fn app() -> Router {{ {expr} }}"))
    }

    fn methods(f: &[RouteFact]) -> Vec<String> {
        let mut v: Vec<String> = f.iter().filter_map(|x| x.method.clone()).collect();
        v.sort();
        v
    }

    fn paths(f: &[RouteFact]) -> Vec<String> {
        f.iter().filter_map(|x| x.path.clone()).collect()
    }

    fn handlers(f: &[RouteFact]) -> Vec<String> {
        f.iter().filter_map(|x| x.handler.as_ref().map(|(name, _)| name.clone())).collect()
    }

    fn handler_modules(f: &[RouteFact]) -> Vec<String> {
        f.iter()
            .filter_map(|x| x.handler.as_ref().map(|(_, module)| module.rsplit("::").next().unwrap().to_string()))
            .collect()
    }

    fn collect_with_router_diag(attr_src: &str, router_src: &str) -> (Vec<Route>, Diagnostics) {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("app.rs"), format!("{attr_src}\n{router_src}\n")).unwrap();

        let cfg = Config::new().scan(&src).output(dir.path().join("bindings")).axum(Axum::new().routers(true));
        let mut diags = Diagnostics::default();
        let scan = scan_roots(std::slice::from_ref(&src), &LowerCtx::default(), &mut diags);
        let (mut reg, fns) = scan.split();
        collect_routes(&fns, &mut reg, &cfg, &LowerCtx::default(), &mut diags);
        (reg.routes().to_vec(), diags)
    }

    fn collect_with_router(attr_src: &str, router_src: &str) -> Vec<Route> {
        let (routes, d) = collect_with_router_diag(attr_src, router_src);
        assert!(!d.has_errors(), "unexpected errors: {:?}", d.iter().collect::<Vec<_>>());
        routes
    }

    #[test]
    fn a_literal_route_call_yields_one_fact_per_method() {
        let facts = facts("Router::new().route(\"/a\", get(list).post(create))");
        assert_eq!(methods(&facts), vec!["GET", "POST"]);
        assert_eq!(handlers(&facts), vec!["list", "create"]);
    }

    #[test]
    fn nest_prefixes_compose_through_a_called_fn() {
        let facts = facts_in_module(
            "fn routes() -> Router { Router::new().route(\"/b\", get(h)) }
             fn app() -> Router { Router::new().nest(\"/a\", routes()) }",
        );
        assert_eq!(paths(&facts), vec!["/a/b"]);
    }

    #[test]
    fn merge_is_followed_and_layers_pass_through() {
        let facts = facts_in_module(
            "fn inner() -> Router { Router::new().route(\"/x\", get(h)) }
             fn app() -> Router { Router::new().merge(inner()).layer(trace()).with_state(s) }",
        );
        assert_eq!(paths(&facts), vec!["/x"]);
    }

    #[test]
    fn a_cycle_between_router_fns_terminates_with_a_diagnostic() {
        let (_, d) = facts_diag(
            "fn a() -> Router { Router::new().merge(b()) }
             fn b() -> Router { Router::new().merge(a()) }",
        );
        assert!(d.iter().any(|x| x.message.contains("cycle")));
    }

    #[test]
    fn a_non_literal_path_warns_and_does_not_fail_the_build() {
        let (facts, d) = facts_diag("fn app() -> Router { Router::new().route(PATH, get(h)) }");
        assert!(facts.is_empty());
        assert!(!d.has_errors());
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn strict_routes_escalates_an_unfollowable_expression_to_an_error() {
        let (_, d) = facts_diag_strict("fn app() -> Router { Router::new().route(PATH, get(h)) }");
        assert!(d.has_errors());
    }

    #[test]
    fn a_non_path_handler_warns_and_does_not_fail_the_build() {
        let (facts, d) =
            facts_diag("fn app() -> Router { Router::new().route(\"/\", get(|| async { \"Hello\" })) }");
        assert!(facts.is_empty());
        assert!(!d.has_errors());
        assert_eq!(d.len(), 1);
    }

    #[test]
    fn strict_routes_escalates_a_non_path_handler_to_an_error() {
        let (_, d) =
            facts_diag_strict("fn app() -> Router { Router::new().route(\"/\", get(|| async { \"Hello\" })) }");
        assert!(d.has_errors());
    }

    #[test]
    fn nest_target_wrapped_in_with_state_does_not_duplicate_the_fact() {
        let facts = facts_in_module(
            "fn routes() -> Router { Router::new().route(\"/b\", get(h)) }
             fn app() -> Router { Router::new().nest(\"/a\", routes().with_state(s)) }",
        );
        assert_eq!(paths(&facts), vec!["/a/b"]);
    }

    #[test]
    fn any_and_on_expand_to_concrete_methods() {
        assert_eq!(methods(&facts("Router::new().route(\"/a\", any(h))")), vec!["ANY"]);
        let on = facts("Router::new().route(\"/a\", on(MethodFilter::GET | MethodFilter::POST, h))");
        assert_eq!(methods(&on), vec!["GET", "POST"]);
    }

    #[test]
    fn a_handler_reference_resolves_by_longest_module_suffix() {
        let facts = facts_in_module(
            "fn app() -> Router {
                 Router::new()
                     .route(\"/a\", get(handlers::patient::get_patient))
                     .route(\"/b\", get(handlers::patient_view::get_patient))
             }",
        );
        assert_eq!(handler_modules(&facts), vec!["patient", "patient_view"]);
    }

    #[test]
    fn an_ambiguous_bare_handler_reference_is_an_error_naming_every_candidate() {
        let (facts, d) = facts_diag_with_fns(
            "fn app() -> Router { Router::new().route(\"/a\", get(get_patient)) }",
            &["handlers/patient/get_patient.rs", "handlers/patient_view/get_patient.rs"],
        );
        assert!(facts.is_empty());
        assert!(d.has_errors());
        let m = &d.iter().next().unwrap().message;
        assert!(m.contains("patient/") && m.contains("patient_view/"), "both candidates: {m}");
    }

    #[test]
    fn a_bare_ts_route_resolves_its_method_and_path_from_the_router() {
        let routes = collect_with_router(
            r#"#[ts_route] async fn h() -> Json<A> {}"#,
            "fn app() -> Router { Router::new().route(\"/a/{id}\", get(h)) }",
        );
        assert_eq!(routes[0].path, "/a/{id}");
        assert_eq!(routes[0].method, Method::Get);
    }

    #[test]
    fn a_bare_ts_route_the_scan_cannot_resolve_is_an_error() {
        let (routes, d) = collect_with_router_diag(r#"#[ts_route] async fn h() {}"#, "");
        assert!(routes.is_empty());
        assert!(d.has_errors());
        assert!(d.iter().next().unwrap().help.as_deref().unwrap().contains("routers"));
    }
}
