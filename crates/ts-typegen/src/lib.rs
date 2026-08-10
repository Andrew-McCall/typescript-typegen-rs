//! Marker derive for types that should have TypeScript bindings generated.
//!
//! Generation itself happens in a build script via the `ts-typegen-build` crate.
//! This derive expands to nothing; it exists so that `#[ts(...)]` is legal syntax
//! and so the build-time scanner has a marker to look for.
//!
//! Pair it with `#[derive(Serialize)]`, since bindings describe the JSON
//! serde actually writes:
//!
//! ```
//! use serde::Serialize;
//! use ts_typegen::Ts;
//!
//! #[derive(Serialize, Ts)]
//! #[serde(rename_all = "camelCase")]
//! struct User {
//!     user_id: u64,
//!     name: String,
//!     #[serde(skip)]
//!     secret: String,
//! }
//! ```
//!
//! `#[ts(...)]` is the escape hatch for anything serde's shape doesn't
//! capture on its own — see the crate README for the full attribute list.
#![forbid(unsafe_code)]

pub use ts_typegen_macros::Ts;

#[cfg(feature = "strict")]
mod marker;
#[cfg(feature = "strict")]
pub use marker::TsType;
