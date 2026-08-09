//! Build-time TypeScript binding generation. Use from `build.rs`.
#![forbid(unsafe_code)]

pub mod attrs;
pub mod config;
pub mod diag;
pub mod emit;
pub mod ir;
pub mod lower;
pub mod parse;
pub mod scan;
pub mod render;
pub mod write;
pub mod run;

pub use config::{BigInt, Config, OptionalStyle, StructStyle};
