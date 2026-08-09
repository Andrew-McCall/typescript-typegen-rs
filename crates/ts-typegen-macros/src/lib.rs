//! The `Ts` derive. Expands to nothing.
//!
//! Its only job is registering `ts` as a helper attribute so rustc accepts
//! `#[ts(...)]` on types, fields, and variants without an `unknown attribute`
//! error. The build script reads those attributes from source text.
#![forbid(unsafe_code)]

use proc_macro::TokenStream;

#[proc_macro_derive(Ts, attributes(ts))]
pub fn derive_ts(_input: TokenStream) -> TokenStream {
    TokenStream::new()
}
