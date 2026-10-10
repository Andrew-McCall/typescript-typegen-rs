//! Build-time TypeScript binding generation. Use from `build.rs`.
//!
//! ```no_run
//! // build.rs
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     ts_typegen_build::Config::new()
//!         .scan("src")
//!         .output("bindings")
//!         .run()
//! }
//! ```
//!
//! Mark the types you want bindings for with `#[derive(Serialize, Ts)]`
//! (`Ts` comes from the `ts-typegen` crate). This crate reads that derive and
//! any `#[ts(...)]` / `#[serde(...)]` attributes straight from source text
//! with `syn` — a build script runs before its own crate compiles, so there
//! is no compiled type information to inspect yet. See [`Config`] for every
//! generation option, and [`Preset`] for built-in mappings of common foreign
//! crates such as `uuid` and `chrono`.
#![forbid(unsafe_code)]

pub mod attrs;
#[cfg(feature = "axum")]
pub mod axum;
pub mod config;
pub mod diag;
pub mod emit;
pub mod ir;
pub mod lower;
pub mod parse;
pub mod scan;
pub mod render;
pub mod write;
pub mod presets;
pub mod run;

pub use config::{BigInt, Config, OptionalStyle, StructStyle};
pub use presets::Preset;
