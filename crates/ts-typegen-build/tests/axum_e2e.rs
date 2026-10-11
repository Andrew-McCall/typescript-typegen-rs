//! Drives the axum fixture crate the way a real user would: `cargo build`,
//! then inspect the generated client. Mirrors `tests/e2e.rs`, generalized to
//! take a fixture directory since this crate now has two fixtures.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Mutex;

use ts_typegen_build::axum::config::Axum;
use ts_typegen_build::run::generate;
use ts_typegen_build::Config;

static FIXTURE: Mutex<()> = Mutex::new(());

fn fixture_dir(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures").join(name)
}

fn build_fixture(dir: &Path, extra_env: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO"));
    cmd.arg("build").current_dir(dir);
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    cmd.output().expect("cargo build must run")
}

/// Cargo only reruns the build script when a scan root changes.
fn touch_source(dir: &Path) {
    let p = dir.join("src/handlers/patient/mod.rs");
    let text = std::fs::read_to_string(&p).unwrap();
    std::fs::write(&p, text).unwrap();
}

#[test]
fn the_macro_fixture_generates_a_client_from_its_own_attribute() {
    let _guard = FIXTURE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture_dir("macro-demo");
    let bindings = dir.join("bindings");
    let _ = std::fs::remove_dir_all(&bindings);

    let out = build_fixture(&dir, &[]);
    assert!(out.status.success(), "build failed: {}", String::from_utf8_lossy(&out.stderr));

    let client = std::fs::read_to_string(bindings.join("client/patient.ts")).unwrap();
    assert!(client.contains("getPatient: ({ id }: { id: number }, opts?: O) =>"));
    assert!(client.contains("path: `/v2/patient/${id}`"), "prefix must be applied: {client}");

    let auth = std::fs::read_to_string(bindings.join("client/auth.ts")).unwrap();
    assert!(auth.contains(r#"route: "/auth/refresh""#), "exempt path must not be prefixed");

    assert!(bindings.join("sender.ts").is_file());
    assert!(bindings.join("routes.ts").is_file());
    assert!(bindings.join("client.ts").is_file());
}

#[test]
fn no_routes_discovered_warns_and_emits_no_client_files() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    std::fs::write(src.join("lib.rs"), "pub struct Nothing;\n").unwrap();

    let cfg = Config::new()
        .scan(&src)
        .output(dir.path().join("bindings"))
        .axum(Axum::new());
    let outcome = generate(&cfg).expect("an empty route set is not a build failure");

    assert!(outcome.warnings.iter().any(|w| w.contains("no routes")), "got: {:?}", outcome.warnings);
    assert!(!dir.path().join("bindings/client.ts").exists());
    assert!(!dir.path().join("bindings/routes.ts").exists());
    assert!(!dir.path().join("bindings/sender.ts").exists());
}

#[test]
fn check_mode_detects_drift_in_a_generated_client_file() {
    let _guard = FIXTURE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture_dir("macro-demo");
    assert!(build_fixture(&dir, &[]).status.success());
    let path = dir.join("bindings/client/patient.ts");
    let original = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, original.replace("id: number", "id: string")).unwrap();

    let out = build_fixture(&dir, &[("TS_TYPEGEN_CHECK", "1")]);
    assert!(!out.status.success(), "check mode must fail on client drift");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("client/patient.ts"), "the drifted file must be named: {stderr}");

    std::fs::write(&path, original).unwrap();
    touch_source(&dir);
    assert!(build_fixture(&dir, &[]).status.success());
}

#[test]
fn the_generated_client_type_checks_under_strict_mode() {
    let _guard = FIXTURE.lock().unwrap_or_else(|e| e.into_inner());
    let dir = fixture_dir("macro-demo");
    if Command::new("npx").arg("--version").output().is_err() {
        eprintln!("skipping: npx not available");
        return;
    }
    assert!(build_fixture(&dir, &[]).status.success());
    let out = Command::new("npx")
        .args(["--yes", "--package", "typescript@5", "--", "tsc", "--noEmit", "--strict", "typecheck.ts"])
        .current_dir(&dir)
        .output()
        .expect("npx must run");
    assert!(out.status.success(), "tsc rejected output:\n{}", String::from_utf8_lossy(&out.stdout));
}
