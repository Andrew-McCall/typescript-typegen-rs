//! Asserts the shapes the emitter produces are the shapes serde actually emits.
//!
//! Enum tagging is where a generator like this quietly lies. Every case below
//! serialises a real value with serde_json and checks the JSON against the
//! structure the corresponding TypeScript describes.

use serde::Serialize;
use serde_json::json;

#[derive(Serialize)]
enum External {
    A,
    B { x: i32 },
    C(String),
}

#[derive(Serialize)]
#[serde(tag = "t")]
enum Internal {
    A,
    B { x: bool },
}

#[derive(Serialize)]
#[serde(tag = "t", content = "c")]
enum Adjacent {
    A,
    B { x: i32 },
    C(String),
}

#[derive(Serialize)]
#[serde(untagged)]
enum Untagged {
    A,
    B { x: i32 },
    C(String),
}

#[derive(Serialize)]
enum UnitOnly {
    Active,
    Banned,
}

#[test]
fn externally_tagged_wire_shape() {
    // TS: "A" | { B: { x: number } } | { C: string }
    assert_eq!(serde_json::to_value(External::A).unwrap(), json!("A"));
    assert_eq!(serde_json::to_value(External::B { x: 1 }).unwrap(), json!({ "B": { "x": 1 } }));
    assert_eq!(serde_json::to_value(External::C("s".into())).unwrap(), json!({ "C": "s" }));
}

#[test]
fn internally_tagged_wire_shape() {
    // TS: { t: "A" } | { t: "B"; x: boolean }
    assert_eq!(serde_json::to_value(Internal::A).unwrap(), json!({ "t": "A" }));
    assert_eq!(
        serde_json::to_value(Internal::B { x: true }).unwrap(),
        json!({ "t": "B", "x": true })
    );
}

#[test]
fn adjacently_tagged_wire_shape() {
    // TS: { t: "A" } | { t: "B"; c: { x: number } } | { t: "C"; c: string }
    assert_eq!(serde_json::to_value(Adjacent::A).unwrap(), json!({ "t": "A" }));
    assert_eq!(
        serde_json::to_value(Adjacent::B { x: 2 }).unwrap(),
        json!({ "t": "B", "c": { "x": 2 } })
    );
    assert_eq!(
        serde_json::to_value(Adjacent::C("s".into())).unwrap(),
        json!({ "t": "C", "c": "s" })
    );
}

#[test]
fn untagged_wire_shape() {
    // TS: null | { x: number } | string
    assert_eq!(serde_json::to_value(Untagged::A).unwrap(), json!(null));
    assert_eq!(serde_json::to_value(Untagged::B { x: 3 }).unwrap(), json!({ "x": 3 }));
    assert_eq!(serde_json::to_value(Untagged::C("s".into())).unwrap(), json!("s"));
}

#[test]
fn all_unit_enum_wire_shape() {
    // TS: "Active" | "Banned"
    assert_eq!(serde_json::to_value(UnitOnly::Active).unwrap(), json!("Active"));
    assert_eq!(serde_json::to_value(UnitOnly::Banned).unwrap(), json!("Banned"));
}

#[derive(Serialize)]
struct Optionality {
    plain: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skipped: Option<i32>,
}

#[test]
fn option_wire_shape_justifies_the_default_optional_style() {
    // Plain Option keeps its key as null -> `plain: number | null`.
    // skip_serializing_if drops the key entirely  -> `skipped?: number`.
    let v = serde_json::to_value(Optionality { plain: None, skipped: None }).unwrap();
    assert_eq!(v, json!({ "plain": null }));
    assert!(v.get("plain").is_some());
    assert!(v.get("skipped").is_none());
}

#[derive(Serialize)]
struct WireResult {
    v: Result<u8, String>,
}

#[test]
fn result_wire_shape_is_not_transparent() {
    // TS: { Ok: number } | { Err: string } -- justifies lower.rs not unwrapping Result.
    assert_eq!(serde_json::to_value(WireResult { v: Ok(1) }).unwrap(), json!({ "v": { "Ok": 1 } }));
    assert_eq!(
        serde_json::to_value(WireResult { v: Err("e".into()) }).unwrap(),
        json!({ "v": { "Err": "e" } })
    );
}

#[derive(Serialize)]
struct WireNonZero {
    v: std::num::NonZeroU32,
}

#[derive(Serialize)]
struct WireMapKey {
    by_id: std::collections::HashMap<i32, String>,
}

#[test]
fn map_key_wire_shape_is_always_a_string() {
    // TS: Record<string, string> -- justifies lower.rs forcing the key to
    // string regardless of the Rust key type; serde_json stringifies it.
    let mut by_id = std::collections::HashMap::new();
    by_id.insert(7, "seven".to_string());
    let v = serde_json::to_value(WireMapKey { by_id }).unwrap();
    let obj = v.get("by_id").unwrap().as_object().unwrap();
    assert!(obj.contains_key("7"), "expected the string key \"7\", got: {v}");
}

#[test]
fn nonzero_wire_shape_is_a_plain_number() {
    // TS: v: number -- justifies lower.rs mapping NonZero* to the same width as
    // its plain counterpart rather than to some branded/object shape.
    let v = WireNonZero { v: std::num::NonZeroU32::new(7).unwrap() };
    assert_eq!(serde_json::to_value(v).unwrap(), json!({ "v": 7 }));
}
