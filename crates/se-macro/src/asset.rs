//! `se::asset!("rel/path.ext")` — the whole body of an asset module.

use proc_macro2::{Span, TokenStream};
use quote::quote;
use syn::LitStr;

pub fn asset(path: LitStr) -> syn::Result<TokenStream> {
    let value = path.value();
    let kind = value
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty() && !e.contains('/'))
        .ok_or_else(|| syn::Error::new(path.span(), "an asset needs a file extension: it is its kind"))?;
    // call_site, so `include_bytes!` resolves against the calling file.
    let lit = LitStr::new(&value, Span::call_site());
    let n = kind.len();
    let bytes = syn::LitByteStr::new(kind.as_bytes(), Span::call_site());
    Ok(quote! {
        #[used]
        #[link_section = "se.asset"]
        pub static SE_ASSET: [u8; include_bytes!(#lit).len()] = *include_bytes!(#lit);
        #[used]
        #[link_section = "se.asset.kind"]
        pub static SE_ASSET_KIND: [u8; #n] = *#bytes;
    })
}
