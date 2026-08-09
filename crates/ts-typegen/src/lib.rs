//! Marker derive for types that should have TypeScript bindings generated.
//!
//! Generation itself happens in a build script via the `ts-typegen-build` crate.
//! This derive expands to nothing; it exists so that `#[ts(...)]` is legal syntax
//! and so the build-time scanner has a marker to look for.
#![forbid(unsafe_code)]

pub use ts_typegen_macros::Ts;
