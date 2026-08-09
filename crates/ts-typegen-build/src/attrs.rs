//! Reading `#[ts(...)]` and `#[serde(...)]` attributes.
//!
//! Both are parsed into one `Attrs`. Where they overlap, `ts` wins — it is the
//! escape hatch, so it must be able to contradict serde.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameRule {
    Lower,
    Upper,
    Pascal,
    Camel,
    Snake,
    ScreamingSnake,
    Kebab,
    ScreamingKebab,
}

impl RenameRule {
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "lowercase" => Self::Lower,
            "UPPERCASE" => Self::Upper,
            "PascalCase" => Self::Pascal,
            "camelCase" => Self::Camel,
            "snake_case" => Self::Snake,
            "SCREAMING_SNAKE_CASE" => Self::ScreamingSnake,
            "kebab-case" => Self::Kebab,
            "SCREAMING-KEBAB-CASE" => Self::ScreamingKebab,
            _ => return None,
        })
    }

    pub fn apply(&self, ident: &str) -> String {
        match self {
            Self::Lower => ident.to_lowercase(),
            Self::Upper => ident.to_uppercase(),
            Self::Pascal => ident.to_upper_camel_case(),
            Self::Camel => ident.to_lower_camel_case(),
            Self::Snake => ident.to_snake_case(),
            Self::ScreamingSnake => ident.to_shouty_snake_case(),
            Self::Kebab => ident.to_kebab_case(),
            Self::ScreamingKebab => ident.to_kebab_case().to_uppercase(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Attrs {
    pub rename: Option<String>,
    pub rename_all: Option<RenameRule>,
    pub rename_all_fields: Option<RenameRule>,
    pub skip: bool,
    pub optional: bool,
    pub flatten: bool,
    pub transparent: bool,
    pub ts_type: Option<String>,
    pub tag: Option<String>,
    pub content: Option<String>,
    pub untagged: bool,
    pub has_default: bool,
}

pub fn parse_attrs(attrs: &[syn::Attribute]) -> Attrs {
    let mut out = Attrs::default();
    // serde first so that a later ts entry overwrites it.
    for (path, is_ts) in [("serde", false), ("ts", true)] {
        for attr in attrs.iter().filter(|a| a.path().is_ident(path)) {
            let _ = attr.parse_nested_meta(|meta| {
                apply_meta(&mut out, &meta, is_ts);
                Ok(())
            });
        }
    }
    out
}

fn apply_meta(out: &mut Attrs, meta: &syn::meta::ParseNestedMeta<'_>, is_ts: bool) {
    // `#[ts(type = "...")]` uses a keyword as a path segment, and `get_ident()`
    // returns None for keywords — read the last segment's ident directly.
    let key = meta
        .path
        .segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default();
    let value = || -> Option<String> {
        let v: syn::LitStr = meta.value().ok()?.parse().ok()?;
        Some(v.value())
    };
    match key.as_str() {
        "rename" => {
            if let Some(v) = value() {
                out.rename = Some(v);
            }
        }
        "rename_all" => out.rename_all = value().and_then(|v| RenameRule::parse(&v)),
        "rename_all_fields" => out.rename_all_fields = value().and_then(|v| RenameRule::parse(&v)),
        "tag" => out.tag = value(),
        "content" => out.content = value(),
        "untagged" => out.untagged = true,
        "transparent" => out.transparent = true,
        "flatten" => out.flatten = true,
        "default" => out.has_default = true,
        "optional" if is_ts => out.optional = true,
        "skip" | "skip_serializing" => out.skip = true,
        "skip_serializing_if" => {
            let _ = value();
            out.optional = true;
        }
        "type" | "as" if is_ts => out.ts_type = value(),
        _ => {}
    }
}

pub fn doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) => {
                    Some(s.value().trim().to_string())
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs_of(src: &str) -> Attrs {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        parse_attrs(&item.attrs)
    }

    #[test]
    fn reads_serde_rename_all() {
        let a = attrs_of("#[serde(rename_all = \"camelCase\")] struct S;");
        assert_eq!(a.rename_all, Some(RenameRule::Camel));
    }

    #[test]
    fn ts_overrides_serde() {
        let a = attrs_of("#[serde(rename = \"a\")] #[ts(rename = \"B\")] struct S;");
        assert_eq!(a.rename.as_deref(), Some("B"));
    }

    #[test]
    fn reads_enum_tagging() {
        let a = attrs_of("#[serde(tag = \"t\", content = \"c\")] enum E { A }");
        assert_eq!(a.tag.as_deref(), Some("t"));
        assert_eq!(a.content.as_deref(), Some("c"));
        assert!(attrs_of("#[serde(untagged)] enum E { A }").untagged);
    }

    #[test]
    fn skip_serializing_if_marks_field_optional() {
        let a = attrs_of("#[serde(skip_serializing_if = \"Option::is_none\")] struct S;");
        assert!(a.optional);
    }

    #[test]
    fn skip_variants_all_set_skip() {
        assert!(attrs_of("#[serde(skip)] struct S;").skip);
        assert!(attrs_of("#[serde(skip_serializing)] struct S;").skip);
        assert!(attrs_of("#[ts(skip)] struct S;").skip);
    }

    #[test]
    fn rename_rules_convert_correctly() {
        assert_eq!(RenameRule::Camel.apply("user_id"), "userId");
        assert_eq!(RenameRule::Pascal.apply("user_id"), "UserId");
        assert_eq!(RenameRule::Snake.apply("userId"), "user_id");
        assert_eq!(RenameRule::Kebab.apply("user_id"), "user-id");
        assert_eq!(RenameRule::ScreamingSnake.apply("user_id"), "USER_ID");
        assert_eq!(RenameRule::Lower.apply("UserId"), "userid");
        assert_eq!(RenameRule::Upper.apply("UserId"), "USERID");
    }

    #[test]
    fn extracts_doc_comments_trimmed() {
        let item: syn::DeriveInput =
            syn::parse_str("/// Stable identifier.\n/// Second line.\nstruct S;").unwrap();
        assert_eq!(doc_lines(&item.attrs), vec!["Stable identifier.", "Second line."]);
    }

    #[test]
    fn ts_type_escape_hatch_is_captured() {
        let a = attrs_of("#[ts(type = \"Record<string, never>\")] struct S;");
        assert_eq!(a.ts_type.as_deref(), Some("Record<string, never>"));
    }
}
