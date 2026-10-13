//! `#[se::stage]` — a function whose parameter list *is* its query.
//!
//! The signature is the whole contract. There is no world handle, no command
//! buffer and no escape hatch: reaching outside the parameters is not
//! expressible, so single-writer reasoning holds by construction.
//!
//! What the attribute emits is two things, and both are data as far as the
//! host is concerned:
//!
//!   * an entry in the `se.stages` custom section, naming the components the
//!     signature named and whether each is `&` or `&mut`. The host reads it
//!     without instantiating the module and checks it against `contract.wit`.
//!   * `se_stage_run__<name>`, the batched loop. The per-row loop runs *here*,
//!     inside the module, over the host's own columns — so the boundary is
//!     crossed once per stage, not once per row, and nothing is copied.
//!
//! There is no `StageCall` any more. A stage takes no arguments: the column
//! descriptors are at an address the contract fixed, and how many rows exist
//! is the pool's length, which a control var asked for.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use crate::common::{col_ident, declaration, element, params, Binding};
use syn::{spanned::Spanned, Error, FnArg, ItemFn, Pat, Result};


fn classify(arg: &FnArg) -> Result<Binding> {
    let FnArg::Typed(t) = arg else {
        return Err(Error::new(arg.span(), "a stage is a free function; it has no `self`"));
    };
    if let Pat::Wild(_) = *t.pat {
        return Err(Error::new(t.pat.span(), "a stage parameter needs a name"));
    }
    element(
        &t.ty,
        "a stage parameter must be `&Component` or `&mut Component`. Time and input \
         are control vars now: read them from the arena rather than taking them here.",
    )
}

pub fn stage(f: ItemFn) -> Result<TokenStream> {
    let name = f.sig.ident.clone();
    let name_str = name.to_string();
    if f.sig.inputs.is_empty() {
        return Err(Error::new(
            f.sig.span(),
            "a stage's parameter list is its query; one that names nothing would run over nothing",
        ));
    }

    let bindings: Vec<Binding> = f.sig.inputs.iter().map(classify).collect::<Result<_>>()?;

    // The declaration, encoded by the same code the host decodes with.
    let grouped: Vec<(Binding, u32)> = bindings.iter().map(|b| (b.clone(), 0)).collect();
    let ps = params(&grouped);
    let refs: Vec<(&str, u32, u32)> = ps.iter().map(|(n, a, g)| (n.as_str(), *a, *g)).collect();
    let entry = se_contract::stages::encode(&name_str, se_contract::stages::ROW, &refs);
    let decl = declaration(&name_str, &entry);

    // Each column is one load through the descriptor the host published, and
    // the pool it belongs to is a field of that descriptor — so a stage never
    // needs to be told which pool it runs over.
    let mut lets = Vec::new();
    let mut args = Vec::new();
    for (i, b) in bindings.iter().enumerate() {
        let c = format_ident!("se_c{i}");
        let ty = b.ty();
        let col = col_ident(ty);
        lets.push(quote! { let #c = crate::se_col(crate::arena::#col); });
        args.push(match b {
            Binding::Read(_) => quote! { &*crate::se_row::<#ty>(&#c, se_i) },
            Binding::Write(_) => quote! { &mut *crate::se_row::<#ty>(&#c, se_i) },
        });
    }
    let first = format_ident!("se_c0");

    let run = format_ident!("{}{}", se_contract::stages::RUN_PREFIX, name_str);

    Ok(quote! {
        #f

        #decl

        /// The batched loop. One crossing per stage; the rows are the host's
        /// own bytes, read and written in place.
        #[no_mangle]
        pub extern "C" fn #run() -> u32 {
            unsafe {
                #(#lets)*
                let se_n = crate::se_pool(#first.pool).len;
                let mut se_i = 0u32;
                while se_i < se_n {
                    #name(#(#args),*);
                    se_i += 1;
                }
                se_n
            }
        }
    })
}

