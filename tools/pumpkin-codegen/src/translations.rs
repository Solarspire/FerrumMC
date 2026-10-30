use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use std::{collections::BTreeMap, fs};

pub fn build() -> TokenStream {
    let java_json: BTreeMap<String, String> = serde_json::from_str(
        &fs::read_to_string("../../assets/en_us_java.json").expect("en_us_java is missing"),
    )
    .unwrap();

    let mut java_constants = TokenStream::new();
    for (name, value) in &java_json {
        let ident = to_valid_ident(name);
        let doc = if !value.is_empty() {
            quote!(#[doc = #value])
        } else {
            quote!()
        };
        java_constants.extend(quote! {
            #doc
            pub const #ident: &str = #name;
        });
    }

    // --- Final Assembly ---
    quote! {
        #![allow(clippy::doc_markdown)]
        pub mod java {
            #java_constants
        }
    }
}

fn to_valid_ident(name: &str) -> Ident {
    let mut clean = name.to_uppercase().replace(['.', ':', '-'], "_");

    if clean.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        clean.insert(0, '_');
    }

    format_ident!("{}", clean)
}
