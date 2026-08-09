//! Lowering from `syn::Type` into the IR's `Ty`.
//!
//! This module is deliberately ignorant of TypeScript. Integer widths are
//! recorded as-is; choosing `number` vs `bigint` is the emitter's job.

use std::collections::{HashMap, HashSet};

use crate::ir::{Field, Prim, Ty};

/// Maps Rust type paths to verbatim TypeScript. Lookup is by final path
/// segment, matching the reference-resolution rule in the spec.
#[derive(Debug, Clone, Default)]
pub struct TypeMap(HashMap<String, String>);

impl TypeMap {
    pub fn insert(&mut self, rust_path: &str, ts: &str) {
        let key = rust_path.rsplit("::").next().unwrap_or(rust_path);
        self.0.insert(key.to_string(), ts.to_string());
    }

    pub fn get(&self, last_segment: &str) -> Option<&str> {
        self.0.get(last_segment).map(String::as_str)
    }
}

#[derive(Debug, Default, Clone)]
pub struct LowerCtx {
    /// Type parameters of the enclosing declaration.
    pub params: HashSet<String>,
    pub map: TypeMap,
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

pub fn lower_type(ty: &syn::Type, ctx: &LowerCtx) -> Ty {
    match ty {
        syn::Type::Reference(r) => lower_type(&r.elem, ctx),
        syn::Type::Paren(p) => lower_type(&p.elem, ctx),
        syn::Type::Group(g) => lower_type(&g.elem, ctx),
        syn::Type::Slice(s) => Ty::Array(Box::new(lower_type(&s.elem, ctx))),
        syn::Type::Array(a) => Ty::Array(Box::new(lower_type(&a.elem, ctx))),
        syn::Type::Tuple(t) if t.elems.is_empty() => Ty::Prim(Prim::Null),
        syn::Type::Tuple(t) => Ty::Tuple(t.elems.iter().map(|e| lower_type(e, ctx)).collect()),
        syn::Type::Path(p) => lower_path(p, ctx),
        _ => Ty::Unknown,
    }
}

fn lower_path(p: &syn::TypePath, ctx: &LowerCtx) -> Ty {
    let Some(seg) = p.path.segments.last() else {
        return Ty::Unknown;
    };
    let name = seg.ident.to_string();
    let args = type_args(seg, ctx);

    // Config overrides win before anything else.
    if let Some(raw) = ctx.map.get(&name) {
        return Ty::Raw(raw.to_string());
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
        return Ty::Record {
            key: Box::new(it.next().unwrap_or(Ty::Unknown)),
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
        _ => Ty::Ref { name, args },
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

fn type_args(seg: &syn::PathSegment, ctx: &LowerCtx) -> Vec<Ty> {
    let syn::PathArguments::AngleBracketed(ab) = &seg.arguments else {
        return vec![];
    };
    ab.args
        .iter()
        .filter_map(|a| match a {
            syn::GenericArgument::Type(t) => Some(lower_type(t, ctx)),
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

    fn lower(src: &str) -> Ty {
        let ty: syn::Type = syn::parse_str(src).expect("parse");
        lower_type(&ty, &LowerCtx::default())
    }

    fn lower_generic(src: &str, params: &[&str]) -> Ty {
        let ty: syn::Type = syn::parse_str(src).expect("parse");
        let ctx = LowerCtx {
            params: params.iter().map(|s| s.to_string()).collect(),
            ..LowerCtx::default()
        };
        lower_type(&ty, &ctx)
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
        let ctx = LowerCtx {
            map,
            ..LowerCtx::default()
        };
        let ty: syn::Type = syn::parse_str("uuid::Uuid").unwrap();
        assert_eq!(lower_type(&ty, &ctx), Ty::Raw("string".into()));
        let bare: syn::Type = syn::parse_str("Uuid").unwrap();
        assert_eq!(lower_type(&bare, &ctx), Ty::Raw("string".into()));
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
}
