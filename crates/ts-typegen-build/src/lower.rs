//! Lowering from `syn::Type` into the IR's `Ty`.
//!
//! This module is deliberately ignorant of TypeScript. Integer widths are
//! recorded as-is; choosing `number` vs `bigint` is the emitter's job.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use crate::diag::{Diagnostic, Diagnostics};
use crate::ir::{Field, Prim, SourceLoc, Ty, placeholder_positions};

/// Maps Rust type paths to verbatim TypeScript. Lookup is by final path
/// segment, matching the reference-resolution rule in the spec.
#[derive(Debug, Clone, Default)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    /// Adds an override, keyed by `rust_path`'s final `::`-separated segment.
    pub fn insert(&mut self, rust_path: &str, ts: &str) {
        let key = rust_path.rsplit("::").next().unwrap_or(rust_path);
        self.0.insert(key.to_string(), ts.to_string());
    }

    /// Looks up an override by a type's final path segment.
    pub fn get(&self, last_segment: &str) -> Option<&str> {
        self.0.get(last_segment).map(String::as_str)
    }
}

/// A `pub type Name<P...> = Target;` collected while scanning, not yet
/// lowered -- an alias may be declared in a file visited after its first use,
/// so every alias is collected before any of them are resolved.
#[derive(Debug, Clone)]
pub struct RawAlias {
    /// The alias's own generic type parameters, in declaration order.
    pub params: Vec<String>,
    /// The unlowered right-hand side.
    pub target: syn::Type,
    /// Where the alias was declared.
    pub loc: SourceLoc,
}

/// A [`RawAlias`] after its target has been lowered, with [`Ty::Param`]
/// standing in for each of the alias's own parameters wherever they appear.
#[derive(Debug, Clone)]
pub struct ResolvedAlias {
    /// The alias's own generic type parameters, in declaration order.
    pub params: Vec<String>,
    /// The lowered target, containing `Ty::Param(p)` for each `p` in `params`.
    pub ty: Ty,
}

/// Type aliases discovered while scanning, resolved once up front so a field
/// can reference `type Timestamp = chrono::DateTime<chrono::Utc>;` without a
/// `.map_type` entry of its own. Cheap to clone: shared via `Rc`, since a copy
/// rides along in every declaration's [`LowerCtx`].
#[derive(Debug, Clone, Default)]
pub struct AliasTable(Rc<HashMap<String, ResolvedAlias>>);

impl AliasTable {
    /// Looks up an alias by its own (unqualified) name.
    pub fn get(&self, name: &str) -> Option<&ResolvedAlias> {
        self.0.get(name)
    }
}

/// Context threaded through [`lower_type`]: the enclosing declaration's
/// generic parameters, the configured type overrides, and resolved aliases.
#[derive(Debug, Default, Clone)]
pub struct LowerCtx {
    /// Type parameters of the enclosing declaration.
    pub params: HashSet<String>,
    /// Configured type overrides, checked before any built-in mapping.
    pub map: TypeMap,
    /// Type aliases collected while scanning, checked after built-in forms.
    pub aliases: AliasTable,
}

/// Wrappers that vanish on the wire.
const TRANSPARENT: &[&str] = &[
    "Box", "Rc", "Arc", "Cow", "RefCell", "Cell", "Mutex", "RwLock",
];
const ARRAYS: &[&str] = &[
    "Vec",
    "VecDeque",
    "HashSet",
    "BTreeSet",
    "LinkedList",
    "BinaryHeap",
];
const MAPS: &[&str] = &["HashMap", "BTreeMap", "IndexMap"];

/// Lowers a parsed Rust type to its IR [`Ty`], per the wire shape
/// `serde_json` actually produces — not a structural translation. `loc` is
/// where the type was written, for diagnostics raised while lowering it.
pub fn lower_type(ty: &syn::Type, ctx: &LowerCtx, loc: &SourceLoc, diags: &mut Diagnostics) -> Ty {
    match ty {
        syn::Type::Reference(r) => lower_type(&r.elem, ctx, loc, diags),
        syn::Type::Paren(p) => lower_type(&p.elem, ctx, loc, diags),
        syn::Type::Group(g) => lower_type(&g.elem, ctx, loc, diags),
        syn::Type::Slice(s) => Ty::Array(Box::new(lower_type(&s.elem, ctx, loc, diags))),
        syn::Type::Array(a) => Ty::Array(Box::new(lower_type(&a.elem, ctx, loc, diags))),
        syn::Type::Tuple(t) if t.elems.is_empty() => Ty::Prim(Prim::Null),
        syn::Type::Tuple(t) => {
            Ty::Tuple(t.elems.iter().map(|e| lower_type(e, ctx, loc, diags)).collect())
        }
        syn::Type::Path(p) => lower_path(p, ctx, loc, diags),
        _ => Ty::Unknown,
    }
}

/// TS keywords a `map_type` target commonly is, on purpose, when it drops a
/// generic argument -- every built-in [`crate::presets::Preset`] targets
/// `string` this way. Used only to keep the "did you forget `{0}`" warning
/// from firing on this extremely common, clearly intentional pattern; it has
/// no effect on whether the argument is actually dropped, which happens
/// whenever the template has no placeholder, keyword or not.
const TERMINAL_KEYWORDS: &[&str] =
    &["string", "number", "boolean", "unknown", "any", "never", "null"];

fn lower_path(p: &syn::TypePath, ctx: &LowerCtx, loc: &SourceLoc, diags: &mut Diagnostics) -> Ty {
    let Some(seg) = p.path.segments.last() else {
        return Ty::Unknown;
    };
    let name = seg.ident.to_string();
    let args = type_args(seg, ctx, loc, diags);

    // Config overrides win before anything else.
    if let Some(template) = ctx.map.get(&name) {
        return apply_map_type(&name, template, args, loc, diags);
    }

    // A bare, unqualified identifier matching a declared type parameter.
    if p.qself.is_none()
        && p.path.segments.len() == 1
        && args.is_empty()
        && ctx.params.contains(&name)
    {
        return Ty::Param(name);
    }

    if let Some(prim) = builtin_prim(&name) {
        return Ty::Prim(prim);
    }

    if TRANSPARENT.contains(&name.as_str()) {
        return args.into_iter().next().unwrap_or(Ty::Unknown);
    }
    if ARRAYS.contains(&name.as_str()) {
        return Ty::Array(Box::new(args.into_iter().next().unwrap_or(Ty::Unknown)));
    }
    if MAPS.contains(&name.as_str()) {
        let mut it = args.into_iter();
        // The Rust key type is irrelevant to the binding: serde_json always
        // writes map keys as JSON object keys, which are strings whatever the
        // Rust side is -- HashMap<i32, V> serialises as {"7": ...}, not {7: ...}.
        it.next();
        return Ty::Record {
            key: Box::new(Ty::Prim(Prim::Str)),
            value: Box::new(it.next().unwrap_or(Ty::Unknown)),
        };
    }
    match name.as_str() {
        "Option" => {
            let inner = args.into_iter().next().unwrap_or(Ty::Unknown);
            Ty::Union(vec![inner, Ty::Prim(Prim::Null)])
        }
        "Result" => {
            let mut it = args.into_iter();
            let ok = it.next().unwrap_or(Ty::Unknown);
            let err = it.next().unwrap_or(Ty::Unknown);
            Ty::Union(vec![wire_field("Ok", ok), wire_field("Err", err)])
        }
        "Value" => Ty::Unknown,
        _ => resolve_alias_or_ref(name, args, ctx),
    }
}

/// Builds the [`Ty::Raw`] for a `map_type` match, keeping `args` only when
/// `template` actually references them via a `{0}`-style placeholder --
/// otherwise they are dropped here, before they can reach an import or an
/// "unknown type" diagnostic downstream. Warns when that drop looks like an
/// oversight rather than the deliberate terminal pattern every built-in
/// preset uses.
fn apply_map_type(
    name: &str,
    template: &str,
    args: Vec<Ty>,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Ty {
    let used = !placeholder_positions(template).is_empty();
    if !args.is_empty()
        && !used
        && !TERMINAL_KEYWORDS.contains(&template)
        && !template.contains('<')
    {
        let n = args.len();
        diags.push(
            Diagnostic::warning(
                loc.clone(),
                format!(
                    "map_type target `{template}` for `{name}` has no `{{0}}` placeholder, \
                     but `{name}` was used here with {n} generic argument{}",
                    if n == 1 { "" } else { "s" },
                ),
            )
            .with_help(format!(
                "add `{{0}}`{more} to the mapping to forward {them}, e.g. \
                 .map_type(\"...::{name}\", \"{template}<{{0}}>\") -- or ignore this if \
                 dropping {them} is intentional, the way string-valued presets do",
                more = if n > 1 { " (and `{1}`, ...)" } else { "" },
                them = if n == 1 { "it" } else { "them" },
            )),
        );
    }
    Ty::Raw {
        template: template.to_string(),
        args: if used { args } else { vec![] },
    }
}

/// Falls back to a resolved type alias before giving up on a name as an
/// unresolved reference to a to-be-generated declaration.
fn resolve_alias_or_ref(name: String, args: Vec<Ty>, ctx: &LowerCtx) -> Ty {
    if let Some(alias) = ctx.aliases.get(&name) {
        if alias.params.len() == args.len() {
            if alias.params.is_empty() {
                return alias.ty.clone();
            }
            let subs: HashMap<String, Ty> =
                alias.params.iter().cloned().zip(args).collect();
            return substitute_params(&alias.ty, &subs);
        }
    }
    Ty::Ref { name, args }
}

/// Replaces every [`Ty::Param`] in `ty` per `subs`, instantiating a generic
/// type alias's lowered target with the arguments from a specific use site.
fn substitute_params(ty: &Ty, subs: &HashMap<String, Ty>) -> Ty {
    match ty {
        Ty::Param(name) => subs.get(name).cloned().unwrap_or_else(|| ty.clone()),
        Ty::Array(inner) => Ty::Array(Box::new(substitute_params(inner, subs))),
        Ty::Tuple(items) => Ty::Tuple(items.iter().map(|t| substitute_params(t, subs)).collect()),
        Ty::Record { key, value } => Ty::Record {
            key: Box::new(substitute_params(key, subs)),
            value: Box::new(substitute_params(value, subs)),
        },
        Ty::Union(items) => Ty::Union(items.iter().map(|t| substitute_params(t, subs)).collect()),
        Ty::Object(fields) => Ty::Object(
            fields
                .iter()
                .map(|f| Field { ty: substitute_params(&f.ty, subs), ..f.clone() })
                .collect(),
        ),
        Ty::Ref { name, args } => Ty::Ref {
            name: name.clone(),
            args: args.iter().map(|t| substitute_params(t, subs)).collect(),
        },
        Ty::Raw { template, args } => Ty::Raw {
            template: template.clone(),
            args: args.iter().map(|t| substitute_params(t, subs)).collect(),
        },
        Ty::Prim(_) | Ty::Unknown => ty.clone(),
    }
}

/// Resolves every alias `raw` collected while scanning into an [`AliasTable`],
/// in dependency order so `type A = B;` sees `B`'s resolution even when `B` is
/// declared later in the scan. A cycle is reported and the aliases involved
/// are left unresolved -- a field referencing one falls through to a plain
/// reference and an "unknown type" diagnostic rather than looping forever.
pub fn resolve_aliases(
    raw: &HashMap<String, RawAlias>,
    base: &LowerCtx,
    diags: &mut Diagnostics,
) -> AliasTable {
    let mut resolved: HashMap<String, ResolvedAlias> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut names: Vec<&String> = raw.keys().collect();
    names.sort(); // deterministic diagnostic order, independent of scan order
    for name in names {
        resolve_one(name, raw, base, &mut resolved, &mut visiting, diags);
    }
    AliasTable(Rc::new(resolved))
}

fn resolve_one(
    name: &str,
    raw: &HashMap<String, RawAlias>,
    base: &LowerCtx,
    resolved: &mut HashMap<String, ResolvedAlias>,
    visiting: &mut HashSet<String>,
    diags: &mut Diagnostics,
) -> Option<Ty> {
    if let Some(r) = resolved.get(name) {
        return Some(r.ty.clone());
    }
    let entry = raw.get(name)?;
    if !visiting.insert(name.to_string()) {
        diags.push(
            Diagnostic::new(
                entry.loc.clone(),
                format!("type alias `{name}` is defined in terms of itself"),
            )
            .with_help(
                "cyclic type aliases have no finite shape; break the cycle, or \
                 map_type one side instead of aliasing it",
            ),
        );
        return None;
    }

    // Dependencies are resolved first so their entries exist in `resolved`
    // by the time this alias's target is lowered, whatever order the scan
    // found the declarations in.
    for dep in referenced_alias_names(&entry.target, raw) {
        resolve_one(&dep, raw, base, resolved, visiting, diags);
    }

    let ctx = LowerCtx {
        params: entry.params.iter().cloned().collect(),
        map: base.map.clone(),
        aliases: AliasTable(Rc::new(resolved.clone())),
    };
    let ty = lower_type(&entry.target, &ctx, &entry.loc, diags);
    visiting.remove(name);
    resolved.insert(name.to_string(), ResolvedAlias { params: entry.params.clone(), ty: ty.clone() });
    Some(ty)
}

/// Collects the names of every other raw alias directly referenced by `ty`'s
/// path segments (including inside generic arguments), so [`resolve_one`] can
/// resolve dependencies before the alias that needs them.
fn referenced_alias_names(ty: &syn::Type, raw: &HashMap<String, RawAlias>) -> Vec<String> {
    let mut out = Vec::new();
    collect_alias_refs(ty, raw, &mut out);
    out
}

fn collect_alias_refs(ty: &syn::Type, raw: &HashMap<String, RawAlias>, out: &mut Vec<String>) {
    match ty {
        syn::Type::Reference(r) => collect_alias_refs(&r.elem, raw, out),
        syn::Type::Paren(p) => collect_alias_refs(&p.elem, raw, out),
        syn::Type::Group(g) => collect_alias_refs(&g.elem, raw, out),
        syn::Type::Slice(s) => collect_alias_refs(&s.elem, raw, out),
        syn::Type::Array(a) => collect_alias_refs(&a.elem, raw, out),
        syn::Type::Tuple(t) => t.elems.iter().for_each(|e| collect_alias_refs(e, raw, out)),
        syn::Type::Path(p) => {
            let Some(seg) = p.path.segments.last() else { return };
            let name = seg.ident.to_string();
            if raw.contains_key(&name) {
                out.push(name);
            }
            if let syn::PathArguments::AngleBracketed(ab) = &seg.arguments {
                for a in &ab.args {
                    if let syn::GenericArgument::Type(t) = a {
                        collect_alias_refs(t, raw, out);
                    }
                }
            }
        }
        _ => {}
    }
}

fn wire_field(name: &str, ty: Ty) -> Ty {
    Ty::Object(vec![Field {
        rust_name: name.to_string(),
        ts_name: name.to_string(),
        ty,
        optional: false,
        flatten: false,
        readonly: false,
        inline: false,
        docs: vec![],
    }])
}

fn type_args(
    seg: &syn::PathSegment,
    ctx: &LowerCtx,
    loc: &SourceLoc,
    diags: &mut Diagnostics,
) -> Vec<Ty> {
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
        return vec![];
    };
    ab.args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(lower_type(t, ctx, loc, diags)),
            // lifetimes and const args carry no wire shape
            _ => None,
        })
        .collect()
}

fn builtin_prim(name: &str) -> Option<Prim> {
    Some(match name {
        "String" | "str" | "char" | "PathBuf" | "Path" | "OsString" | "OsStr" => Prim::Str,
        "bool" => Prim::Bool,
        "f32" | "f64" => Prim::Float,
        "i8" => Prim::Int {
            bits: 8,
            signed: true,
        },
        "i16" => Prim::Int {
            bits: 16,
            signed: true,
        },
        "i32" => Prim::Int {
            bits: 32,
            signed: true,
        },
        "i64" => Prim::Int {
            bits: 64,
            signed: true,
        },
        "i128" => Prim::Int {
            bits: 128,
            signed: true,
        },
        "isize" => Prim::Int {
            bits: 64,
            signed: true,
        },
        "u8" => Prim::Int {
            bits: 8,
            signed: false,
        },
        "u16" => Prim::Int {
            bits: 16,
            signed: false,
        },
        "u32" => Prim::Int {
            bits: 32,
            signed: false,
        },
        "u64" => Prim::Int {
            bits: 64,
            signed: false,
        },
        "u128" => Prim::Int {
            bits: 128,
            signed: false,
        },
        "usize" => Prim::Int {
            bits: 64,
            signed: false,
        },
        n if n.starts_with("NonZero") => return nonzero_prim(n),
        _ => return None,
    })
}

fn nonzero_prim(name: &str) -> Option<Prim> {
    builtin_prim(&name["NonZero".len()..].to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{Prim, Ty};

    fn dummy_loc() -> SourceLoc {
        SourceLoc { file: "t.rs".into(), line: 1 }
    }

    fn lower(src: &str) -> Ty {
        lower_with(src, &LowerCtx::default()).0
    }

    fn lower_generic(src: &str, params: &[&str]) -> Ty {
        let ctx = LowerCtx {
            params: params.iter().map(|s| s.to_string()).collect(),
            ..LowerCtx::default()
        };
        lower_with(src, &ctx).0
    }

    fn lower_with(src: &str, ctx: &LowerCtx) -> (Ty, Diagnostics) {
        let ty: syn::Type = syn::parse_str(src).expect("parse");
        let mut diags = Diagnostics::default();
        let result = lower_type(&ty, ctx, &dummy_loc(), &mut diags);
        (result, diags)
    }

    #[test]
    fn primitives() {
        assert_eq!(lower("String"), Ty::Prim(Prim::Str));
        assert_eq!(lower("&str"), Ty::Prim(Prim::Str));
        assert_eq!(lower("char"), Ty::Prim(Prim::Str));
        assert_eq!(lower("std::path::PathBuf"), Ty::Prim(Prim::Str));
        assert_eq!(lower("bool"), Ty::Prim(Prim::Bool));
        assert_eq!(lower("f64"), Ty::Prim(Prim::Float));
        assert_eq!(
            lower("i32"),
            Ty::Prim(Prim::Int {
                bits: 32,
                signed: true
            })
        );
        assert_eq!(
            lower("u64"),
            Ty::Prim(Prim::Int {
                bits: 64,
                signed: false
            })
        );
        assert_eq!(
            lower("usize"),
            Ty::Prim(Prim::Int {
                bits: 64,
                signed: false
            })
        );
        assert_eq!(lower("()"), Ty::Prim(Prim::Null));
    }

    #[test]
    fn collections() {
        assert_eq!(
            lower("Vec<u8>"),
            Ty::Array(Box::new(Ty::Prim(Prim::Int {
                bits: 8,
                signed: false
            })))
        );
        assert_eq!(
            lower("HashSet<String>"),
            Ty::Array(Box::new(Ty::Prim(Prim::Str)))
        );
        assert_eq!(
            lower("[bool; 4]"),
            Ty::Array(Box::new(Ty::Prim(Prim::Bool)))
        );
        assert_eq!(
            lower("HashMap<String, i32>"),
            Ty::Record {
                key: Box::new(Ty::Prim(Prim::Str)),
                value: Box::new(Ty::Prim(Prim::Int {
                    bits: 32,
                    signed: true
                })),
            }
        );
        assert_eq!(
            lower("(bool, String)"),
            Ty::Tuple(vec![Ty::Prim(Prim::Bool), Ty::Prim(Prim::Str)])
        );
    }

    #[test]
    fn transparent_wrappers_unwrap() {
        let expected = Ty::Prim(Prim::Str);
        assert_eq!(lower("Box<String>"), expected);
        assert_eq!(lower("Arc<String>"), expected);
        assert_eq!(lower("Cow<'a, String>"), expected);
        assert_eq!(lower("Mutex<String>"), expected);
    }

    #[test]
    fn option_lowers_to_union_with_null() {
        assert_eq!(
            lower("Option<String>"),
            Ty::Union(vec![Ty::Prim(Prim::Str), Ty::Prim(Prim::Null)])
        );
    }

    #[test]
    fn result_matches_serde_wire_shape() {
        // serde_json emits {"Ok": ...} / {"Err": ...}, not a bare value.
        let Ty::Union(arms) = lower("Result<u8, String>") else {
            panic!("expected union");
        };
        assert_eq!(arms.len(), 2);
        let Ty::Object(ok) = &arms[0] else {
            panic!("expected object")
        };
        assert_eq!(ok[0].ts_name, "Ok");
    }

    #[test]
    fn unknown_named_type_becomes_a_ref() {
        assert_eq!(
            lower("Address"),
            Ty::Ref {
                name: "Address".into(),
                args: vec![]
            }
        );
        assert_eq!(
            lower("crate::types::Address"),
            Ty::Ref {
                name: "Address".into(),
                args: vec![]
            }
        );
    }

    #[test]
    fn generic_params_beat_refs() {
        assert_eq!(lower_generic("T", &["T"]), Ty::Param("T".into()));
        assert_eq!(
            lower_generic("Vec<T>", &["T"]),
            Ty::Array(Box::new(Ty::Param("T".into())))
        );
        // Not a declared param, so it stays a reference.
        assert_eq!(
            lower_generic("T", &["U"]),
            Ty::Ref {
                name: "T".into(),
                args: vec![]
            }
        );
    }

    #[test]
    fn type_map_overrides_by_final_segment() {
        let mut map = TypeMap::default();
        map.insert("uuid::Uuid", "string");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        assert_eq!(
            lower_with("uuid::Uuid", &ctx).0,
            Ty::Raw { template: "string".into(), args: vec![] }
        );
        assert_eq!(
            lower_with("Uuid", &ctx).0,
            Ty::Raw { template: "string".into(), args: vec![] }
        );
    }

    #[test]
    fn generic_refs_carry_their_arguments() {
        assert_eq!(
            lower("Page<String>"),
            Ty::Ref {
                name: "Page".into(),
                args: vec![Ty::Prim(Prim::Str)]
            }
        );
    }

    #[test]
    fn a_map_type_with_a_placeholder_forwards_its_argument() {
        let mut map = TypeMap::default();
        map.insert("some::Wrapper", "TsWrapper<{0}>");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (ty, diags) = lower_with("some::Wrapper<String>", &ctx);
        assert_eq!(
            ty,
            Ty::Raw {
                template: "TsWrapper<{0}>".into(),
                args: vec![Ty::Prim(Prim::Str)]
            }
        );
        assert!(diags.is_empty(), "a placeholder that's used needs no warning: {diags:?}");
    }

    #[test]
    fn a_map_type_with_no_placeholder_drops_the_argument_entirely() {
        // The terminal case: chrono::DateTime -> "string" must not carry
        // DateTime<Utc>'s Utc forward, or it would need to resolve as an
        // importable type and break the build over an argument nothing uses.
        let mut map = TypeMap::default();
        map.insert("chrono::DateTime", "string");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (ty, diags) = lower_with("chrono::DateTime<Utc>", &ctx);
        assert_eq!(ty, Ty::Raw { template: "string".into(), args: vec![] });
        assert!(diags.is_empty(), "a bare TS keyword target is a deliberate terminal: {diags:?}");
    }

    #[test]
    fn a_shaped_alias_places_the_argument_inside_the_template() {
        let mut map = TypeMap::default();
        map.insert("dto::Patch", "{0} | null");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (ty, diags) = lower_with("dto::Patch<String>", &ctx);
        assert_eq!(
            ty,
            Ty::Raw { template: "{0} | null".into(), args: vec![Ty::Prim(Prim::Str)] }
        );
        assert!(diags.is_empty());
    }

    #[test]
    fn a_custom_terminal_with_no_placeholder_warns_its_probably_a_mistake() {
        let mut map = TypeMap::default();
        map.insert("mycrate::Wrapper", "TsWrapper");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (ty, diags) = lower_with("mycrate::Wrapper<String>", &ctx);
        // The argument is still dropped -- a warning changes nothing about
        // what gets emitted, only whether the author is told about it.
        assert_eq!(ty, Ty::Raw { template: "TsWrapper".into(), args: vec![] });
        assert_eq!(diags.len(), 1, "{diags:?}");
        let d = diags.iter().next().unwrap();
        assert!(d.message.contains("TsWrapper"), "{}", d.message);
        assert!(d.message.contains("Wrapper"), "{}", d.message);
        assert!(d.message.contains("{0}"), "{}", d.message);
    }

    #[test]
    fn a_template_that_already_spells_out_its_own_generics_does_not_warn() {
        // The author wrote a fixed instantiation on purpose; dropping the
        // source argument is clearly deliberate here, same as a keyword target.
        let mut map = TypeMap::default();
        map.insert("mycrate::Wrapper", "SomeUtility<string>");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (_, diags) = lower_with("mycrate::Wrapper<String>", &ctx);
        assert!(diags.is_empty(), "{diags:?}");
    }

    #[test]
    fn a_map_type_with_no_arguments_at_the_use_site_never_warns() {
        let mut map = TypeMap::default();
        map.insert("mycrate::Wrapper", "TsWrapper");
        let ctx = LowerCtx { map, ..LowerCtx::default() };
        let (_, diags) = lower_with("mycrate::Wrapper", &ctx);
        assert!(diags.is_empty(), "no argument was dropped, so nothing to warn about: {diags:?}");
    }

    #[test]
    fn nonzero_integers_lower_to_the_same_width_as_their_plain_counterpart() {
        // serde_json serialises NonZeroU32 as a plain number, same as u32, so the
        // binding must be identical -- only the Rust side enforces non-zero-ness.
        assert_eq!(
            lower("std::num::NonZeroU32"),
            Ty::Prim(Prim::Int {
                bits: 32,
                signed: false
            })
        );
        assert_eq!(
            lower("NonZeroI8"),
            Ty::Prim(Prim::Int {
                bits: 8,
                signed: true
            })
        );
        assert_eq!(
            lower("NonZeroUsize"),
            Ty::Prim(Prim::Int {
                bits: 64,
                signed: false
            })
        );
    }

    #[test]
    fn map_keys_are_always_strings_regardless_of_the_rust_key_type() {
        // serde_json writes every map key as a JSON object key, i.e. a string,
        // whatever the Rust key type is: HashMap<i32, V> serialises as
        // {"7": ...}, never {7: ...}.
        assert_eq!(
            lower("std::collections::HashMap<i32, String>"),
            Ty::Record {
                key: Box::new(Ty::Prim(Prim::Str)),
                value: Box::new(Ty::Prim(Prim::Str)),
            }
        );
        assert_eq!(
            lower("BTreeMap<u64, bool>"),
            Ty::Record {
                key: Box::new(Ty::Prim(Prim::Str)),
                value: Box::new(Ty::Prim(Prim::Bool)),
            }
        );
    }

    #[test]
    fn a_name_merely_prefixed_with_nonzero_falls_back_to_a_reference() {
        // Not a real NonZero* type, so there is no narrower primitive to report;
        // treating it as an ordinary reference is the honest fallback.
        assert_eq!(
            lower("NonZeroFrobnicator"),
            Ty::Ref {
                name: "NonZeroFrobnicator".into(),
                args: vec![]
            }
        );
    }
}
