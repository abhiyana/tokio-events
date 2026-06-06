use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, DeriveInput, LitStr};

#[proc_macro_derive(Event, attributes(event))]
pub fn derive_event(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    // Default event_type is the struct name
    let mut event_type_val = None;
    let mut format_val = "json".to_string();

    // Look for #[event(event_type = "...", format = "...")] attribute
    for attr in &input.attrs {
        if attr.path().is_ident("event") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("event_type") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    event_type_val = Some(lit.value());
                    Ok(())
                } else if meta.path.is_ident("format") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    format_val = lit.value();
                    Ok(())
                } else {
                    Err(meta.error("unrecognized event attribute"))
                }
            });
        }
    }

    let serialization_impl = if format_val == "protobuf" {
        quote! {
            fn serialize_event(&self) -> tokio_events::Result<Vec<u8>> {
                use prost::Message;
                let mut buf = Vec::new();
                self.encode(&mut buf).map_err(|e| tokio_events::Error::SerializationError(e.to_string()))?;
                Ok(buf)
            }

            fn deserialize_event(bytes: &[u8]) -> tokio_events::Result<Self> {
                use prost::Message;
                Self::decode(bytes).map_err(|e| tokio_events::Error::SerializationError(e.to_string()))
            }
        }
    } else {
        quote! {
            fn serialize_event(&self) -> tokio_events::Result<Vec<u8>> {
                serde_json::to_vec(self).map_err(|e| tokio_events::Error::SerializationError(e.to_string()))
            }

            fn deserialize_event(bytes: &[u8]) -> tokio_events::Result<Self> {
                serde_json::from_slice(bytes).map_err(|e| tokio_events::Error::SerializationError(e.to_string()))
            }
        }
    };

    let event_type_impl = if let Some(val) = event_type_val {
        quote! { #val }
    } else {
        quote! { concat!(module_path!(), "::", stringify!(#name)) }
    };

    let expanded = quote! {
        impl tokio_events::Event for #name {
            fn event_type() -> &'static str {
                #event_type_impl
            }

            #serialization_impl
        }
    };

    TokenStream::from(expanded)
}

#[proc_macro_derive(Remote, attributes(remote))]
pub fn derive_remote(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;

    let mut topic_val = None;

    for attr in &input.attrs {
        if attr.path().is_ident("remote") {
            let _ = attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("topic") {
                    let value = meta.value()?;
                    let lit: LitStr = value.parse()?;
                    topic_val = Some(lit.value());
                    Ok(())
                } else {
                    Err(meta.error("unrecognized remote attribute"))
                }
            });
        }
    }

    let topic_str = match topic_val {
        Some(t) => t,
        None => {
            return syn::Error::new_spanned(
                name,
                "tokio_events: #[remote(topic = \"...\")] attribute is required",
            )
            .to_compile_error()
            .into();
        }
    };

    let segments = topic_str.split('.').filter(|s| !s.is_empty()).count();
    if segments < 3 {
        return syn::Error::new_spanned(
            name,
            format!(
                "tokio_events: remote topic must contain at least 3 dot-separated segments (e.g., domain.service.event). Found: '{}'",
                topic_str
            ),
        )
        .to_compile_error()
        .into();
    }

    let expanded = quote! {
        impl tokio_events::Remote for #name {
            fn remote_topic() -> &'static str {
                #topic_str
            }
        }
    };

    TokenStream::from(expanded)
}
