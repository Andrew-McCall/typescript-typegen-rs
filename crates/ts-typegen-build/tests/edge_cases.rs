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
