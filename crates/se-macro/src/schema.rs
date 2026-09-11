//! `#[derive(se::Schema)]` — the only way a struct becomes a component.
//!
//! It reads what `#[repr(C)]` already decided (via `offset_of!`) rather than
//! recomputing a layout of its own, so the description the host receives is
//! the memory the module actually uses.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{spanned::Spanned, Data, DeriveInput, Error, Fields, Result, Type};

/// A field flattened to (scalar type, element count).
pub(crate) fn scalar_of(ty: &Type) -> Result<(TokenStream, u32)> {
    match ty {
        Type::Path(p) => {
            let ident = p
                .path
                .get_ident()
                .ok_or_else(|| Error::new(ty.span(), "component fields must be scalars or arrays of scalars"))?;
            let tag = match ident.to_string().as_str() {
                "u8" => quote!(::se::ScalarTy::U8),
                "i32" => quote!(::se::ScalarTy::I32),
                "u32" => quote!(::se::ScalarTy::U32),
                "f32" => quote!(::se::ScalarTy::F32),
                "f64" => quote!(::se::ScalarTy::F64),
                other => {
                    return Err(Error::new(
                        ty.span(),
                        format!("`{other}` has no ABI-stable layout; use u8/i32/u32/f32/f64"),
                    ))
                }
            };
            Ok((tag, 1))
        }
        Type::Array(a) => {
            let (tag, inner) = scalar_of(&a.elem)?;
            let len: usize = match &a.len {
                syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Int(i), .. }) => i.base10_parse()?,
                _ => return Err(Error::new(a.len.span(), "array length must be a literal")),
            };
            Ok((tag, inner * len as u32))
        }
        _ => Err(Error::new(
            ty.span(),
            "component fields must be scalars or arrays of scalars",
        )),
    }
}

fn is_repr_c(input: &DeriveInput) -> bool {
    input.attrs.iter().any(|a| {
        if !a.path().is_ident("repr") {
            return false;
        }
        let mut found = false;
        let _ = a.parse_nested_meta(|m| {
            if m.path.is_ident("C") {
                found = true;
            }
            Ok(())
        });
        found
    })
}

pub fn derive(input: DeriveInput) -> Result<TokenStream> {
    if !is_repr_c(&input) {
        return Err(Error::new(
            input.ident.span(),
            "a component must be `#[repr(C)]` — the host reads it by layout, not by type",
        ));
    }

    let name = &input.ident;
    let name_str = name.to_string();

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(n) => &n.named,
            _ => {
                return Err(Error::new(
                    input.ident.span(),
                    "a component must be a struct with named fields",
                ))
            }
        },
        _ => return Err(Error::new(input.ident.span(), "only structs can be components")),
    };

    let mut entries = Vec::new();
    let mut folds = Vec::new();
    for f in fields {
        let fname = f.ident.as_ref().unwrap();
        let fstr = fname.to_string();
        let (tag, count) = scalar_of(&f.ty)?;
        entries.push(quote! {
            ::se::Field::new(
                #fstr,
                #tag,
                ::core::mem::offset_of!(#name, #fname) as u32,
                #count,
            )
        });
        folds.push(quote! {
            h = ::se::hash_field(
                h,
                #fstr,
                #tag,
                ::core::mem::offset_of!(#name, #fname) as u32,
                #count,
            );
        });
    }

    let n = entries.len();

    Ok(quote! {
        impl #name {
            #[doc(hidden)]
            pub const __SE_FIELDS: [::se::Field; #n] = [#(#entries),*];
        }

        impl ::se::Schema for #name {
            const NAME: &'static str = #name_str;
            const HASH: u64 = {
                let mut h = ::se::hash_begin(
                    #name_str,
                    ::core::mem::size_of::<#name>() as u32,
                    ::core::mem::align_of::<#name>() as u32,
                );
                #(#folds)*
                h
            };

            fn layout() -> ::se::Layout {
                ::se::Layout::new(
                    #name_str,
                    ::core::mem::size_of::<#name>() as u32,
                    ::core::mem::align_of::<#name>() as u32,
                    &#name::__SE_FIELDS,
                    <#name as ::se::Schema>::HASH,
                )
            }
        }

        // Being a component is what lets the type appear in a stage signature.
        unsafe impl ::se::StageParam for #name {
            const PARAM: ::se::Param = ::se::Param::component(
                #name_str,
                ::se::Access::Read,
                ::core::mem::size_of::<#name>() as u32,
                <#name as ::se::Schema>::HASH,
            );
            unsafe fn fetch(call: *const ::se::StageCall, slot: u32, row: u32) -> *mut u8 {
                ::se::fetch_component(call, slot, row)
            }
        }
    })
}
