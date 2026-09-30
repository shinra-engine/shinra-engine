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

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::{spanned::Spanned, Error, FnArg, Ident, ItemFn, Pat, Result, Type};

enum Binding {
    Read(Ident),
    Write(Ident),
}

impl Binding {
    fn ty(&self) -> &Ident {
        match self {
            Binding::Read(t) | Binding::Write(t) => t,
        }
    }
}

/// `OnGround` -> `on-ground`: the WIT record name, which is what the contract
/// table and so the `se.stages` entry must carry. Names must be plain
/// lower-kebab words; an acronym or digit (`Http2Req`) does not round-trip.
fn kebab_name(ident: &Ident) -> String {
    let s = ident.to_string();
    let mut out = String::with_capacity(s.len() + 2);
    for (i, c) in s.chars().enumerate() {
        if c.is_uppercase() && i != 0 {
            out.push('-');
        }
        out.extend(c.to_lowercase());
    }
    out
}

/// `OnGround` -> `COL_ON_GROUND`, the constant the contract emitter writes.
fn col_ident(ident: &Ident) -> Ident {
    let name = kebab_name(ident).replace('-', "_").to_uppercase();
    Ident::new(&format!("COL_{name}"), Span::call_site())
}

fn component_of(ty: &Type) -> Result<Ident> {
    match ty {
        Type::Path(p) => p
            .path
            .get_ident()
            .cloned()
            .ok_or_else(|| Error::new(ty.span(), "a stage parameter must be a component type")),
        _ => Err(Error::new(ty.span(), "a stage parameter must be a component type")),
    }
}

fn classify(arg: &FnArg) -> Result<Binding> {
    let FnArg::Typed(t) = arg else {
        return Err(Error::new(arg.span(), "a stage is a free function; it has no `self`"));
    };
    if let Pat::Wild(_) = *t.pat {
        return Err(Error::new(t.pat.span(), "a stage parameter needs a name"));
    }
    match &*t.ty {
        Type::Reference(r) => {
            let c = component_of(&r.elem)?;
            Ok(if r.mutability.is_some() { Binding::Write(c) } else { Binding::Read(c) })
        }
        other => Err(Error::new(
            other.span(),
            "a stage parameter must be `&Component` or `&mut Component`. Time and input \
             are control vars now: read them from the arena rather than taking them here.",
        )),
    }
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
    let params: Vec<(String, u32)> = bindings
        .iter()
        .map(|b| {
            let access = match b {
                Binding::Read(_) => se_contract::stages::READ,
                Binding::Write(_) => se_contract::stages::WRITE,
            };
            (kebab_name(b.ty()), access)
        })
        .collect();
    let refs: Vec<(&str, u32)> = params.iter().map(|(n, a)| (n.as_str(), *a)).collect();
    let entry = se_contract::stages::encode(&name_str, &refs);
    let n_bytes = entry.len();
    let bytes = entry.iter().map(|b| quote!(#b));

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

    let section = se_contract::stages::SECTION;
    let decl = format_ident!("SE_STAGE_{}", name_str.to_uppercase());
    let run = format_ident!("{}{}", se_contract::stages::RUN_PREFIX, name_str);

    Ok(quote! {
        #f

        /// The declaration the host reads out of the file. `#[used]` because
        /// nothing references it: `--gc-sections` would otherwise drop the one
        /// thing that says this stage exists.
        #[used]
        #[link_section = #section]
        static #decl: [u8; #n_bytes] = [#(#bytes),*];

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

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Ident {
        Ident::new(s, Span::call_site())
    }

    #[test]
    fn single_word() {
        assert_eq!(kebab_name(&id("Transform")), "transform");
        assert_eq!(col_ident(&id("Transform")).to_string(), "COL_TRANSFORM");
    }

    #[test]
    fn multi_word_is_kebab_for_wit_and_snake_upper_for_col() {
        assert_eq!(kebab_name(&id("OnGround")), "on-ground");
        assert_eq!(col_ident(&id("OnGround")).to_string(), "COL_ON_GROUND");
    }
}
