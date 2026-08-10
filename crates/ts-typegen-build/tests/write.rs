use std::collections::BTreeMap;
use std::fs;

use ts_typegen_build::config::Config;
use ts_typegen_build::render::MARKER;
use ts_typegen_build::write::{validate_paths, write_all};

fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

fn generated(body: &str) -> String {
    format!("{MARKER}\n{body}")
}

#[test]
fn writes_files_and_creates_the_output_directory() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("nested/bindings");
    let cfg = Config::new().output(&out);
    let report =
        write_all(&files(&[("A.ts", &generated("export type A = null;\n"))]), &cfg).unwrap();
    assert_eq!(report.written, vec!["A.ts"]);
    assert!(out.join("A.ts").is_file());
}

#[test]
fn identical_content_is_not_rewritten() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new().output(dir.path());
    let f = files(&[("A.ts", &generated("export type A = null;\n"))]);
    write_all(&f, &cfg).unwrap();
    let report = write_all(&f, &cfg).unwrap();
    assert!(report.written.is_empty(), "second write must be a no-op");
    assert_eq!(report.unchanged, vec!["A.ts"]);
}

#[test]
fn orphaned_generated_files_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new().output(dir.path());
    write_all(&files(&[("Old.ts", &generated("export type Old = null;\n"))]), &cfg).unwrap();
    let report =
        write_all(&files(&[("New.ts", &generated("export type New = null;\n"))]), &cfg).unwrap();
    assert_eq!(report.removed, vec!["Old.ts"]);
    assert!(!dir.path().join("Old.ts").exists());
    assert!(dir.path().join("New.ts").is_file());
}

#[test]
fn hand_written_files_are_never_removed() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("helpers.ts"), "export const x = 1;\n").unwrap();
    let cfg = Config::new().output(dir.path());
    let report =
        write_all(&files(&[("A.ts", &generated("export type A = null;\n"))]), &cfg).unwrap();
    assert!(report.removed.is_empty());
    assert!(dir.path().join("helpers.ts").is_file());
}

#[test]
fn non_ts_files_are_ignored_by_the_sweep() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("notes.md"), format!("{MARKER}\nnot typescript\n")).unwrap();
    let cfg = Config::new().output(dir.path());
    write_all(&files(&[("A.ts", &generated("export type A = null;\n"))]), &cfg).unwrap();
    assert!(dir.path().join("notes.md").is_file());
}

#[test]
fn output_inside_a_scan_root_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let cfg = Config::new().scan(&src).output(src.join("bindings"));
    let err = validate_paths(&cfg).expect_err("must reject nested output");
    assert!(format!("{err}").contains("output directory"));
}

#[test]
fn output_equal_to_a_scan_root_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    let cfg = Config::new().scan(&src).output(&src);
    assert!(validate_paths(&cfg).is_err());
}

#[test]
fn disjoint_paths_validate() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    let out = dir.path().join("bindings");
    fs::create_dir_all(&src).unwrap();
    let cfg = Config::new().scan(&src).output(&out);
    assert!(validate_paths(&cfg).is_ok());
}

#[test]
fn check_mode_reports_drift_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new().output(dir.path());
    write_all(&files(&[("A.ts", &generated("export type A = null;\n"))]), &cfg).unwrap();

    let check = Config::new().output(dir.path()).check(true);
    let err = write_all(&files(&[("A.ts", &generated("export type A = string;\n"))]), &check)
        .expect_err("drift must fail in check mode");
    let msg = format!("{err}");
    assert!(msg.contains("out of date"));
    assert!(msg.contains("A.ts"));
    // Disk is untouched.
    let on_disk = fs::read_to_string(dir.path().join("A.ts")).unwrap();
    assert!(on_disk.contains("export type A = null;"));
}

#[test]
fn check_mode_reports_an_orphaned_file_as_drift() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::new().output(dir.path());
    write_all(
        &files(&[
            ("A.ts", &generated("export type A = null;\n")),
            ("B.ts", &generated("export type B = null;\n")),
        ]),
        &cfg,
    )
    .unwrap();

    // The current generation set no longer includes B -- its Rust type is
    // gone, so an ordinary run would sweep B.ts. Check mode must report that
    // as drift too, not just content mismatches on files it still expects.
    let check = Config::new().output(dir.path()).check(true);
    let err = write_all(&files(&[("A.ts", &generated("export type A = null;\n"))]), &check)
        .expect_err("an orphaned generated file must count as drift");
    let msg = format!("{err}");
    assert!(msg.contains("B.ts"), "{msg}");
    assert!(msg.contains("no matching Rust type"), "{msg}");
    // Disk is untouched: check mode never deletes.
    assert!(dir.path().join("B.ts").is_file());
}

#[test]
fn check_mode_passes_when_output_matches() {
    let dir = tempfile::tempdir().unwrap();
    let f = files(&[("A.ts", &generated("export type A = null;\n"))]);
    write_all(&f, &Config::new().output(dir.path())).unwrap();
    assert!(write_all(&f, &Config::new().output(dir.path()).check(true)).is_ok());
}
