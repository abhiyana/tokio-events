use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, LitStr};

#[proc_macro_derive(Event, attributes(event))]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Default event_type is the struct name
    let mut event_type_val = name.to_string();

    // Look for #[event(event_type = "...")] attribute
    for attr in &input.attrs {
        if attr.path().is_ident("event") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("event_type") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    event_type_val = lit.value();
                    Ok(())
                } else {
                    Err(meta.error("unrecognized event attribute"))
                }
            });
        }
    }

    let expanded = quote! {
        impl tokio_events::Event for #name {
            fn event_type() -> &'static str {
                #event_type_val
            }
        }
    };

    TokenStream::from(expanded)
}
