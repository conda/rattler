use proc_macro::TokenStream;
use quote::quote_spanned;
use syn::{parse_macro_input, Data, DeriveInput, Field, Fields, FieldsNamed, Ident, LitStr};

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
        _ => panic!("This macro only supports structs and enums."),
    };

    if check_sorted.is_err() {
        check_sorted.err().unwrap()
    } else {
        out
    }
}

fn get_rename(field: &Field) -> Option<String> {
    let mut rename = None;
    for attr in &field.attrs {
        if attr.path().is_ident("serde") {
            let res = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename") {
                    println!("Found rename");
                    let value = meta.value()?;
                    let s: LitStr = value.parse()?;
                    println!("STRING: {}", s.value());
                    rename = Some(s.value());
                }
                Ok(())
            });
            println!("Result: {:?}", res);
        }
    }
    rename
}

fn check_fields_sorted(outer_ident: &Ident, fields: &FieldsNamed) -> Result<(), TokenStream> {
    let mut prev_field: Option<String> = None;
    // println!("Fields: {:?}", fields);
    for field in &fields.named {
        let current_field = field.ident.as_ref().unwrap();
        println!("Fields: {:?}", current_field);
        let current_field_name = get_rename(field).unwrap_or_else(|| current_field.to_string());

        if let Some(prev) = prev_field {
            println!("{} < {}", current_field_name, prev);
            if current_field_name < prev {
                let error = format!(
                    "The field {} must be sorted before {} in struct {}.",
                    current_field, prev, outer_ident
                );
                let tokens = quote_spanned! {current_field.span() =>
                    compile_error!(#error);
                };
                return Err(TokenStream::from(tokens));
            }
        }

        prev_field = Some(current_field_name);
    }
    Ok(())
}

fn check_identifiers_sorted(outer_ident: &Ident, idents: &[&Ident]) -> Result<(), TokenStream> {
    let mut prev_ident: Option<&Ident> = None;
    for ident in idents {
        if let Some(prev) = prev_ident {
            if *ident < prev {
                let error = format!(
                    "The field {} must be sorted before {} in enum {}.",
                    ident, prev, outer_ident
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
