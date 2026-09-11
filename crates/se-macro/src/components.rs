//! `se::components! { ... }` — declaring a component *is* registering it.
//!
//! The hand-written spelling is three attributes, a struct, and then the name
//! repeated in `se::layouts!(...)` at the foot of the file. Two acts that must
//! agree, with nothing checking that they do: omit a name and the component
//! simply does not exist at runtime, silently. Here there is one act.
//!
//! It is also the earliest point at which a field name can be judged, so the
//! two rules that used to fire late — a type the host cannot describe, and a
//! name that cannot cross into WGSL — fire at the field that broke them.

use proc_macro2::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, spanned::Spanned, Attribute, Error, Ident, Result, Token, Type, Visibility};

/// WGSL keywords and reserved words that a component field may not be named.
///
/// Copied from `se-render/src/attrs.rs`, which is where the same list rejects
/// a name while building a shader. It is duplicated rather than shared
/// because `se-macro` is what a module compiles against and `se-render` links
/// wgpu: making the macro depend on the renderer would drag the whole GPU
/// stack into every bundle build. Keep the two in step.
const RESERVED: &[&str] = &[
    "alias", "break", "case", "const", "const_assert", "continue", "continuing",
    "default", "diagnostic", "discard", "else", "enable", "false", "fn", "for",
    "if", "let", "loop", "override", "requires", "return", "struct", "switch",
    "true", "var", "while",
    // Reserved for future use by the WGSL spec.
    "as", "asm", "do", "enum", "extends", "filter", "goto", "handle", "impl",
    "in", "inline", "macro", "match", "mut", "of", "out", "package", "priv",
    "pub", "ref", "regardless", "static", "super", "target", "template",
    "this", "trait", "try", "type", "typedef", "union", "unless", "using",
    "virtual", "where", "yield",
    // Predeclared types and constructors that shadowing would break.
    "array", "atomic", "bool", "f16", "f32", "i32", "mat2x2", "mat3x3",
    "mat4x4", "ptr", "sampler", "texture_2d", "u32", "vec2", "vec3", "vec4",
];

struct Field {
    attrs: Vec<Attribute>,
    name: Ident,
    ty: Type,
}

struct Component {
    attrs: Vec<Attribute>,
    name: Ident,
    fields: Punctuated<Field, Token![,]>,
}

pub struct Components(Vec<Component>);

impl Parse for Field {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        // Every field is `pub` in the emitted struct — the host reads the
        // whole layout — so writing it is allowed and simply redundant.
        let _: Visibility = input.parse()?;
        let name: Ident = input.parse()?;
        input.parse::<Token![:]>()?;
        let ty: Type = input.parse()?;
        Ok(Field { attrs, name, ty })
    }
}

impl Parse for Component {
    fn parse(input: ParseStream) -> Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let _: Visibility = input.parse()?;
        let name: Ident = input.parse()?;
        let body;
        braced!(body in input);
        let fields = body.parse_terminated(Field::parse, Token![,])?;
        Ok(Component { attrs, name, fields })
    }
}

impl Parse for Components {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut items = Vec::new();
        while !input.is_empty() {
            items.push(input.parse()?);
            // A brace already separates one component from the next; a comma
            // between them is a habit, not an error.
            let _ = input.parse::<Token![,]>();
        }
        Ok(Components(items))
    }
}

/// A name that survives the trip into a shader.
fn check_name(f: &Ident) -> Result<()> {
    let s = f.to_string();
    if RESERVED.contains(&s.as_str()) {
        return Err(Error::new(
            f.span(),
            format!(
                "field `{s}` cannot cross into a shader: `{s}` is a WGSL reserved word. \
                 Rename it."
            ),
        ));
    }
    if s.starts_with("se_") || s.starts_with("__") {
        return Err(Error::new(
            f.span(),
            format!("field `{s}` uses a reserved prefix; `se_` and `__` belong to the engine"),
        ));
    }
    Ok(())
}

/// A type the host can describe. `crate::schema` already knows which those
/// are; all this adds is whose fault it is.
fn check_ty(f: &Field) -> Result<()> {
    crate::schema::scalar_of(&f.ty).map(|_| ()).map_err(|e| {
        let name = &f.name;
        Error::new(f.ty.span(), format!("field `{name}`: {e}"))
    })
}

pub fn components(items: Components) -> Result<TokenStream> {
    let mut error: Option<Error> = None;
    let mut push = |e: Error| match &mut error {
        Some(acc) => acc.combine(e),
        none => *none = Some(e),
    };

    let mut structs = Vec::new();
    let mut names = Vec::new();
    for c in &items.0 {
        for f in &c.fields {
            if let Err(e) = check_name(&f.name) {
                push(e);
            }
            if let Err(e) = check_ty(f) {
                push(e);
            }
        }

        let (attrs, name) = (&c.attrs, &c.name);
        let fields = c.fields.iter().map(|f| {
            let (fattrs, fname, fty) = (&f.attrs, &f.name, &f.ty);
            quote!(#(#fattrs)* pub #fname: #fty)
        });
        structs.push(quote! {
            #(#attrs)*
            #[repr(C)]
            #[derive(Clone, Copy, Debug, ::se::Schema)]
            pub struct #name { #(#fields),* }
        });
        names.push(name);
    }

    if let Some(e) = error {
        return Err(e);
    }

    Ok(quote! {
        #(#structs)*
        ::se::layouts!(#(#names),*);
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every diagnostic this macro raises, as one string. `Error` holds a
    /// list — printing only the first would hide the second bad field.
    fn errors(src: &str) -> String {
        let items: Components = syn::parse_str(src).expect("parses");
        match components(items) {
            Ok(_) => String::new(),
            Err(e) => e.into_iter().map(|e| e.to_string()).collect::<Vec<_>>().join(" | "),
        }
    }

    #[test]
    fn accepts_scalars_and_arrays() {
        assert_eq!(errors("Transform { pos: [f32; 3], hp: u32, mask: [u8; 4] }"), "");
    }

    #[test]
    fn several_components_in_one_invocation() {
        assert_eq!(errors("A { x: f32 } B { y: [f32; 2] }"), "");
    }

    /// The three names the ticket calls out by hand, plus the shape of the
    /// message: it must say which field, or the author has to go hunting.
    #[test]
    fn reserved_word_names_the_field() {
        for bad in ["target", "filter", "sampler"] {
            let msg = errors(&format!("C {{ {bad}: [f32; 2] }}"));
            assert!(msg.contains(&format!("field `{bad}`")), "{bad}: {msg}");
            assert!(msg.contains("WGSL reserved word"), "{bad}: {msg}");
        }
    }

    #[test]
    fn reserved_prefix_names_the_field() {
        let msg = errors("C { se_inner: f32 }");
        assert!(msg.contains("field `se_inner`"), "{msg}");
        assert!(msg.contains("reserved prefix"), "{msg}");
    }

    #[test]
    fn unsupported_type_names_the_field() {
        let msg = errors("C { text: String }");
        assert!(msg.contains("field `text`"), "{msg}");
        assert!(msg.contains("no ABI-stable layout"), "{msg}");
    }

    /// One bad field must not mask the next.
    #[test]
    fn every_bad_field_is_reported() {
        let msg = errors("C { target: f32, text: String }");
        assert!(msg.contains("field `target`"), "{msg}");
        assert!(msg.contains("field `text`"), "{msg}");
    }
}
