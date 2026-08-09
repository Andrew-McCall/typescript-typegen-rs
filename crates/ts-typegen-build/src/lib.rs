//! Build-time TypeScript binding generation. Use from `build.rs`.
#![forbid(unsafe_code)]

pub mod ir;
pub mod lower;
pub mod attrs;
pub mod diag;
pub mod parse;
pub mod scan;
