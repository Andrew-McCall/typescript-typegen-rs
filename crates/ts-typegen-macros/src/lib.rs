//! The `Ts` derive.
//!
//! Without the `strict` feature it expands to nothing. Its baseline job is
//! registering `ts` as a helper attribute so rustc accepts `#[ts(...)]` on types,
//! fields, and variants; the build script reads those attributes from source text.
#![forbid(unsafe_code)]

use proc_macro::TokenStream;

#[proc_macro_derive(Ts, attributes(ts))]
pub fn derive_ts(input: TokenStream) -> TokenStream {
    #[cfg(feature = "strict")]
    {
        strict::expand(input)
    }
    #[cfg(not(feature = "strict"))]
    {
        let _ = input;
        TokenStream::new()
    }
}

#[cfg(feature = "strict")]
mod strict {
    use proc_macro::TokenStream;
    use quote::quote;
    use syn::{Data, DeriveInput, Fields, parse_macro_input};

    pub fn expand(input: TokenStream) -> TokenStream {
        let input = parse_macro_input!(input as DeriveInput);
        let mut tys = Vec::new();
        match &input.data {
            Data::Struct(s) => collect(&s.fields, &mut tys),
            Data::Enum(e) => e.variants.iter().for_each(|v| collect(&v.fields, &mut tys)),
            Data::Union(_) => {}
        }
        let asserts = tys.iter().map(|ty| {
            quote! {
                const _: fn() = || {
                    fn assert_ts_type<T: ::ts_typegen::TsType + ?Sized>() {}
                    assert_ts_type::<#ty>();
                };
            }
        });
        quote!(#(#asserts)*).into()
    }

    /// Skips fields carrying `#[ts(type = "...")]`, `#[ts(skip)]`, or
    /// `#[serde(skip)]` — those never reach the type mapper.
    fn collect(fields: &Fields, out: &mut Vec<syn::Type>) {
        for f in fields.iter() {
            if is_exempt(&f.attrs) {
                continue;
            }
            out.push(f.ty.clone());
        }
    }

    fn is_exempt(attrs: &[syn::Attribute]) -> bool {
        let mut exempt = false;
        for attr in attrs {
            if !(attr.path().is_ident("ts") || attr.path().is_ident("serde")) {
                continue;
            }
            let _ = attr.parse_nested_meta(|meta| {
                let key =
                    meta.path.segments.last().map(|s| s.ident.to_string()).unwrap_or_default();
                if matches!(key.as_str(), "type" | "as" | "skip" | "skip_serializing") {
                    exempt = true;
                }
                Ok(())
            });
            Ok::<(), ()>(()).ok();
        }
        exempt
    }
}
