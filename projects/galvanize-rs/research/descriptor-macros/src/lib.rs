//! Research-only safe descriptor inheritance attributes.
#![forbid(unsafe_code)]

use galvanize_rs_inheritance_macro_support as support;
use proc_macro::TokenStream;
use quote::quote;

#[proc_macro_attribute]
pub fn class(attr: TokenStream, item: TokenStream) -> TokenStream {
  support::class(attr.into(), item.into(), quote!(::galvanize_rs_descriptor_macros)).into()
}

#[proc_macro_attribute]
pub fn class_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
  support::class_methods(attr.into(), item.into()).into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __complete(input: TokenStream) -> TokenStream {
  support::complete(
    input.into(),
    quote!(::galvanize_rs_descriptor_macros),
    support::generate_descriptor,
  )
  .into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __inherit(input: TokenStream) -> TokenStream {
  support::inherit(input.into(), support::generate_descriptor).into()
}
