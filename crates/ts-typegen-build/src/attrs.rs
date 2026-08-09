//! Reading `#[ts(...)]` and `#[serde(...)]` attributes.
//!
//! Both are parsed into one `Attrs`. Where they overlap, `ts` wins — it is the
//! escape hatch, so it must be able to contradict serde.


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

    /// Renames a struct field or a named variant field.
    ///
    /// This is a direct port of serde's own `apply_to_field`, not a generic
    /// case converter, and the difference is load-bearing: serde assumes the
    /// input is already `snake_case` and does character substitution, whereas a
    /// word-segmenting converter silently drops trailing and doubled
    /// underscores. `type_` must become `type-` under kebab-case, not `type`.
    pub fn apply_to_field(&self, field: &str) -> String {
        match self {
            // serde treats the field as already being in this shape.
            Self::Lower | Self::Snake => field.to_owned(),
            Self::Upper | Self::ScreamingSnake => field.to_ascii_uppercase(),
            Self::Pascal => {
                let mut out = String::with_capacity(field.len());
                let mut capitalize = true;
                for ch in field.chars() {
                    if ch == '_' {
                        capitalize = true;
                    } else if capitalize {
                        out.extend(ch.to_uppercase());
                        capitalize = false;
                    } else {
                        out.push(ch);
                    }
                }
                out
            }
            Self::Camel => lower_first(Self::Pascal.apply_to_field(field)),
            Self::Kebab => field.replace('_', "-"),
            Self::ScreamingKebab => {
                let mut out = field.replace('_', "-");
                out.make_ascii_uppercase();
                out
            }
        }
    }

    /// Renames an enum variant. Ported from serde's `apply_to_variant`, which
    /// assumes the input is `PascalCase` — the opposite starting point from
    /// fields, which is why serde keeps the two conversions separate.
    pub fn apply_to_variant(&self, variant: &str) -> String {
        match self {
            Self::Pascal => variant.to_owned(),
            Self::Lower => variant.to_ascii_lowercase(),
            Self::Upper => variant.to_ascii_uppercase(),
            Self::Camel => lower_first(variant.to_owned()),
            Self::Snake => split_capitals(variant, '_'),
            Self::Kebab => split_capitals(variant, '-'),
            Self::ScreamingSnake => shout(split_capitals(variant, '_')),
            Self::ScreamingKebab => shout(split_capitals(variant, '-')),
        }
    }
}

/// Splits a `PascalCase` variant at each capital, joining the words with `sep`
/// and lowercasing as it goes.
///
/// The separator is a parameter rather than a post-hoc `replace`, so kebab-case
/// is built directly in one pass instead of building snake_case and rewriting
/// it. Uppercasing is left to [`shout`], which works in place.
///
/// An underscore already present in the variant is emitted as `sep` too. That
/// looks like a detail and is not: serde derives kebab-case by replacing *every*
/// underscore in the snake_case form, so `Key_Press` is `key--press`. Passing
/// the separator through without this line silently produces `key_-press`.
fn split_capitals(variant: &str, sep: char) -> String {
    // Most identifiers gain one or two separators; over-reserving beats
    // reallocating mid-loop.
    let mut out = String::with_capacity(variant.len() + 4);
    for (i, ch) in variant.char_indices() {
        if ch == '_' {
            out.push(sep);
            continue;
        }
        if i > 0 && ch.is_uppercase() {
            out.push(sep);
        }
        out.extend(ch.to_lowercase());
    }
    out
}

/// Uppercases in place. Both separators are ASCII and unaffected, so this is
/// safe to apply after splitting and costs no allocation.
fn shout(mut s: String) -> String {
    s.make_ascii_uppercase();
    s
}

/// Lowercases the first character, reusing the buffer when it is ASCII — which
/// covers every practical identifier. The non-ASCII path has to rebuild, since
/// lowercasing can change a character's UTF-8 length.
fn lower_first(mut s: String) -> String {
    match s.chars().next() {
        Some(c) if c.is_ascii() => {
            s[..1].make_ascii_lowercase();
            s
        }
        Some(c) => {
            let mut out = String::with_capacity(s.len());
            out.extend(c.to_lowercase());
            out.push_str(&s[c.len_utf8()..]);
            out
        }
        None => s,
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
    /// `#[ts(readonly)]` — emit `readonly` on this field, or on every field of
    /// this type when set at container level.
    pub readonly: bool,
    /// `#[ts(inline)]` — splice the referenced type's shape in place of a
    /// reference, instead of importing it.
    pub inline: bool,
    /// `#[serde(with)]` / `#[serde(serialize_with)]` — the wire shape is decided
    /// by user code we cannot see, so the Rust type no longer describes it.
    pub custom_ser: bool,
}

// `#[serde(default)]` is deliberately not tracked: it affects deserialization
// only. A defaulted field is still always written during serialization, so it
// cannot change the shape we emit. Use `#[ts(optional)]` to mark a field
// optional regardless.

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
            if let Some(v) = value().or_else(|| nested_serialize_value(meta)) {
                out.rename = Some(v);
            }
        }
        "rename_all" => {
            if let Some(r) =
                value().or_else(|| nested_serialize_value(meta)).and_then(|v| RenameRule::parse(&v))
            {
                out.rename_all = Some(r);
            }
        }
        "rename_all_fields" => {
            if let Some(r) =
                value().or_else(|| nested_serialize_value(meta)).and_then(|v| RenameRule::parse(&v))
            {
                out.rename_all_fields = Some(r);
            }
        }
        "with" | "serialize_with" => out.custom_ser = true,
        "readonly" if is_ts => out.readonly = true,
        "inline" if is_ts => out.inline = true,
        "tag" => out.tag = value(),
        "content" => out.content = value(),
        "untagged" => out.untagged = true,
        "transparent" => out.transparent = true,
        "flatten" => out.flatten = true,
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

/// Reads the `serialize = "..."` arm of serde's nested form, e.g.
/// `rename(serialize = "a", deserialize = "b")`. The deserialize arm is
/// deliberately dropped: it does not describe the shape we emit.
fn nested_serialize_value(meta: &syn::meta::ParseNestedMeta<'_>) -> Option<String> {
    let mut found = None;
    meta.parse_nested_meta(|inner| {
        let is_serialize = inner.path.is_ident("serialize");
        // Every arm's literal must be parsed, not just its `=`. Leaving the
        // deserialize arm's value on the stream makes syn stop at "expected `,`"
        // and discard the serialize value already read.
        if let Ok(lit) = inner.value().and_then(|v| v.parse::<syn::LitStr>())
            && is_serialize
        {
            found = Some(lit.value());
        }
        Ok(())
    })
    .ok()?;
    found
}

pub fn doc_lines(attrs: &[syn::Attribute]) -> Vec<String> {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("doc"))
        .filter_map(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value().trim().to_string()),
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
    fn field_rules_convert_from_snake_case() {
        assert_eq!(RenameRule::Camel.apply_to_field("user_id"), "userId");
        assert_eq!(RenameRule::Pascal.apply_to_field("user_id"), "UserId");
        assert_eq!(RenameRule::Kebab.apply_to_field("user_id"), "user-id");
        assert_eq!(RenameRule::ScreamingSnake.apply_to_field("user_id"), "USER_ID");
        assert_eq!(RenameRule::ScreamingKebab.apply_to_field("user_id"), "USER-ID");
        assert_eq!(RenameRule::Upper.apply_to_field("user_id"), "USER_ID");
        // serde treats a field as already snake_case, so these are no-ops.
        assert_eq!(RenameRule::Snake.apply_to_field("user_id"), "user_id");
        assert_eq!(RenameRule::Lower.apply_to_field("user_id"), "user_id");
    }

    #[test]
    fn variant_rules_convert_from_pascal_case() {
        assert_eq!(RenameRule::Snake.apply_to_variant("KeyPress"), "key_press");
        assert_eq!(RenameRule::Camel.apply_to_variant("KeyPress"), "keyPress");
        assert_eq!(RenameRule::Kebab.apply_to_variant("KeyPress"), "key-press");
        assert_eq!(RenameRule::ScreamingSnake.apply_to_variant("KeyPress"), "KEY_PRESS");
        assert_eq!(RenameRule::ScreamingKebab.apply_to_variant("KeyPress"), "KEY-PRESS");
        assert_eq!(RenameRule::Lower.apply_to_variant("KeyPress"), "keypress");
        assert_eq!(RenameRule::Upper.apply_to_variant("KeyPress"), "KEYPRESS");
        assert_eq!(RenameRule::Pascal.apply_to_variant("KeyPress"), "KeyPress");
    }

    #[test]
    fn lowercasing_the_first_char_handles_non_ascii_identifiers() {
        // Rust identifiers may be non-ASCII, so the in-place fast path must not
        // be the only path. `İ` is the nasty one: lowercasing it grows the
        // string from 2 bytes to 3.
        assert_eq!(RenameRule::Camel.apply_to_variant("Ünit"), "ünit");
        assert_eq!(RenameRule::Camel.apply_to_variant("İd"), "i\u{307}d");
        assert_eq!(RenameRule::Camel.apply_to_variant(""), "");
    }

    #[test]
    fn a_literal_underscore_becomes_the_separator_too() {
        // serde derives kebab-case by replacing every underscore in the
        // snake_case form, so both the literal `_` and the one inserted before
        // `P` become dashes. Verified against serde_json in case_fidelity.rs.
        assert_eq!(RenameRule::Kebab.apply_to_variant("Key_Press"), "key--press");
        assert_eq!(RenameRule::Snake.apply_to_variant("Key_Press"), "key__press");
    }

    #[test]
    fn field_and_variant_rules_are_not_interchangeable() {
        // The same rule on the same text must differ, because the two start
        // from opposite conventions. Collapsing them into one function is the
        // bug this split exists to prevent.
        assert_ne!(
            RenameRule::Snake.apply_to_field("KeyPress"),
            RenameRule::Snake.apply_to_variant("KeyPress")
        );
    }

    #[test]
    fn extracts_doc_comments_trimmed() {
        let item: syn::DeriveInput =
            syn::parse_str("/// Stable identifier.\n/// Second line.\nstruct S;").unwrap();
        assert_eq!(
            doc_lines(&item.attrs),
            vec!["Stable identifier.", "Second line."]
        );
    }

    #[test]
    fn ts_type_escape_hatch_is_captured() {
        let a = attrs_of("#[ts(type = \"Record<string, never>\")] struct S;");
        assert_eq!(a.ts_type.as_deref(), Some("Record<string, never>"));
    }
}

#[cfg(test)]
mod nested_and_new_attr_tests {
    use super::*;

    fn attrs_of(src: &str) -> Attrs {
        let item: syn::DeriveInput = syn::parse_str(src).expect("parse");
        parse_attrs(&item.attrs)
    }

    #[test]
    fn nested_rename_serialize_form_is_read() {
        // serde allows #[serde(rename(serialize = "..", deserialize = ".."))].
        // Only the serialize side describes the wire shape we emit.
        let a = attrs_of("#[serde(rename(serialize = \"Ser\", deserialize = \"De\"))] struct S;");
        assert_eq!(a.rename.as_deref(), Some("Ser"));
    }

    #[test]
    fn nested_rename_deserialize_only_is_ignored() {
        let a = attrs_of("#[serde(rename(deserialize = \"De\"))] struct S;");
        assert_eq!(a.rename, None, "deserialize-only rename must not change output");
    }

    #[test]
    fn nested_rename_all_serialize_form_is_read() {
        let a = attrs_of("#[serde(rename_all(serialize = \"camelCase\"))] struct S;");
        assert_eq!(a.rename_all, Some(RenameRule::Camel));
    }

    #[test]
    fn custom_serialisation_is_flagged() {
        assert!(attrs_of("#[serde(with = \"m\")] struct S;").custom_ser);
        assert!(attrs_of("#[serde(serialize_with = \"f\")] struct S;").custom_ser);
        assert!(!attrs_of("#[serde(deserialize_with = \"f\")] struct S;").custom_ser);
        assert!(!attrs_of("struct S;").custom_ser);
    }

    #[test]
    fn readonly_and_inline_flags_are_read() {
        assert!(attrs_of("#[ts(readonly)] struct S;").readonly);
        assert!(attrs_of("#[ts(inline)] struct S;").inline);
        assert!(!attrs_of("struct S;").readonly);
        assert!(!attrs_of("struct S;").inline);
    }

    #[test]
    fn readonly_and_inline_are_ts_only_not_serde() {
        // serde has no such options; a serde attr of the same name must not leak.
        assert!(!attrs_of("#[serde(readonly)] struct S;").readonly);
        assert!(!attrs_of("#[serde(inline)] struct S;").inline);
    }
}
