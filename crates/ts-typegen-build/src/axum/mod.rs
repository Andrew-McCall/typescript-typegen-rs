//! axum route discovery, signature inference, and TypeScript client emission.
//!
//! A handler reaches the generated client by carrying `#[ts_route(...)]` (or
//! a configured foreign attribute), by appearing in a scanned `Router`
//! builder chain, or both at once as a drift check between the two.
//! Configure with [`config::Axum`] and pass it to [`crate::Config::axum`];
//! see the crate README for the full attribute and diagnostics reference.
//!
//! ```no_run
//! fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     ts_typegen_build::Config::new()
//!         .scan("src")
//!         .output("bindings")
//!         .axum(
//!             ts_typegen_build::axum::config::Axum::new()
//!                 .prefix("v2")
//!                 .prefix_exempt(["/auth/*"])
//!                 .client(ts_typegen_build::axum::config::Client::Modules {
//!                     strip: Some("handlers".into()),
//!                 }),
//!         )
//!         .run()
//! }
//! ```

pub mod bind;
pub mod client;
pub mod config;
pub mod path;
pub mod router;
pub mod routes;
