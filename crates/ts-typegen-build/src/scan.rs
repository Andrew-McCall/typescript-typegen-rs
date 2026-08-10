//! Walking source roots and collecting declarations.
//!
//! Module resolution is deliberately syntactic: `mod foo;` is followed to
//! `foo.rs` or `foo/mod.rs`, and `#[cfg(...)]` is ignored. Generating a superset
//! of types is safer than evaluating cfgs incorrectly.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Registry, SourceLoc};
use crate::lower::LowerCtx;
use crate::parse::{has_ts_derive, parse_item};

/// Walks every root's `mod` tree, collecting every `#[derive(Ts)]` struct
/// and enum into a [`Registry`]. `#[cfg(...)]` is not evaluated, so a type
/// gated out for the current build still produces a binding.
pub fn scan_roots(roots: &[PathBuf], ctx: &LowerCtx, diags: &mut Diagnostics) -> Registry {
    let mut reg = Registry::default();
    let mut seen = HashSet::new();
    for root in roots {
        for entry in entry_points(root) {
            walk_file(&entry, "crate", ctx, &mut reg, &mut seen, diags);
        }
    }
    reg
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
    ctx: &LowerCtx,
    reg: &mut Registry,
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
    walk_items(&ast.items, file, module_path, ctx, reg, seen, diags);
}

fn walk_items(
    items: &[syn::Item],
    file: &Path,
    module_path: &str,
    ctx: &LowerCtx,
    reg: &mut Registry,
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
                if let Some(decl) = parse_item(&input, module_path, file, ctx, diags) {
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
            }
            syn::Item::Mod(m) => {
                let child = format!("{module_path}::{}", m.ident);
                match &m.content {
                    Some((_, inner)) => walk_items(inner, file, &child, ctx, reg, seen, diags),
                    None => {
                        if let Some(path) = resolve_mod_file(file, &m.ident.to_string()) {
                            walk_file(&path, &child, ctx, reg, seen, diags);
                        }
                    }
                }
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
