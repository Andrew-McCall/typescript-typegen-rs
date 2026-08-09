//! Asserts our rename rules produce exactly the keys serde produces.
//!
//! This is the same discipline as `serde_truth.rs`: compare against serde's
//! real output rather than against a belief about it. A generic word-segmenting
//! case converter passes casual inspection and still disagrees with serde on
//! trailing and doubled underscores — `type_` under kebab-case is `type-`, not
//! `type` — which would silently emit the wrong JSON key.

use serde::Serialize;
use ts_typegen_build::attrs::RenameRule;

/// Field names chosen to hit the edges: keyword-avoiding trailing underscores,
/// doubled underscores, digits, and single characters.
macro_rules! field_probe {
    ($name:ident, $lit:literal) => {
        #[derive(Serialize)]
        #[serde(rename_all = $lit)]
        #[allow(non_snake_case, dead_code)]
        struct $name {
            user_id: u8,
            xml_http_request: u8,
            field_2: u8,
            id: u8,
            oauth2_token: u8,
            multiple__underscores: u8,
            ref_: u8,
            a1b: u8,
            r#type: u8,
            r#match: u8,
        }
    };
}

const FIELDS: &[&str] = &[
    "user_id",
    "xml_http_request",
    "field_2",
    "id",
    "oauth2_token",
    "multiple__underscores",
    "ref_",
    "a1b",
    // Raw identifiers arrive unrawed: parse::unraw strips `r#` before any
    // rename rule runs, exactly as serde does.
    "type",
    "match",
];

field_probe!(FCamel, "camelCase");
field_probe!(FPascal, "PascalCase");
field_probe!(FKebab, "kebab-case");
field_probe!(FScreamSnake, "SCREAMING_SNAKE_CASE");
field_probe!(FScreamKebab, "SCREAMING-KEBAB-CASE");
field_probe!(FSnake, "snake_case");
field_probe!(FLower, "lowercase");
field_probe!(FUpper, "UPPERCASE");

macro_rules! variant_probe {
    ($name:ident, $lit:literal) => {
        #[derive(Serialize)]
        #[serde(rename_all = $lit)]
        #[allow(dead_code, non_camel_case_types)]
        enum $name {
            KeyPress,
            HTTPServer,
            A,
            Resize2D,
            Ok,
            Key_Press,
        }
    };
}

const VARIANTS: &[&str] =
    &["KeyPress", "HTTPServer", "A", "Resize2D", "Ok", "Key_Press"];

variant_probe!(VCamel, "camelCase");
variant_probe!(VPascal, "PascalCase");
variant_probe!(VKebab, "kebab-case");
variant_probe!(VScreamSnake, "SCREAMING_SNAKE_CASE");
variant_probe!(VScreamKebab, "SCREAMING-KEBAB-CASE");
variant_probe!(VSnake, "snake_case");
variant_probe!(VLower, "lowercase");
variant_probe!(VUpper, "UPPERCASE");

/// serde_json orders object keys, so compare sorted sets rather than zipping.
fn field_keys<T: Serialize>(v: T) -> Vec<String> {
    let json = serde_json::to_value(v).unwrap();
    let mut keys: Vec<String> = json.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    keys
}

fn ours_for_fields(rule: RenameRule) -> Vec<String> {
    let mut out: Vec<String> = FIELDS.iter().map(|f| rule.apply_to_field(f)).collect();
    out.sort();
    out
}

#[test]
fn field_renaming_matches_serde_exactly() {
    let cases: Vec<(&str, Vec<String>, RenameRule)> = vec![
        ("camelCase", field_keys(FCamel::default_probe()), RenameRule::Camel),
        ("PascalCase", field_keys(FPascal::default_probe()), RenameRule::Pascal),
        ("kebab-case", field_keys(FKebab::default_probe()), RenameRule::Kebab),
        (
            "SCREAMING_SNAKE_CASE",
            field_keys(FScreamSnake::default_probe()),
            RenameRule::ScreamingSnake,
        ),
        (
            "SCREAMING-KEBAB-CASE",
            field_keys(FScreamKebab::default_probe()),
            RenameRule::ScreamingKebab,
        ),
        ("snake_case", field_keys(FSnake::default_probe()), RenameRule::Snake),
        ("lowercase", field_keys(FLower::default_probe()), RenameRule::Lower),
        ("UPPERCASE", field_keys(FUpper::default_probe()), RenameRule::Upper),
    ];

    let mut bad = Vec::new();
    for (rule_name, serde_keys, rule) in cases {
        let ours = ours_for_fields(rule);
        if ours != serde_keys {
            bad.push(format!("  {rule_name}\n    serde={serde_keys:?}\n    ours ={ours:?}"));
        }
    }
    assert!(bad.is_empty(), "field renaming diverges from serde:\n{}", bad.join("\n"));
}

#[test]
fn variant_renaming_matches_serde_exactly() {
    fn variant_name<T: Serialize>(v: T) -> String {
        // A unit variant serialises to a bare string.
        serde_json::to_value(v).unwrap().as_str().unwrap().to_string()
    }

    let cases: Vec<(&str, Vec<String>, RenameRule)> = vec![
        (
            "camelCase",
            vec![
                variant_name(VCamel::KeyPress),
                variant_name(VCamel::HTTPServer),
                variant_name(VCamel::A),
                variant_name(VCamel::Resize2D),
                variant_name(VCamel::Ok),
                variant_name(VCamel::Key_Press),
            ],
            RenameRule::Camel,
        ),
        (
            "snake_case",
            vec![
                variant_name(VSnake::KeyPress),
                variant_name(VSnake::HTTPServer),
                variant_name(VSnake::A),
                variant_name(VSnake::Resize2D),
                variant_name(VSnake::Ok),
                variant_name(VSnake::Key_Press),
            ],
            RenameRule::Snake,
        ),
        (
            "kebab-case",
            vec![
                variant_name(VKebab::KeyPress),
                variant_name(VKebab::HTTPServer),
                variant_name(VKebab::A),
                variant_name(VKebab::Resize2D),
                variant_name(VKebab::Ok),
                variant_name(VKebab::Key_Press),
            ],
            RenameRule::Kebab,
        ),
        (
            "SCREAMING_SNAKE_CASE",
            vec![
                variant_name(VScreamSnake::KeyPress),
                variant_name(VScreamSnake::HTTPServer),
                variant_name(VScreamSnake::A),
                variant_name(VScreamSnake::Resize2D),
                variant_name(VScreamSnake::Ok),
                variant_name(VScreamSnake::Key_Press),
            ],
            RenameRule::ScreamingSnake,
        ),
        (
            "SCREAMING-KEBAB-CASE",
            vec![
                variant_name(VScreamKebab::KeyPress),
                variant_name(VScreamKebab::HTTPServer),
                variant_name(VScreamKebab::A),
                variant_name(VScreamKebab::Resize2D),
                variant_name(VScreamKebab::Ok),
                variant_name(VScreamKebab::Key_Press),
            ],
            RenameRule::ScreamingKebab,
        ),
        (
            "PascalCase",
            vec![
                variant_name(VPascal::KeyPress),
                variant_name(VPascal::HTTPServer),
                variant_name(VPascal::A),
                variant_name(VPascal::Resize2D),
                variant_name(VPascal::Ok),
                variant_name(VPascal::Key_Press),
            ],
            RenameRule::Pascal,
        ),
        (
            "lowercase",
            vec![
                variant_name(VLower::KeyPress),
                variant_name(VLower::HTTPServer),
                variant_name(VLower::A),
                variant_name(VLower::Resize2D),
                variant_name(VLower::Ok),
                variant_name(VLower::Key_Press),
            ],
            RenameRule::Lower,
        ),
        (
            "UPPERCASE",
            vec![
                variant_name(VUpper::KeyPress),
                variant_name(VUpper::HTTPServer),
                variant_name(VUpper::A),
                variant_name(VUpper::Resize2D),
                variant_name(VUpper::Ok),
                variant_name(VUpper::Key_Press),
            ],
            RenameRule::Upper,
        ),
    ];

    let mut bad = Vec::new();
    for (rule_name, serde_names, rule) in cases {
        let ours: Vec<String> = VARIANTS.iter().map(|v| rule.apply_to_variant(v)).collect();
        if ours != serde_names {
            bad.push(format!("  {rule_name}\n    serde={serde_names:?}\n    ours ={ours:?}"));
        }
    }
    assert!(bad.is_empty(), "variant renaming diverges from serde:\n{}", bad.join("\n"));
}

/// The specific case a word-segmenting converter gets wrong.
#[test]
fn trailing_and_doubled_underscores_survive() {
    assert_eq!(RenameRule::Kebab.apply_to_field("type_"), "type-");
    assert_eq!(RenameRule::Kebab.apply_to_field("multiple__underscores"), "multiple--underscores");
    assert_eq!(RenameRule::ScreamingSnake.apply_to_field("type_"), "TYPE_");
}

macro_rules! impl_default_probe {
    ($($name:ident),* $(,)?) => {
        $(impl $name {
            fn default_probe() -> Self {
                $name {
                    user_id: 0, xml_http_request: 0, field_2: 0, id: 0,
                    oauth2_token: 0, multiple__underscores: 0, ref_: 0, a1b: 0,
                    r#type: 0, r#match: 0,
                }
            }
        })*
    };
}

impl_default_probe!(FCamel, FPascal, FKebab, FScreamSnake, FScreamKebab, FSnake, FLower, FUpper);
