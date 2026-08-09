use ts_typegen::Ts;

#[derive(Ts)]
#[ts(rename = "Account")]
struct User {
    #[ts(type = "string")]
    id: u64,
    name: String,
}

#[derive(Ts)]
#[ts(tag = "kind")]
enum Event {
    #[ts(rename = "clicked")]
    Click { x: i32 },
    Ping,
}

#[test]
fn derive_expands_to_nothing_and_leaves_the_type_usable() {
    let u = User { id: 1, name: "a".into() };
    assert_eq!(u.id, 1);
    assert_eq!(u.name, "a");
    // Proves the derive did not add fields, impls, or consume the attributes.
    let Event::Click { x } = (Event::Click { x: 3 }) else {
        panic!("expected Click");
    };
    assert_eq!(x, 3);
    assert!(matches!(Event::Ping, Event::Ping));
}
