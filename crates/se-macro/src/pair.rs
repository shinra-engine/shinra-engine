//! `#[se::pair]` — a function over two rows at once.
//!
//! `fn hit((a, sa): (&mut Actor, &Sprite), (b, sb): (&mut Actor, &Sprite))`.
//! Each parameter is a tuple of components of one pool: one row of it. When
//! both tuples live in the same pool the loop visits every unordered pair
//! `i < j` once; over two pools it visits every `i × j`. Brute force is the
//! specification; the host checks the entry the way it checks a row stage.

use crate::common::{col_ident, declaration, element, params, Binding};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{spanned::Spanned, Error, FnArg, ItemFn, Pat, Result, Type};

fn group(arg: &FnArg) -> Result<Vec<Binding>> {
    let FnArg::Typed(t) = arg else {
        return Err(Error::new(arg.span(), "a pair stage is a free function; it has no `self`"));
    };
    if !matches!(*t.pat, Pat::Tuple(_)) {
        return Err(Error::new(t.pat.span(), "a pair parameter is a tuple pattern: `(a, b): (&A, &B)`"));
    }
    let Type::Tuple(tt) = &*t.ty else {
        return Err(Error::new(t.ty.span(), "a pair parameter is a tuple of `&Component` / `&mut Component`"));
    };
    if tt.elems.is_empty() {
        return Err(Error::new(tt.span(), "a pair parameter names no component"));
    }
    tt.elems
        .iter()
        .map(|e| element(e, "a pair parameter element must be `&Component` or `&mut Component`"))
        .collect()
}

pub fn pair(f: ItemFn) -> Result<TokenStream> {
    let name = f.sig.ident.clone();
    let name_str = name.to_string();
    if f.sig.inputs.len() != 2 {
        return Err(Error::new(f.sig.span(), "a pair stage takes exactly two tuple parameters"));
    }
    let groups: Vec<Vec<Binding>> = f.sig.inputs.iter().map(group).collect::<Result<_>>()?;

    let flat: Vec<(Binding, u32)> = groups
        .iter()
        .enumerate()
        .flat_map(|(g, bs)| bs.iter().map(move |b| (b.clone(), g as u32)))
        .collect();
    let ps = params(&flat);
    let refs: Vec<(&str, u32, u32)> = ps.iter().map(|(n, a, g)| (n.as_str(), *a, *g)).collect();
    let entry = se_contract::stages::encode(&name_str, se_contract::stages::PAIR, &refs);
    let decl = declaration(&name_str, &entry);

    let mut lets = Vec::new();
    let mut tuples = Vec::new();
    for (g, bs) in groups.iter().enumerate() {
        let idx = if g == 0 { format_ident!("se_i") } else { format_ident!("se_j") };
        let mut elems = Vec::new();
        for (k, b) in bs.iter().enumerate() {
            let c = format_ident!("se_c{g}_{k}");
            let ty = b.ty();
            let col = col_ident(ty);
            lets.push(quote! { let #c = crate::se_col(crate::arena::#col); });
            elems.push(match b {
                Binding::Read(_) => quote! { &*crate::se_row::<#ty>(&#c, #idx) },
                Binding::Write(_) => quote! { &mut *crate::se_row::<#ty>(&#c, #idx) },
            });
        }
        tuples.push(quote! { (#(#elems),*,) });
    }
    let (a, b) = (format_ident!("se_c0_0"), format_ident!("se_c1_0"));
    let (ta, tb) = (&tuples[0], &tuples[1]);
    let run = format_ident!("{}{}", se_contract::stages::RUN_PREFIX, name_str);

    Ok(quote! {
        #f

        #decl

        /// The batched loop: every pair, inside the module. Returns how many
        /// pairs it visited.
        #[no_mangle]
        pub extern "C" fn #run() -> u32 {
            unsafe {
                #(#lets)*
                let se_na = crate::se_pool(#a.pool).len;
                let se_nb = crate::se_pool(#b.pool).len;
                let se_same = #a.pool == #b.pool;
                let mut se_pairs = 0u32;
                let mut se_i = 0u32;
                while se_i < se_na {
                    let mut se_j = if se_same { se_i + 1 } else { 0 };
                    while se_j < se_nb {
                        #name(#ta, #tb);
                        se_pairs += 1;
                        se_j += 1;
                    }
                    se_i += 1;
                }
                se_pairs
            }
        }
    })
}
