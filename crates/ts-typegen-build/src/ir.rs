//! Language-neutral intermediate representation.
//!
//! Everything upstream of this module understands `syn` and serde semantics and
//! nothing about TypeScript. Everything downstream understands TypeScript and
//! nothing about Rust.

use std::collections::HashMap;
use std::path::PathBuf;

/// A source position, attached to declarations so diagnostics and name-clash
/// errors can point back at the Rust that produced them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    /// Path to the source file, relative to the scan root.
    pub file: PathBuf,
    /// One-based line number.
    pub line: usize,
}

/// A TypeScript primitive that a Rust scalar type lowers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prim {
    /// `string`.
    Str,
    /// `boolean`.
    Bool,
    /// `number`, from `f32`/`f64`.
    Float,
    /// An integer, carrying its Rust width and signedness so [`crate::render`]
    /// can pick `number`, `bigint`, or `string` per the configured [`crate::BigInt`] style.
    Int {
        /// Bit width of the source Rust type (8, 16, 32, 64, or 128).
        bits: u8,
        /// Whether the source Rust type is signed.
        signed: bool,
    },
    /// `null`.
    Null,
}

/// A TypeScript type, built up from a lowered Rust type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    /// A primitive scalar.
    Prim(Prim),
    /// `T[]`.
    Array(Box<Ty>),
    /// A fixed-length tuple, `[A, B, C]`.
    Tuple(Vec<Ty>),
    /// `Record<K, V>`, from a Rust map type.
    Record {
        /// The map's key type.
        key: Box<Ty>,
        /// The map's value type.
        value: Box<Ty>,
    },
    /// `A | B | ...`.
    Union(Vec<Ty>),
    /// An inline object shape, `{ field: T; ... }`, used when a field is
    /// spliced in with `#[ts(inline)]` instead of imported.
    Object(Vec<Field>),
    /// A reference to another generated type; becomes an import line.
    Ref {
        /// The referenced type's emitted TypeScript name.
        name: String,
        /// Generic arguments applied to the reference, e.g. the `T` in `Box<Page<T>>`.
        args: Vec<Ty>,
    },
    /// A generic type parameter of the enclosing declaration.
    Param(String),
    /// Verbatim TypeScript from `#[ts(type = "...")]` or a config type mapping,
    /// unimported and unchecked.
    Raw {
        /// The TS text to emit, e.g. `"string"` or `"TsWrapper<{0}>"`. May
        /// contain `{0}`, `{1}`, ... placeholders; see [`placeholder_positions`].
        template: String,
        /// Generic arguments from the source Rust type, substituted into
        /// `template`'s placeholders at render time. Empty when `template` has
        /// no placeholder -- a `.map_type` match that doesn't reference its
        /// argument drops it, it never reaches an import or an "unknown type"
        /// diagnostic.
        args: Vec<Ty>,
    },
    /// An opaque value with no known shape, e.g. `serde_json::Value`. Renders
    /// as `unknown` or `any` depending on [`crate::config::Config::unknown_type`].
    Unknown,
}

/// Finds every `{N}` placeholder in a [`Ty::Raw`] template, as
/// `(start_byte, end_byte_exclusive, N)` triples in left-to-right order.
///
/// A `{` not followed by ASCII digits and a `}` is left alone as literal
/// text rather than treated as a malformed placeholder -- a template is
/// free to contain a brace for other reasons.
pub fn placeholder_positions(template: &str) -> Vec<(usize, usize, usize)> {
    let mut out = Vec::new();
    let bytes = template.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            if let Some(rel_end) = template[i + 1..].find('}') {
                let digits = &template[i + 1..i + 1 + rel_end];
                if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                    if let Ok(idx) = digits.parse::<usize>() {
                        out.push((i, i + 2 + rel_end, idx));
                    }
                    i += 2 + rel_end;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod placeholder_tests {
    use super::placeholder_positions;

    #[test]
    fn finds_placeholders_in_order() {
        assert_eq!(placeholder_positions("{0}"), vec![(0, 3, 0)]);
        assert_eq!(placeholder_positions("{0} | {1}"), vec![(0, 3, 0), (6, 9, 1)]);
        assert_eq!(placeholder_positions("TsWrapper<{0}>"), vec![(10, 13, 0)]);
    }

    #[test]
    fn a_template_with_no_placeholder_finds_nothing() {
        assert!(placeholder_positions("string").is_empty());
        assert!(placeholder_positions("Foo<Bar>").is_empty());
    }

    #[test]
    fn braces_without_digits_are_literal_text_not_a_placeholder() {
        assert!(placeholder_positions("{}").is_empty());
        assert!(placeholder_positions("{abc}").is_empty());
        assert!(placeholder_positions("{").is_empty());
    }
}

/// A struct field or enum struct-variant field, lowered to its TypeScript shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    /// The field's name as written in Rust.
    pub rust_name: String,
    /// The field's name on the wire, after serde/`#[ts(rename...)]` renaming.
    pub ts_name: String,
    /// The field's lowered type.
    pub ty: Ty,
    /// Whether the field renders with `?:` instead of `:`.
    pub optional: bool,
    /// Whether the field came from `#[serde(flatten)]` and should splice its
    /// shape into the parent instead of appearing as a named member.
    pub flatten: bool,
    /// Emit `readonly` before the field name.
    pub readonly: bool,
    /// Splice the referenced type's shape here instead of importing it.
    pub inline: bool,
    /// Doc comment lines carried over from the Rust field, emitted as a
    /// leading TSDoc comment.
    pub docs: Vec<String>,
}

/// The payload shape of an enum variant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantKind {
    /// No payload.
    Unit,
    /// A single unnamed field, `Variant(T)`.
    Newtype(Ty),
    /// Multiple unnamed fields, `Variant(A, B)`.
    Tuple(Vec<Ty>),
    /// Named fields, `Variant { a: A, b: B }`.
    Struct(Vec<Field>),
}

/// An enum variant, lowered to its TypeScript shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    /// The variant's name as written in Rust.
    pub rust_name: String,
    /// The variant's name on the wire, after serde/`#[ts(rename...)]` renaming.
    pub ts_name: String,
    /// Doc comment lines carried over from the Rust variant, emitted as a
    /// leading TSDoc comment.
    pub docs: Vec<String>,
    /// The variant's payload shape.
    pub kind: VariantKind,
    /// `#[serde(untagged)]` on this variant alone: it serialises as its bare
    /// payload even inside an otherwise tagged enum.
    pub untagged: bool,
}

/// How an enum's variants are tagged on the wire, mirroring serde's four
/// enum representations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumRepr {
    /// serde's default: each variant is its own object keyed by variant name,
    /// `{ "VariantName": payload }`.
    External,
    /// `#[serde(tag = "t")]`: the variant name is a field on the payload object.
    Internal {
        /// The tag field's name.
        tag: String,
    },
    /// `#[serde(tag = "t", content = "c")]`: the variant name and payload are
    /// sibling fields of a wrapper object.
    Adjacent {
        /// The tag field's name.
        tag: String,
        /// The payload field's name.
        content: String,
    },
    /// `#[serde(untagged)]`: variants are indistinguishable except by shape.
    Untagged,
    /// Selected only when the enum would be `External` *and* every variant is a
    /// unit variant. An all-unit enum carrying `#[serde(tag = "t")]` stays
    /// `Internal`.
    UnitOnly,
}

/// The shape of a declaration's body, lowered from a Rust struct or enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    /// A struct with named fields; renders as an `interface` or object `type`.
    Struct(Vec<Field>),
    /// A single-field tuple struct; renders as its inner type.
    Newtype(Ty),
    /// A multi-field tuple struct; renders as a tuple type.
    Tuple(Vec<Ty>),
    /// A unit struct; renders as `null`.
    Unit,
    /// An enum; renders as a union per its [`EnumRepr`].
    Enum {
        /// How variants are tagged on the wire.
        repr: EnumRepr,
        /// The enum's variants.
        variants: Vec<Variant>,
    },
}

/// A single generated declaration: one Rust struct or enum, lowered to the
/// shape [`crate::render`] turns into a `.ts` file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    /// The fully-qualified Rust path the declaration was parsed from.
    pub rust_path: String,
    /// The emitted TypeScript name, and the file it is written to.
    pub ts_name: String,
    /// Doc comment lines carried over from the Rust item, emitted as a
    /// leading TSDoc comment.
    pub docs: Vec<String>,
    /// Names of the declaration's generic type parameters, in order.
    pub generics: Vec<String>,
    /// The declaration's shape.
    pub body: Body,
    /// Where in the source this declaration was parsed from.
    pub loc: SourceLoc,
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Trace,
    Any,
}

#[cfg(feature = "axum")]
impl Method {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Trace => "TRACE",
            Self::Any => "ANY",
        }
    }
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RouteParam {
    pub name: String,
    pub ty: Ty,
    pub wildcard: bool,
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RouteBody {
    Typed(Ty),
    FormData,
    Raw,
    Text,
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    pub method: Method,
    pub path: String,
    pub ts_name: String,
    pub group: Vec<String>,
    pub params: Vec<RouteParam>,
    pub query: Option<Ty>,
    pub body: Option<RouteBody>,
    pub response: Ty,
    pub error: Option<Ty>,
    pub query_arrays: Option<String>,
    pub docs: Vec<String>,
    pub loc: SourceLoc,
}

#[cfg(feature = "axum")]
#[derive(Debug, Clone)]
pub struct RouteClash {
    pub key: String,
    pub existing: SourceLoc,
    pub incoming: SourceLoc,
}

/// Two declarations claimed the same TypeScript name.
#[derive(Debug, Clone)]
pub struct NameClash {
    /// The TypeScript name both declarations claimed.
    pub ts_name: String,
    /// Where the declaration already in the registry was parsed from.
    pub existing: SourceLoc,
    /// Where the declaration that lost the clash was parsed from.
    pub incoming: SourceLoc,
}

/// All declarations collected from a scan, keyed by their emitted TypeScript
/// name so collisions can be detected as they're inserted.
#[derive(Debug, Default)]
pub struct Registry {
    decls: Vec<Decl>,
    by_ts_name: HashMap<String, usize>,
    #[cfg(feature = "axum")]
    routes: Vec<Route>,
    #[cfg(feature = "axum")]
    route_keys: HashMap<String, usize>,
}

impl Registry {
    /// Adds a declaration, failing if its TypeScript name is already taken.
    pub fn insert(&mut self, decl: Decl) -> Result<(), NameClash> {
        if let Some(&idx) = self.by_ts_name.get(&decl.ts_name) {
            return Err(NameClash {
                ts_name: decl.ts_name.clone(),
                existing: self.decls[idx].loc.clone(),
                incoming: decl.loc,
            });
        }
        self.by_ts_name
            .insert(decl.ts_name.clone(), self.decls.len());
        self.decls.push(decl);
        Ok(())
    }

    /// Finds a declaration by its emitted TypeScript name.
    pub fn lookup(&self, ts_name: &str) -> Option<&Decl> {
        self.by_ts_name.get(ts_name).map(|&i| &self.decls[i])
    }

    /// All declarations, in insertion order.
    pub fn decls(&self) -> &[Decl] {
        &self.decls
    }

    /// Adds a route, failing if its `METHOD /path` key is already taken — axum
    /// panics on a duplicate at `Router` construction.
    #[cfg(feature = "axum")]
    pub fn insert_route(&mut self, route: Route) -> Result<(), RouteClash> {
        let key = format!("{} {}", route.method.as_str(), route.path);
        if let Some(&idx) = self.route_keys.get(&key) {
            return Err(RouteClash { key, existing: self.routes[idx].loc.clone(), incoming: route.loc });
        }
        self.route_keys.insert(key, self.routes.len());
        self.routes.push(route);
        Ok(())
    }

    /// All routes, in insertion order.
    #[cfg(feature = "axum")]
    pub fn routes(&self) -> &[Route] {
        &self.routes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(ts_name: &str) -> Decl {
        Decl {
            rust_path: format!("crate::{ts_name}"),
            ts_name: ts_name.to_string(),
            docs: vec![],
            generics: vec![],
            body: Body::Unit,
            loc: SourceLoc {
                file: "x.rs".into(),
                line: 1,
            },
        }
    }

    #[test]
    fn registry_looks_up_by_ts_name() {
        let mut reg = Registry::default();
        reg.insert(decl("User")).unwrap();
        assert_eq!(reg.lookup("User").unwrap().rust_path, "crate::User");
        assert!(reg.lookup("Missing").is_none());
    }

    #[test]
    fn registry_reports_duplicate_ts_names_with_both_locations() {
        let mut reg = Registry::default();
        reg.insert(decl("User")).unwrap();
        let mut second = decl("User");
        second.rust_path = "crate::other::User".into();
        second.loc = SourceLoc {
            file: "y.rs".into(),
            line: 9,
        };
        let clash = reg.insert(second).expect_err("second insert must clash");
        assert_eq!(clash.existing.file.to_str().unwrap(), "x.rs");
        assert_eq!(clash.incoming.line, 9);
    }

    #[test]
    fn int_width_is_preserved_not_collapsed_to_number() {
        let ty = Ty::Prim(Prim::Int {
            bits: 64,
            signed: false,
        });
        assert_eq!(
            ty,
            Ty::Prim(Prim::Int {
                bits: 64,
                signed: false
            })
        );
        assert_ne!(
            ty,
            Ty::Prim(Prim::Int {
                bits: 32,
                signed: false
            })
        );
    }
}

#[cfg(all(test, feature = "axum"))]
mod route_tests {
    use super::*;

    fn route(method: Method, path: &str) -> Route {
        Route {
            method,
            path: path.to_string(),
            ts_name: "getThing".into(),
            group: vec!["thing".into()],
            params: vec![],
            query: None,
            body: None,
            response: Ty::Prim(Prim::Null),
            error: None,
            query_arrays: None,
            docs: vec![],
            loc: SourceLoc { file: "h.rs".into(), line: 3 },
        }
    }

    #[test]
    fn methods_render_as_uppercase_wire_names() {
        assert_eq!(Method::Get.as_str(), "GET");
        assert_eq!(Method::Any.as_str(), "ANY");
    }

    #[test]
    fn the_same_path_under_different_methods_is_not_a_clash() {
        let mut reg = Registry::default();
        reg.insert_route(route(Method::Get, "/a")).unwrap();
        reg.insert_route(route(Method::Post, "/a")).expect("different method is fine");
        assert_eq!(reg.routes().len(), 2);
    }

    #[test]
    fn a_duplicate_method_and_path_clashes_with_both_locations() {
        let mut reg = Registry::default();
        reg.insert_route(route(Method::Get, "/a")).unwrap();
        let mut second = route(Method::Get, "/a");
        second.loc = SourceLoc { file: "other.rs".into(), line: 9 };
        let clash = reg.insert_route(second).expect_err("duplicate must clash");
        assert_eq!(clash.key, "GET /a");
        assert_eq!(clash.existing.line, 3);
        assert_eq!(clash.incoming.line, 9);
    }

    #[test]
    fn a_trailing_slash_is_a_distinct_route_as_in_axum() {
        let mut reg = Registry::default();
        reg.insert_route(route(Method::Get, "/a")).unwrap();
        reg.insert_route(route(Method::Get, "/a/")).expect("axum treats these as distinct");
    }
}
