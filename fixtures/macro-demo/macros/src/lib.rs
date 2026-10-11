use proc_macro::TokenStream;

/// A foreign route attribute, inert like `ts_route` -- ts-typegen-build reads
/// its arguments from source text, not from macro expansion.
#[proc_macro_attribute]
pub fn api(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
