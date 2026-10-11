//! Walking source roots and collecting declarations.
//!
//! Module resolution is deliberately syntactic: `mod foo;` is followed to
//! `foo.rs` or `foo/mod.rs`, and `#[cfg(...)]` is ignored. Generating a superset
//! of types is safer than evaluating cfgs incorrectly.
//!
//! Scanning happens in two passes. The first walks the whole `mod` tree once,
//! collecting `#[derive(Ts)]` candidates and `type` aliases without lowering
//! either -- an alias may be declared in a file visited after its first use,
//! so nothing can be resolved mid-walk. The second resolves every alias (see
//! [`crate::lower::resolve_aliases`]), then lowers each candidate against a
//! [`LowerCtx`] that already knows about all of them.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Registry, SourceLoc};
use crate::lower::{LowerCtx, RawAlias, resolve_aliases};
use crate::parse::{has_ts_derive, loc_of, parse_item, type_param_names};

/// The result of a scan: every `#[derive(Ts)]` type, lowered into a
/// [`Registry`], plus every `fn` item found along the way.
pub struct ScanOutput {
    pub registry: Registry,
    pub fns: Vec<FnItem>,
}

impl ScanOutput {
    /// Splits the scan apart so its `fns` can be collected against a
    /// `&mut Registry` already moved out of this struct.
    pub fn split(self) -> (Registry, Vec<FnItem>) {
        (self.registry, self.fns)
    }
}

/// A `fn` item found while walking, kept regardless of whether it looks like
/// a handler -- relevance is decided later by discovery logic.
pub struct FnItem {
    pub item: syn::ItemFn,
    pub module_path: String,
    pub file: PathBuf,
}

/// Walks every root's `mod` tree, collecting every `#[derive(Ts)]` struct
/// and enum into a [`Registry`], and every `fn` item into [`ScanOutput::fns`].
/// `#[cfg(...)]` is not evaluated, so a type or fn gated out for the current
/// build still produces a binding.
pub fn scan_roots(roots: &[PathBuf], ctx: &LowerCtx, diags: &mut Diagnostics) -> ScanOutput {
    let mut collected = Collected::default();
    let mut seen = HashSet::new();
    for root in roots {
        for entry in entry_points(root) {
            walk_file(&entry, "crate", &mut collected, &mut seen, diags);
        }
    }

    let aliases = resolve_aliases(&collected.aliases, ctx, diags);

    let mut reg = Registry::default();
    for candidate in collected.decls {
        let item_ctx = LowerCtx { aliases: aliases.clone(), ..ctx.clone() };
        let Some(decl) = parse_item(&candidate.input, &candidate.module_path, &candidate.file, &item_ctx, diags)
        else {
            continue;
        };
        if let Err(clash) = reg.insert(decl) {
            diags.push(
                Diagnostic::new(
                    clash.incoming.clone(),
                    format!("duplicate TypeScript name `{}`", clash.ts_name),
                )
                .with_help(format!(
                    "already declared at {}:{}; use #[ts(rename = \"...\")] \
                     on one of them",
                    clash.existing.file.display(),
                    clash.existing.line
                )),
            );
        }
    }
    ScanOutput { registry: reg, fns: collected.fns }
}

/// Every `#[derive(Ts)]` candidate, `type` alias, and `fn` item found while
/// walking, not yet parsed or lowered.
#[derive(Default)]
struct Collected {
    decls: Vec<Candidate>,
    aliases: HashMap<String, RawAlias>,
    fns: Vec<FnItem>,
}

/// A struct or enum carrying `#[derive(Ts)]`, deferred until aliases are
/// resolved so its fields can reference one.
struct Candidate {
    input: syn::DeriveInput,
    module_path: String,
    file: PathBuf,
}

/// A root's entry points: `lib.rs` and `main.rs` if present, otherwise every
/// `.rs` file directly inside it.
fn entry_points(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in ["lib.rs", "main.rs"] {
        let p = root.join(name);
        if p.is_file() {
            out.push(p);
        }
    }
    if out.is_empty() {
        if let Ok(rd) = std::fs::read_dir(root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "rs") {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn walk_file(
    file: &Path,
    module_path: &str,
    collected: &mut Collected,
    seen: &mut HashSet<PathBuf>,
    diags: &mut Diagnostics,
) {
    let canonical = file.canonicalize().unwrap_or_else(|_| file.to_path_buf());
    if !seen.insert(canonical) {
        return;
    }
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let ast = match syn::parse_file(&text) {
        Ok(a) => a,
        Err(e) => {
            diags.push(Diagnostic::new(
                SourceLoc {
                    file: file.to_path_buf(),
                    line: e.span().start().line,
                },
                format!("failed to parse: {e}"),
            ));
            return;
        }
    };
    walk_items(&ast.items, file, module_path, collected, seen, diags);
}

fn walk_items(
    items: &[syn::Item],
    file: &Path,
    module_path: &str,
    collected: &mut Collected,
    seen: &mut HashSet<PathBuf>,
    diags: &mut Diagnostics,
) {
    for item in items {
        match item {
            syn::Item::Struct(_) | syn::Item::Enum(_) => {
                let Some(input) = to_derive_input(item) else {
                    continue;
                };
                if !has_ts_derive(&input.attrs) {
                    continue;
                }
                collected.decls.push(Candidate {
                    input,
                    module_path: module_path.to_string(),
                    file: file.to_path_buf(),
                });
            }
            syn::Item::Type(t) => {
                let name = t.ident.to_string();
                let loc = loc_of(file, t.ident.span());
                let params = type_param_names(&t.generics, &loc, diags);
                match collected.aliases.entry(name.clone()) {
                    std::collections::hash_map::Entry::Occupied(_) => {
                        diags.push(
                            Diagnostic::warning(
                                loc,
                                format!("type alias `{name}` is declared more than once"),
                            )
                            .with_help(
                                "only the first declaration found is used; \
                                 later ones are ignored, same as a duplicate TypeScript name",
                            ),
                        );
                    }
                    std::collections::hash_map::Entry::Vacant(e) => {
                        e.insert(RawAlias { params, target: (*t.ty).clone(), loc });
                    }
                }
            }
            syn::Item::Mod(m) => {
                let child = format!("{module_path}::{}", m.ident);
                match &m.content {
                    Some((_, inner)) => walk_items(inner, file, &child, collected, seen, diags),
                    None => {
                        if let Some(path) = resolve_mod_file(file, &m.ident.to_string()) {
                            walk_file(&path, &child, collected, seen, diags);
                        }
                    }
                }
            }
            syn::Item::Fn(f) => {
                collected.fns.push(FnItem {
                    item: f.clone(),
                    module_path: module_path.to_string(),
                    file: file.to_path_buf(),
                });
            }
            _ => {}
        }
    }
}

fn to_derive_input(item: &syn::Item) -> Option<syn::DeriveInput> {
    match item {
        syn::Item::Struct(s) => Some(syn::DeriveInput::from(s.clone())),
        syn::Item::Enum(e) => Some(syn::DeriveInput::from(e.clone())),
        _ => None,
    }
}

/// `mod foo;` inside `a/b.rs` resolves to `a/b/foo.rs` or `a/b/foo/mod.rs`;
/// inside `a/mod.rs` or a crate root it resolves relative to that directory.
fn resolve_mod_file(current: &Path, name: &str) -> Option<PathBuf> {
    let dir = current.parent()?;
    let stem = current.file_stem()?.to_str()?;
    let bases = if matches!(stem, "lib" | "main" | "mod") {
        vec![dir.to_path_buf()]
    } else {
        vec![dir.join(stem), dir.to_path_buf()]
    };
    for base in bases {
        for candidate in [
            base.join(format!("{name}.rs")),
            base.join(name).join("mod.rs"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}
