//! Proc macros behind the `se` facade. Nothing here is meant to be used
//! directly — depend on `se` and reach them as `se::Schema` / `#[se::stage]`.

use quote::{format_ident, quote};
use syn::punctuated::Punctuated;
use syn::{parse_macro_input, DeriveInput, Ident, ItemFn, Token};

mod components;
mod schema;
mod stage;

/// Describe a `#[repr(C)]` struct to the host so it can be stored, saved and
/// fed to a render pass without the host ever knowing the Rust type.
#[proc_macro_derive(Schema)]
pub fn derive_schema(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    schema::derive(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// `bundle/data.rs` — declare components and register them in one act.
///
/// ```ignore
/// se::components! {
///     Transform { pos: [f32; 3], rot: [f32; 4], scale: [f32; 3] }
///     Body { vel: [f32; 3] }
/// }
/// ```
///
/// Each becomes a `#[repr(C)]`, `#[derive(Clone, Copy, Debug, se::Schema)]`
/// `pub struct`, and the whole block emits one `se_register_layouts` covering
/// all of them — there is no second list to keep in step. A field the host
/// cannot describe, or whose name is a WGSL reserved word, is refused here
/// rather than at first draw.
#[proc_macro]
pub fn components(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let items = parse_macro_input!(input as components::Components);
    components::components(items)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
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

