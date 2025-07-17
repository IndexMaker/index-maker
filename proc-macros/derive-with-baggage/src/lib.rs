extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Fields};

/// Main entry point for the `#[derive(WithBaggage)]` procedural macro.
#[proc_macro_derive(WithBaggage, attributes(baggage))]
pub fn derive_with_baggage(input: TokenStream) -> TokenStream {
    // Parse the input tokens into a syntax tree.
    let input = parse_macro_input!(input as DeriveInput);

    // Extract the name of the type for which we are deriving the trait.
    let name = &input.ident;

    // Get the generics from the input type, so we can apply them to the impl block.
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the implementation of the `WithBaggage` trait.
    let expanded = match &input.data {
        Data::Enum(data_enum) => {
            
            // Iterate over each variant of the enum.
            let match_arms = data_enum.variants.iter().map(|variant| {
                let variant_name = &variant.ident;
                let mut field_injectors = Vec::new();
                let mut field_names = Vec::new();

                match &variant.fields {

                    // We only support named fields
                    Fields::Named(fields) => {
                        for field in &fields.named {
                            
                            // Get field name and its attributes
                            let field_name = field.ident.as_ref().unwrap();
                            let has_baggage_attr = field.attrs.iter().any(|attr| attr.path().is_ident("baggage"));

                            // If there is #[baggage] attribute on the field
                            if has_baggage_attr {
                                // then destructure that field
                                field_names.push(quote! { #field_name });

                                // and implement injector taking name of tha field and the value of that field
                                field_injectors.push(quote! {
                                    tracing_data.set(stringify!(#field_name), #field_name.to_string());
                                });

                            } else {
                                // Otherwise we still need to destructure that field, but we won't use it
                                field_names.push(quote! { #field_name: _ });
                            }
                        }

                        // Construct the pattern for named fields: `Variant { field1, field2: _, ... }`
                        let fields_pattern = quote! { { #(#field_names),* } };
                        quote! {
                            Self::#variant_name #fields_pattern => {
                                #(#field_injectors)*
                            }
                        }
                    }
                    _ => { quote! { _ => {} } }
                }
            });

            // The generated `impl` block for the enum.
            quote! {
                impl #impl_generics WithBaggage for #name #ty_generics #where_clause {
                    fn inject_baggage(&self, tracing_data: &mut TracingData) {
                        match self {
                            #(#match_arms),*
                        }
                    }
                }
            }
        }
        Data::Struct(_) => {
            panic!("WithBaggage derive macro only supports enums at the moment.");
        }
        Data::Union(_) => {
            panic!("WithBaggage derive macro does not support unions.");
        }
    };

    // Hand the generated code back to the compiler.
    expanded.into()
}
