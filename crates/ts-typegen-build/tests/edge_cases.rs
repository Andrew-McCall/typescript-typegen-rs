//! Behaviours verified against serde's real output where the answer wasn't obvious.

use std::fs;

use tempfile::TempDir;
use ts_typegen_build::config::{Bytes, Config};
use ts_typegen_build::run::generate;

fn project(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, body).unwrap();
    }
    dir
}

fn build(src: &str) -> (TempDir, Config) {
    let dir = project(&[("src/lib.rs", src)]);
    let cfg = Config::new()
        .scan(dir.path().join("src"))
        .output(dir.path().join("bindings"))
        .always_run(true);
    (dir, cfg)
}

fn emit(src: &str) -> String {
    let (dir, cfg) = build(src);
    let out = generate(&cfg).expect("generation must succeed");
    assert!(out.warnings.is_empty(), "unexpected warnings: {:?}", out.warnings);
    fs::read_dir(dir.path().join("bindings"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "index.ts")
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn byte_arrays_are_number_arrays_by_default() {
    // serde_json writes Vec<u8> as [1,2], so number[] is the accurate default.
    assert!(emit("#[derive(Ts)] pub struct S { pub b: Vec<u8>, pub a: [u8; 3] }")
        .contains("b: number[];"));
}

#[test]
fn byte_arrays_render_as_string_when_configured() {
    let (dir, cfg) = build("#[derive(Ts)] pub struct S { pub b: Vec<u8>, pub n: Vec<u16> }");
    let cfg = cfg.bytes(Bytes::Str);
    generate(&cfg).unwrap();
    let s = fs::read_to_string(dir.path().join("bindings/S.ts")).unwrap();
    assert!(s.contains("b: string;"), "{s}");
    assert!(s.contains("n: number[];"), "only u8 arrays are byte arrays: {s}");
}

#[test]
fn a_flattened_map_becomes_an_intersection_with_record() {
    // serde merges the map's entries into the parent object.
    let s = emit(
        "#[derive(Ts)] pub struct S { pub id: u32, \
         #[serde(flatten)] pub rest: std::collections::HashMap<String, u32> }",
    );
    assert!(s.contains("Record<string, number> & {"), "{s}");
    assert!(s.contains("id: number;"), "{s}");
}

#[test]
fn a_variant_marked_untagged_drops_the_discriminant() {
    // serde emits Mixed::Loose("s") as a bare "s" even though the enum is tagged.
    let s = emit(
        "#[derive(Ts)] #[serde(tag = \"t\")] pub enum Mixed { \
         Tagged { x: u8 }, #[serde(untagged)] Loose(String) }",
    );
    assert!(s.contains("{ t: \"Tagged\"; x: number }"), "{s}");
    assert!(s.contains("| string"), "untagged variant must be a bare payload: {s}");
}

#[test]
fn serde_other_serialises_by_name_like_any_unit_variant() {
    // #[serde(other)] is deserialize-only; it must not change the emitted shape.
    let s = emit("#[derive(Ts)] pub enum E { A, #[serde(other)] Unknown }");
    assert!(s.contains(r#""A" | "Unknown""#), "{s}");
}

#[test]
fn skip_serializing_if_on_a_non_option_still_marks_the_key_optional() {
    // serde omits the key when the predicate passes, whatever the field type.
    let s = emit(
        "#[derive(Ts)] pub struct S { \
         #[serde(skip_serializing_if = \"Vec::is_empty\")] pub tags: Vec<String> }",
    );
    assert!(s.contains("tags?: string[];"), "{s}");
}

#[test]
fn nested_generic_arguments_survive() {
    let s = emit(
        "#[derive(Ts)] pub struct Page<T> { pub items: Vec<T> }\n\
         #[derive(Ts)] pub struct Wrap { pub p: Page<Vec<String>>, \
         pub q: Page<std::collections::HashMap<String, u8>> }",
    );
    assert!(s.contains("p: Page<string[]>;"), "{s}");
    assert!(s.contains("q: Page<Record<string, number>>;"), "{s}");
}

#[test]
fn scanning_a_root_with_no_types_warns_rather_than_silently_doing_nothing() {
    let (_dir, cfg) = build("pub struct NotMarked { pub v: u8 }\n");
    let out = generate(&cfg).expect("must not fail");
    assert!(
        out.warnings.iter().any(|w| w.contains("no #[derive(Ts)] types found")),
        "got {:?}",
        out.warnings
    );
}

#[test]
fn inlining_a_type_into_itself_warns_and_leaves_the_reference() {
    let (dir, cfg) = build(
        "#[derive(Ts)] pub struct Node { #[ts(inline)] pub next: Option<Box<Node>> }",
    );
    let out = generate(&cfg).expect("must not hang or fail");
    assert!(
        out.warnings.iter().any(|w| w.contains("cannot inline `Node` into itself")),
        "got {:?}",
        out.warnings
    );
    let s = fs::read_to_string(dir.path().join("bindings/Node.ts")).unwrap();
    assert!(s.contains("Node"), "{s}");
}

// --- integer width policy, unknown style, header ---

use ts_typegen_build::config::{BigInt, UnknownStyle};

fn emit_with(src: &str, f: impl FnOnce(Config) -> Config) -> String {
    let (dir, cfg) = build(src);
    generate(&f(cfg)).expect("generation must succeed");
    fs::read_dir(dir.path().join("bindings"))
        .unwrap()
        .flatten()
        .filter(|e| e.file_name() != "index.ts")
        .map(|e| fs::read_to_string(e.path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

const WIDTHS: &str = "#[derive(Ts)] pub struct S { pub a: i32, pub b: i64, \
                      pub c: u64, pub d: i128, pub e: usize }";

#[test]
fn every_wide_integer_is_a_number_by_default() {
    // serde_json writes all of these as JSON numbers.
    let s = emit_with(WIDTHS, |c| c);
    for field in ["a", "b", "c", "d", "e"] {
        assert!(s.contains(&format!("{field}: number;")), "{field} should be number: {s}");
    }
}

#[test]
fn int64_and_int128_are_configured_independently() {
    let s = emit_with(WIDTHS, |c| c.int64(BigInt::Number).int128(BigInt::Bigint));
    assert!(s.contains("b: number;"), "i64 stays number: {s}");
    assert!(s.contains("c: number;"), "u64 stays number: {s}");
    assert!(s.contains("e: number;"), "usize follows int64: {s}");
    assert!(s.contains("d: bigint;"), "i128 must be bigint: {s}");
    assert!(s.contains("a: number;"), "i32 is never affected: {s}");
}

#[test]
fn int64_can_be_bigint_while_int128_is_a_string() {
    let s = emit_with(WIDTHS, |c| c.int64(BigInt::Bigint).int128(BigInt::Str));
    assert!(s.contains("b: bigint;"), "{s}");
    assert!(s.contains("d: string;"), "{s}");
    assert!(s.contains("a: number;"), "32-bit is untouched: {s}");
}

#[test]
fn bigint_shorthand_still_sets_both_widths() {
    let s = emit_with(WIDTHS, |c| c.bigint(BigInt::Bigint));
    assert!(s.contains("b: bigint;") && s.contains("d: bigint;"), "{s}");
}

#[test]
fn opaque_values_are_unknown_by_default_and_any_on_request() {
    let src = "#[derive(Ts)] pub struct S { pub v: serde_json::Value }";
    assert!(emit_with(src, |c| c).contains("v: unknown;"));
    assert!(emit_with(src, |c| c.unknown_type(UnknownStyle::Any)).contains("v: any;"));
}

#[test]
fn a_preset_resolves_a_foreign_type_end_to_end() {
    use ts_typegen_build::Preset;
    let (dir, cfg) = build("#[derive(Ts)] pub struct S { pub id: uuid::Uuid, pub at: chrono::DateTime }");
    generate(&cfg.preset(Preset::Common)).unwrap();
    let s = fs::read_to_string(dir.path().join("bindings/S.ts")).unwrap();
    assert!(s.contains("id: string;"), "{s}");
    assert!(s.contains("at: string;"), "{s}");
    // Presets install a plain map_type entry; nothing about a foreign path
    // should produce an import.
    assert!(!s.contains("import"), "{s}");
}

#[test]
fn map_keys_render_as_string_regardless_of_the_rust_key_type() {
    let s = emit(
        "#[derive(Ts)] pub struct S { pub by_id: std::collections::HashMap<i32, String> }",
    );
    assert!(s.contains("by_id: Record<string, string>;"), "{s}");
}

#[test]
fn nonzero_integers_render_at_the_same_width_as_their_plain_counterpart() {
    let s = emit(
        "#[derive(Ts)] pub struct S { pub a: std::num::NonZeroU8, \
         pub b: std::num::NonZeroI64 }",
    );
    assert!(s.contains("a: number;"), "{s}");
    assert!(s.contains("b: number;"), "{s}");
}

#[test]
fn a_configured_header_follows_the_marker_in_every_file() {
    let (dir, cfg) = build("#[derive(Ts)] pub struct S { pub v: u8 }");
    generate(&cfg.header("/* eslint-disable */")).unwrap();
    for name in ["S.ts", "index.ts"] {
        let s = fs::read_to_string(dir.path().join("bindings").join(name)).unwrap();
        let mut lines = s.lines();
        // The marker must stay first: the orphan sweep keys off it.
        assert_eq!(lines.next().unwrap(), "// @generated by ts-typegen -- DO NOT EDIT");
        assert_eq!(lines.next().unwrap(), "/* eslint-disable */", "in {name}");
    }
}
