use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, LitStr};
use heck::ToKebabCase;

/// Generates proper SVG loading based on Enum variant name.
/// This is done by automatically implementing [IconNamed](https://longbridge.github.io/gpui-component/docs/components/icon#build-you-own-iconname) trait from `gpui-components`
///
/// Example:
///
/// ```
/// enum IconName {
///     MyIcon, // turns into: icons/my-icon.svg
///     #[icon(name = "my-awesome-icon")]
///     FooBar, // turns into: icons/my-awesome-icon.svg
/// }
/// ```
#[proc_macro_derive(IconPath, attributes(icon))]
pub fn derive_icon_path(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let variants = match input.data {
        Data::Enum(ref data) => &data.variants,
        _ => panic!("IconPath can only be derived for enums"),
    };

    let match_arms = variants.iter().map(|variant| {
        let variant_ident = &variant.ident;
        let mut filename = String::new();

        for attr in &variant.attrs {
            if attr.path().is_ident("icon") {
                let result = attr.parse_nested_meta(|meta| {
                    if meta.path.is_ident("name") {
                        let value = meta.value()?;
                        let s: LitStr = value.parse()?;

                        filename = s.value();

                        Ok(())
                    } else {
                        Err(meta.error("unrecognized term in icon attribute"))
                    }
                });

                if let Err(err) = result {
                    return err.to_compile_error();
                }
            }
        }

        // default to kebab-case if no name override
        if filename.is_empty() {
            filename = variant_ident.to_string().to_kebab_case();
        }

        let full_path = format!("icons/{}.svg", filename);

        quote! {
            #name::#variant_ident => #full_path,
        }
    });

    let expanded = quote! {
        impl IconNamed for #name {
            fn path(self) -> SharedString {
                match self {
                    #(#match_arms)*
                }.into()
            }
        }
    };

    TokenStream::from(expanded)
}
