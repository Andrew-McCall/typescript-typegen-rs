#![cfg(feature = "axum")]

use ts_typegen::ts_route;

#[ts_route]
fn bare() -> u8 {
    1
}

#[ts_route(method = "GET", path = "/a/{id}")]
fn with_keys(id: i64) -> i64 {
    id
}

#[ts_route(response(type = "Blob"), params(id(type = "number")), skip)]
fn with_nested(id: i64) -> i64 {
    id
}

#[test]
fn the_attribute_expands_to_the_item_unchanged() {
    assert_eq!(bare(), 1);
    assert_eq!(with_keys(7), 7);
    assert_eq!(with_nested(9), 9);
}
