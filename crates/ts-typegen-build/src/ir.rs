//! Language-neutral intermediate representation.
//!
//! Everything upstream of this module understands `syn` and serde semantics and
//! nothing about TypeScript. Everything downstream understands TypeScript and
//! nothing about Rust.

use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceLoc {
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Prim {
    Str,
    Bool,
    Float,
    Int { bits: u8, signed: bool },
    Null,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Prim(Prim),
    Array(Box<Ty>),
    Tuple(Vec<Ty>),
    Record { key: Box<Ty>, value: Box<Ty> },
    Union(Vec<Ty>),
    Object(Vec<Field>),
    /// A reference to another generated type; becomes an import line.
    Ref { name: String, args: Vec<Ty> },
    /// A generic type parameter of the enclosing declaration.
    Param(String),
    /// Verbatim TypeScript from `#[ts(type = "...")]` or a config type mapping.
    Raw(String),
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub rust_name: String,
    pub ts_name: String,
    pub ty: Ty,
    pub optional: bool,
    pub flatten: bool,
    pub docs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariantKind {
    Unit,
    Newtype(Ty),
    Tuple(Vec<Ty>),
    Struct(Vec<Field>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variant {
    pub rust_name: String,
    pub ts_name: String,
    pub docs: Vec<String>,
    pub kind: VariantKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnumRepr {
    External,
    Internal { tag: String },
    Adjacent { tag: String, content: String },
    Untagged,
    /// Selected only when the enum would be `External` *and* every variant is a
    /// unit variant. An all-unit enum carrying `#[serde(tag = "t")]` stays
    /// `Internal`.
    UnitOnly,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Struct(Vec<Field>),
    Newtype(Ty),
    Tuple(Vec<Ty>),
    Unit,
    Enum { repr: EnumRepr, variants: Vec<Variant> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decl {
    pub rust_path: String,
    pub ts_name: String,
    pub docs: Vec<String>,
    pub generics: Vec<String>,
    pub body: Body,
    pub loc: SourceLoc,
}

/// Two declarations claimed the same TypeScript name.
#[derive(Debug, Clone)]
pub struct NameClash {
    pub ts_name: String,
    pub existing: SourceLoc,
    pub incoming: SourceLoc,
}

#[derive(Debug, Default)]
pub struct Registry {
    decls: Vec<Decl>,
    by_ts_name: HashMap<String, usize>,
}

impl Registry {
    pub fn insert(&mut self, decl: Decl) -> Result<(), NameClash> {
        if let Some(&idx) = self.by_ts_name.get(&decl.ts_name) {
            return Err(NameClash {
                ts_name: decl.ts_name.clone(),
                existing: self.decls[idx].loc.clone(),
                incoming: decl.loc,
            });
        }
        self.by_ts_name.insert(decl.ts_name.clone(), self.decls.len());
        self.decls.push(decl);
        Ok(())
    }

    pub fn lookup(&self, ts_name: &str) -> Option<&Decl> {
        self.by_ts_name.get(ts_name).map(|&i| &self.decls[i])
    }

    pub fn decls(&self) -> &[Decl] {
        &self.decls
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
            loc: SourceLoc { file: "x.rs".into(), line: 1 },
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
        second.loc = SourceLoc { file: "y.rs".into(), line: 9 };
        let clash = reg.insert(second).expect_err("second insert must clash");
        assert_eq!(clash.existing.file.to_str().unwrap(), "x.rs");
        assert_eq!(clash.incoming.line, 9);
    }

    #[test]
    fn int_width_is_preserved_not_collapsed_to_number() {
        let ty = Ty::Prim(Prim::Int { bits: 64, signed: false });
        assert_eq!(ty, Ty::Prim(Prim::Int { bits: 64, signed: false }));
        assert_ne!(ty, Ty::Prim(Prim::Int { bits: 32, signed: false }));
    }
}
