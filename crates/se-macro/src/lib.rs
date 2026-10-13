//! Proc macros behind the `se` facade. Nothing here is meant to be used
//! directly — depend on `se` and reach them as `se::Schema` / `#[se::stage]`.

use quote::quote;
use syn::{parse_macro_input, ItemFn};

mod asset;
mod stage;

const GONE: &str = "components are declared in contract.wit, not in Rust — \
     add the record there and `shinra build` generates the type";

fn gone() -> proc_macro2::TokenStream {
    quote! { compile_error!(#GONE); }
}

/// Removed: a component is declared in `contract.wit`. Expands to a
/// `compile_error!` that says so.
#[proc_macro_derive(Schema)]
pub fn derive_schema(_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    gone().into()
}

/// Removed: a component is declared in `contract.wit`. Expands to a
/// `compile_error!` that says so.
#[proc_macro]
pub fn components(_input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    gone().into()
}

/// Publish a file as this module's asset: its bytes in `se.asset`, its
/// lower-case extension in `se.asset.kind`.
#[proc_macro]
pub fn asset(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let path = parse_macro_input!(input as syn::LitStr);
    asset::asset(path).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Make a function a stage: its parameter list becomes its query.
#[proc_macro_attribute]
pub fn stage(
    attr: proc_macro::TokenStream,
    item: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(
            proc_macro2::Span::call_site(),
            "#[se::stage] takes no arguments — the signature is the whole spec",
        )
        .into_compile_error()
        .into();
    }
    let f = parse_macro_input!(item as ItemFn);
    stage::stage(f)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}


#[cfg(test)]
mod tests {
    #[test]
    fn stub_message() {
        let out = super::gone().to_string();
        assert!(out.contains("compile_error"));
        assert!(out.contains("contract.wit"));
    }
}
