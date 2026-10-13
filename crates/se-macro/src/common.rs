//! What `#[se::stage]` and `#[se::pair]` share: the names a signature gives a
//! component, and the reading of one `&T` / `&mut T`.

use proc_macro2::Span;
use quote::quote;
use syn::{spanned::Spanned, Error, Ident, Result, Type};

#[derive(Clone)]
pub enum Binding {
    Read(Ident),
    Write(Ident),
}

impl Binding {
    pub fn ty(&self) -> &Ident {
        match self {
            Binding::Read(t) | Binding::Write(t) => t,
        }
    }
}

/// `OnGround` -> `on-ground`: the WIT record name, which is what the contract
/// table and so the `se.stages` entry must carry. Names must be plain
/// lower-kebab words; an acronym or digit (`Http2Req`) does not round-trip.
pub fn kebab_name(ident: &Ident) -> String {
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
pub fn col_ident(ident: &Ident) -> Ident {
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

/// `&T` or `&mut T` naming a component.
pub fn element(ty: &Type, what: &str) -> Result<Binding> {
    match ty {
        Type::Reference(r) => {
            let c = component_of(&r.elem)?;
            Ok(if r.mutability.is_some() { Binding::Write(c) } else { Binding::Read(c) })
        }
        other => Err(Error::new(other.span(), what)),
    }
}

/// The entry's `(component, access, group)` for these bindings.
pub fn params(bs: &[(Binding, u32)]) -> Vec<(String, u32, u32)> {
    bs.iter()
        .map(|(b, g)| {
            let access = match b {
                Binding::Read(_) => se_contract::stages::READ,
                Binding::Write(_) => se_contract::stages::WRITE,
            };
            (kebab_name(b.ty()), access, *g)
        })
        .collect()
}

/// The `#[used]` section static and its bytes' length, for an encoded entry.
pub fn declaration(name: &str, entry: &[u8]) -> proc_macro2::TokenStream {
    let n_bytes = entry.len();
    let bytes = entry.iter().map(|b| quote!(#b));
    let section = se_contract::stages::SECTION;
    let decl = quote::format_ident!("SE_STAGE_{}", name.to_uppercase());
    quote! {
        /// The declaration the host reads out of the file. `#[used]` because
        /// nothing references it: `--gc-sections` would otherwise drop the one
        /// thing that says this stage exists.
        #[used]
        #[link_section = #section]
        static #decl: [u8; #n_bytes] = [#(#bytes),*];
    }
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

