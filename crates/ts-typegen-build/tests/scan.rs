use std::fs;
use std::path::PathBuf;

use tempfile::TempDir;
use ts_typegen_build::diag::Diagnostics;
use ts_typegen_build::ir::Registry;
use ts_typegen_build::lower::LowerCtx;
use ts_typegen_build::scan::scan_roots;

fn fixture(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, body) in files {
        let full = dir.path().join(path);
        fs::create_dir_all(full.parent().unwrap()).unwrap();
        fs::write(full, body).unwrap();
    }
    dir
}

fn scan(dir: &TempDir) -> (Registry, Diagnostics) {
    let mut diags = Diagnostics::default();
    let roots: Vec<PathBuf> = vec![dir.path().to_path_buf()];
    let out = scan_roots(&roots, &LowerCtx::default(), &mut diags);
    (out.registry, diags)
}

#[test]
fn finds_only_types_carrying_the_ts_derive() {
    let dir = fixture(&[(
        "lib.rs",
        "#[derive(Ts)] pub struct A { x: u8 }\n\
         #[derive(Debug)] pub struct B { x: u8 }\n",
    )]);
    let (reg, diags) = scan(&dir);
    assert!(diags.is_empty());
    assert!(reg.lookup("A").is_some());
    assert!(reg.lookup("B").is_none());
}

#[test]
fn follows_mod_declarations_and_records_module_paths() {
    let dir = fixture(&[
        ("lib.rs", "mod user;\nmod nested;\n"),
        ("user.rs", "#[derive(Ts)] pub struct User { x: u8 }\n"),
        ("nested/mod.rs", "pub mod deep;\n"),
        (
            "nested/deep.rs",
            "#[derive(Ts)] pub struct Deep { x: u8 }\n",
        ),
    ]);
    let (reg, diags) = scan(&dir);
    assert!(diags.is_empty());
    assert_eq!(reg.lookup("User").unwrap().rust_path, "crate::user::User");
    assert_eq!(
        reg.lookup("Deep").unwrap().rust_path,
        "crate::nested::deep::Deep"
    );
}

#[test]
fn walks_inline_modules() {
    let dir = fixture(&[(
        "lib.rs",
        "pub mod inner { #[derive(Ts)] pub struct Inner { x: u8 } }\n",
    )]);
    let (reg, _) = scan(&dir);
    assert_eq!(
        reg.lookup("Inner").unwrap().rust_path,
        "crate::inner::Inner"
    );
}

#[test]
fn duplicate_ts_names_are_reported_not_silently_merged() {
    let dir = fixture(&[
        ("lib.rs", "mod a;\nmod b;\n"),
        ("a.rs", "#[derive(Ts)] pub struct Config { x: u8 }\n"),
        ("b.rs", "#[derive(Ts)] pub struct Config { y: u8 }\n"),
    ]);
    let (_, diags) = scan(&dir);
    assert!(
        diags
            .iter()
            .any(|d| d.message.contains("duplicate TypeScript name `Config`")),
        "expected a duplicate-name diagnostic"
    );
}

#[test]
fn cfg_gated_modules_are_still_scanned() {
    let dir = fixture(&[(
        "lib.rs",
        "#[cfg(feature = \"never-enabled\")] \
         pub mod gated { #[derive(Ts)] pub struct Gated { x: u8 } }\n",
    )]);
    let (reg, _) = scan(&dir);
    assert!(
        reg.lookup("Gated").is_some(),
        "cfg-gated types must still be emitted"
    );
}

#[test]
fn overlapping_scan_roots_do_not_duplicate_a_type() {
    // Two configured scan roots can resolve to the same file -- e.g.
    // `.scan("src").scan("src/models")` when "src" already reaches
    // "src/models" via `mod`. The same physical file must only be walked
    // once, or its types would collide with themselves.
    let dir = fixture(&[
        ("lib.rs", "mod nested;\n"),
        ("nested.rs", "#[derive(Ts)] pub struct Deep { x: u8 }\n"),
    ]);
    let mut diags = Diagnostics::default();
    let roots: Vec<PathBuf> = vec![dir.path().to_path_buf(), dir.path().to_path_buf()];
    let reg = scan_roots(&roots, &LowerCtx::default(), &mut diags).registry;

    assert!(diags.is_empty(), "expected no duplicate-name diagnostic: {diags:?}");
    assert_eq!(reg.decls().len(), 1, "Deep must be registered exactly once");
    assert!(reg.lookup("Deep").is_some());
}

#[test]
fn modern_2018_style_sibling_directory_modules_resolve() {
    // `api.rs` declaring `mod widgets;` resolves to `api/widgets.rs`, not
    // `widgets.rs` next to `api.rs` -- the common modern convention, distinct
    // from the mod.rs-based layout the other module tests use.
    let dir = fixture(&[
        ("lib.rs", "mod api;\n"),
        ("api.rs", "pub mod widgets;\n#[derive(Ts)] pub struct Api { x: u8 }\n"),
        ("api/widgets.rs", "#[derive(Ts)] pub struct Widget { x: u8 }\n"),
    ]);
    let (reg, diags) = scan(&dir);
    assert!(diags.is_empty(), "{diags:?}");
    assert_eq!(reg.lookup("Api").unwrap().rust_path, "crate::api::Api");
    assert_eq!(
        reg.lookup("Widget").unwrap().rust_path,
        "crate::api::widgets::Widget"
    );
}

#[test]
fn a_mod_declaration_with_no_matching_file_is_silently_skipped() {
    // A module gated out on this platform/feature set has no file to find;
    // syn still sees the `mod` item since #[cfg] isn't evaluated, so this
    // must degrade gracefully rather than error or panic.
    let dir = fixture(&[
        ("lib.rs", "mod missing;\nmod present;\n"),
        ("present.rs", "#[derive(Ts)] pub struct Present { x: u8 }\n"),
    ]);
    let (reg, diags) = scan(&dir);
    assert!(diags.is_empty(), "a missing mod file must not be an error: {diags:?}");
    assert!(reg.lookup("Present").is_some(), "sibling modules must still be scanned");
    assert_eq!(reg.decls().len(), 1);
}

#[test]
fn unparseable_file_produces_a_diagnostic_rather_than_a_panic() {
    let dir = fixture(&[("lib.rs", "this is not rust !!!")]);
    let (_, diags) = scan(&dir);
    assert!(diags.iter().any(|d| d.message.contains("failed to parse")));
}

#[test]
fn scanning_collects_handler_fns_with_their_module_path_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("handlers/patient")).unwrap();
    std::fs::write(src.join("lib.rs"), "pub mod handlers;\n").unwrap();
    std::fs::write(src.join("handlers.rs"), "pub mod patient;\n").unwrap();
    std::fs::write(src.join("handlers/patient.rs"), "pub mod get_patient;\n").unwrap();
    std::fs::write(
        src.join("handlers/patient/get_patient.rs"),
        "pub async fn get_patient(id: i64) -> i64 { id }\n",
    )
    .unwrap();

    let mut diags = Diagnostics::default();
    let out = scan_roots(std::slice::from_ref(&src), &LowerCtx::default(), &mut diags);

    let f = out.fns.iter().find(|f| f.item.sig.ident == "get_patient").expect("fn collected");
    assert_eq!(f.module_path, "crate::handlers::patient::get_patient");
    assert!(f.file.ends_with("handlers/patient/get_patient.rs"));
}
