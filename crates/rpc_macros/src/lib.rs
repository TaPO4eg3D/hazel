use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Error, Fields, ItemStruct, parse_macro_input};

#[proc_macro_attribute]
pub fn rpc_method(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as ItemStruct);

    let name = &input.ident;
    let name_str = name.to_string();

    let vis = &input.vis;

    let mut request_type = None;
    let mut response_type = None;
    let mut error_type = None;

    if let Fields::Named(ref fields) = input.fields {
        for field in &fields.named {
            let field_name = field.ident.as_ref().unwrap().to_string();
            match field_name.as_str() {
                "request" => request_type = Some(&field.ty),
                "response" => response_type = Some(&field.ty),
                "error" => error_type = Some(&field.ty),
                name => {
                    return Error::new_spanned(
                        input,
                        format!(
                            "RPCMethod should have only request|response|error fields. Unknown field: {}",
                            name,
                        )
                    ).to_compile_error().into();
                }
            }
        }
    } else {
        return Error::new_spanned(input, "Fields must be named").to_compile_error().into();
    }

    let Some(request_type) = request_type else {
        return Error::new_spanned(input, "Missing request field").to_compile_error().into();
    };

    let Some(response_type) = response_type else {
        return Error::new_spanned(input, "Missing response field").to_compile_error().into();
    };

    let Some(error_type) = error_type else {
        return Error::new_spanned(input, "Missing error field").to_compile_error().into();
    };

    let expanded = quote! {
        #vis struct #name {}

        impl crate::models::common::RPCMethod for #name {
            type Request = #request_type;
            type Response = crate::models::common::APIResult<#response_type, #error_type>;

            fn key() -> &'static str {
                #name_str
            }
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(RPCNotification)]
pub fn derive_rpc_notification(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);

    let name = &input.ident;
    let name_str = name.to_string();

    let expanded = quote! {
        impl crate::models::common::RPCNotification for #name {
            fn key() -> &'static str {
                #name_str
            }
        }
    };

    TokenStream::from(expanded)
}
