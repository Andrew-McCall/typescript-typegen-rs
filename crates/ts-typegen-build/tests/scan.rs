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
    let reg = scan_roots(&roots, &LowerCtx::default(), &mut diags);
    (reg, diags)
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
fn unparseable_file_produces_a_diagnostic_rather_than_a_panic() {
    let dir = fixture(&[("lib.rs", "this is not rust !!!")]);
    let (_, diags) = scan(&dir);
    assert!(diags.iter().any(|d| d.message.contains("failed to parse")));
}
