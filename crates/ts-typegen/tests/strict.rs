//! Verifies the strict feature accepts what it should. Rejection is covered by
//! the trybuild UI test below.

#![cfg(feature = "strict")]

use std::collections::HashMap;
use ts_typegen::Ts;

struct Foreign;

#[derive(Ts)]
#[allow(dead_code)]
struct AllMapped {
    a: u64,
    b: String,
    c: Vec<bool>,
    d: Option<String>,
    e: HashMap<String, u8>,
    f: (u8, String),
}

#[derive(Ts)]
#[allow(dead_code)]
struct EscapeHatch {
    // No TsType impl exists for Foreign, but the override suppresses the assertion.
    #[ts(type = "string")]
    v: Foreign,
}

#[derive(Ts)]
#[allow(dead_code)]
enum WithVariants {
    A,
    B { x: u32 },
    C(String),
}

#[test]
fn mapped_types_compile_under_strict() {
    let _ = AllMapped {
        a: 1,
        b: String::new(),
        c: vec![],
        d: None,
        e: HashMap::new(),
        f: (0, String::new()),
    };
    let _ = EscapeHatch { v: Foreign };
    let _ = WithVariants::B { x: 1 };
    let _ = WithVariants::C(String::new());
    let _ = WithVariants::A;
}

#[test]
fn trybuild_ui() {
    let t = trybuild::TestCases::new();
    t.compile_fail("tests/ui/unmapped_field.rs");
}
