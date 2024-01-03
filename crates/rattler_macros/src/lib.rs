//! Some macros for the Rattler project.

#![deny(missing_docs)]

use proc_macro::TokenStream;
use quote::quote_spanned;
use syn::{parse_macro_input, Data, DeriveInput, Fields, FieldsNamed, Ident};

/// Macro for enforcing alphabetical order on Structs and Enums.
///
/// This macro will not automatically sort it for you; rather, it will fail to compile if the
/// fields are not defined alphabetically.
#[proc_macro_attribute]
pub fn sorted(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let out = item.clone();
    let input = parse_macro_input!(item as DeriveInput);

    let name = &input.ident;

    let check_sorted = match &input.data {
        Data::Struct(data_struct) => match &data_struct.fields {
            Fields::Named(fields) => check_fields_sorted(name, fields),
            _ => panic!("This macro only supports structs with named fields."),
        },
        Data::Enum(data_enum) => {
            let variants: Vec<&Ident> = data_enum.variants.iter().map(|v| &v.ident).collect();
            check_identifiers_sorted(name, &variants)
        }
        Data::Union(_) => panic!("This macro only supports structs and enums."),
    };

    if check_sorted.is_err() {
        check_sorted.err().unwrap()
    } else {
        out
    }
}

fn check_fields_sorted(outer_ident: &Ident, fields: &FieldsNamed) -> Result<(), TokenStream> {
    let mut prev_field: Option<&Ident> = None;
    for field in &fields.named {
        let current_field = field.ident.as_ref().unwrap();
        if let Some(prev) = prev_field {
            if *current_field < *prev {
                let error = format!(
                    "The field {current_field} must be sorted before {prev} in struct {outer_ident}.",
                );
                let tokens = quote_spanned! {current_field.span() =>
                    compile_error!(#error);
                };
                return Err(TokenStream::from(tokens));
            }
        }
        prev_field = Some(current_field);
    }
    Ok(())
}

fn check_identifiers_sorted(outer_ident: &Ident, idents: &[&Ident]) -> Result<(), TokenStream> {
    let mut prev_ident: Option<&Ident> = None;
    for ident in idents {
        if let Some(prev) = prev_ident {
            if *ident < prev {
                let error = format!(
                    "The field {ident} must be sorted before {prev} in enum {outer_ident}.",
                );
                let tokens = quote_spanned! {ident.span() =>
                    compile_error!(#error);
                };
                return Err(TokenStream::from(tokens));
            }
        }
        prev_ident = Some(ident);
    }
    Ok(())
}
