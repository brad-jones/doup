//! Procedural macros for `galvanize-rs`. Consumers should use the re-exports
//! from the `galvanize_rs` crate rather than depending on this crate directly.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use std::collections::HashSet;
use syn::{Attribute, Error, Fields, ItemStruct, ItemTrait, Type, ext::IdentExt, parse_macro_input, parse_quote};

/// Method names a field must not use: they would shadow `GPointer` methods reached through `Deref`
/// and generated methods.
const RESERVED_METHODS: &[&str] = &[
  "read_with",
  "write_with",
  "get",
  "set",
  "set_from",
  "duplicate",
  "ptr_eq",
];

/// Turns a plain struct into a heap allocated, reference-like "class".
///
/// Given `struct Foo { id: i32, name: String }` this generates:
///
/// - `FooData`, the struct you wrote, holding the actual fields.
/// - `Foo`, a cheaply cloneable handle (`GPointer<FooData>`). Cloning a `Foo`
///   yields another handle to the *same* object, like a reference in C# or Java.
/// - No constructor: write your own `new` that returns `Self::from(FooData { .. })`
///   (`From<FooData>` is generated), or derive `Default` (see below).
/// - A getter `foo.id()` and setter `foo.set_id(value)` per field. Getters
///   clone only the field, so a field of another class type returns a cheap
///   handle to the shared object.
/// - `with_id(|v| ..)` and `update_id(|v| ..)` per field, to read or mutate a field in
///   place under the lock without cloning it (e.g. `foo.update_items(|v| v.push(1))`).
/// - `duplicate()`, returning a new `Foo` with its own copy of the fields. It is *shallow*
///   in the handles: a field that is itself a handle (another class, a `GList`, ...) is
///   still shared with the original. Clone those explicitly if you need independence.
/// - `PartialEq`/`Eq` based on identity (`GPointer::ptr_eq`), not on field values.
/// - `Deref` to the inner `GPointer<FooData>` for `read_with`/`write_with`
///   when you need to touch several fields under one lock.
///
/// Attributes you place on the struct are split: `#[derive(..)]` goes onto the
/// data struct (`Clone` is always derived for you), everything else (docs, etc.)
/// stays on the handle. If `Debug` is derived, the handle implements it too.
///
/// If `Default` is derived, it is derived on the data struct and the handle gets
/// `Default` too (a fresh object with every field defaulted), so `Foo::default()`
/// works, including for fields whose type is another class.
///
/// # Pointer strategy
///
/// The handle uses the default `PointerStrategy`. Pick another with
/// `#[class(pointer_strategy = ArcSwapStrategy)]` (any type implementing `PointerStrategy`).
///
/// # Generics
///
/// Generic structs work: `struct Node<T> { .. }` gives `NodeData<T>` and `Node<T>`.
/// Every type parameter additionally gets a `Clone` bound on the generated items,
/// because the shared value must be `Clone`. Derives such as `Debug` and `Default`
/// keep their usual conditions (`T: Debug`, `T: Default`).
///
/// Field doc comments are copied to the generated getter and setter. A field whose
/// name (or whose generated `set_`/`with_`/`update_` name) collides with another
/// method, or with a `GPointer` method such as `get`, is a compile error.
///
/// Only structs with named fields are supported.
#[proc_macro_attribute]
pub fn class(args: TokenStream, input: TokenStream) -> TokenStream {
  let mut strategy = None;
  let parser = syn::meta::parser(|meta| {
    if meta.path.is_ident("pointer_strategy") {
      strategy = Some(meta.value()?.parse::<Type>()?);
      Ok(())
    } else {
      Err(meta.error("unsupported option, expected `pointer_strategy = <PointerStrategy type>`"))
    }
  });
  parse_macro_input!(args with parser);
  let item = parse_macro_input!(input as ItemStruct);
  expand(item, strategy).unwrap_or_else(Error::into_compile_error).into()
}

fn expand(item: ItemStruct, strategy: Option<Type>) -> syn::Result<TokenStream2> {
  let Fields::Named(named) = &item.fields else {
    return Err(Error::new_spanned(
      &item,
      "#[class] requires a struct with named fields",
    ));
  };

  let vis = &item.vis;
  let handle = &item.ident;
  let data = format_ident!("{}Data", handle);
  let strategy = strategy.map_or_else(|| quote!(::galvanize_rs::DefaultPointerStrategy), |t| quote!(#t));

  // Every type parameter gains a `Clone` bound: `GPointer` requires its value to be `Clone`.
  let mut generics = item.generics.clone();
  for param in generics.type_params_mut() {
    param.bounds.push(parse_quote!(::core::clone::Clone));
  }
  let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
  let data_ty = quote!(#data #ty_generics);
  let handle_ty = quote!(#handle #ty_generics);
  let pointer = quote!(::galvanize_rs::GPointer<#data_ty, #strategy>);
  let where_with = |bound: TokenStream2| {
    let mut w = generics.where_clause.clone().unwrap_or_else(|| parse_quote!(where));
    w.predicates.push(parse_quote!(#data_ty: #bound));
    w
  };

  let (derives, handle_attrs): (Vec<&Attribute>, Vec<&Attribute>) =
    item.attrs.iter().partition(|a| a.path().is_ident("derive"));
  let mut has_clone = false;
  let mut has_debug = false;
  let mut has_default = false;
  for attr in &derives {
    attr.parse_nested_meta(|meta| {
      let last = meta.path.segments.last().map(|s| s.ident.to_string());
      has_clone |= last.as_deref() == Some("Clone");
      has_debug |= last.as_deref() == Some("Debug");
      has_default |= last.as_deref() == Some("Default");
      Ok(())
    })?;
  }
  let derive_clone = (!has_clone).then(|| quote!(#[derive(Clone)]));

  let fields = &named.named;
  let mut taken: HashSet<String> = RESERVED_METHODS.iter().map(|m| m.to_string()).collect();
  let mut accessors = Vec::new();
  for f in fields {
    let (fvis, name, ty) = (&f.vis, f.ident.as_ref().expect("named field"), &f.ty);
    let base = name.unraw();
    let (setter, with, update) = (
      format_ident!("set_{base}"),
      format_ident!("with_{base}"),
      format_ident!("update_{base}"),
    );
    for generated in [&base, &setter, &with, &update] {
      if !taken.insert(generated.to_string()) {
        return Err(Error::new_spanned(
          name,
          format!("the generated method `{generated}` for this field collides with another method or field"),
        ));
      }
    }
    let docs = f.attrs.iter().filter(|a| a.path().is_ident("doc"));
    let docs: Vec<_> = docs.collect();
    let getter_doc = format!("Returns a clone of `{base}`.");
    let setter_doc = format!("Replaces `{base}`; every handle to this object observes the change.");
    let with_doc = format!("Runs `f` with shared access to `{base}`, without cloning it.");
    let update_doc = format!(
      "Runs `f` with exclusive access to `{base}`. `f` may be retried by some strategies, so keep it free of side effects."
    );
    accessors.push(quote! {
      #(#docs)*
      #[doc = #getter_doc]
      #fvis fn #name(&self) -> #ty {
        self.0.read_with(|data| data.#name.clone())
      }

      #(#docs)*
      #[doc = #setter_doc]
      #fvis fn #setter(&self, value: #ty) {
        self.0.write_with(|data| data.#name = value.clone());
      }

      #[doc = #with_doc]
      #fvis fn #with<R>(&self, f: impl ::core::ops::FnOnce(&#ty) -> R) -> R {
        self.0.read_with(|data| f(&data.#name))
      }

      #[doc = #update_doc]
      #fvis fn #update<R>(&self, mut f: impl ::core::ops::FnMut(&mut #ty) -> R) -> R {
        self.0.write_with(|data| f(&mut data.#name))
      }
    });
  }

  let default_impl = has_default.then(|| {
    let w = where_with(quote!(::core::default::Default));
    quote! {
      impl #impl_generics ::core::default::Default for #handle_ty #w {
        fn default() -> Self {
          Self(<#pointer>::with_strategy(<#data_ty as ::core::default::Default>::default()))
        }
      }
    }
  });

  let debug_impl = has_debug.then(|| {
    let w = where_with(quote!(::core::fmt::Debug));
    let type_args = generics.type_params().map(|p| &p.ident);
    quote! {
      impl #impl_generics ::core::fmt::Debug for #handle_ty #w {
        fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
          let type_args: ::std::vec::Vec<::std::string::String> =
            ::std::vec![#(::galvanize_rs::short_type_name::<#type_args>()),*];
          let name = if type_args.is_empty() {
            ::std::string::String::from(::core::stringify!(#handle))
          } else {
            ::std::format!("{}<{}>", ::core::stringify!(#handle), type_args.join(", "))
          };
          f.debug_tuple(&name).field(&self.0).finish()
        }
      }
    }
  });

  let data_doc = format!("The fields backing [`{handle}`].");

  Ok(quote! {
    #[doc = #data_doc]
    #derive_clone
    #(#derives)*
    #vis struct #data #generics #where_clause #named

    #(#handle_attrs)*
    #[derive(Clone)]
    #vis struct #handle #generics (#pointer) #where_clause;

    impl #impl_generics #handle_ty #where_clause {
      /// Copies the current field values into a new, independent object.
      ///
      /// One level deep only: fields that are themselves handles (other classes, lists,
      /// pointers) stay shared with the original.
      #vis fn duplicate(&self) -> Self {
        Self(self.0.duplicate())
      }

      #(#accessors)*
    }

    impl #impl_generics ::core::convert::From<#data_ty> for #handle_ty #where_clause {
      fn from(data: #data_ty) -> Self {
        Self(<#pointer>::with_strategy(data))
      }
    }

    #default_impl

    impl #impl_generics ::core::ops::Deref for #handle_ty #where_clause {
      type Target = #pointer;

      fn deref(&self) -> &Self::Target {
        &self.0
      }
    }

    /// Identity comparison: true only if both handles refer to the same object.
    impl #impl_generics ::core::cmp::PartialEq for #handle_ty #where_clause {
      fn eq(&self, other: &Self) -> bool {
        ::galvanize_rs::GPointer::ptr_eq(&self.0, &other.0)
      }
    }

    impl #impl_generics ::core::cmp::Eq for #handle_ty #where_clause {}

    #debug_impl
  })
}

/// Generates a cloneable, type-erased wrapper for a trait, so values of different
/// classes that implement it can be stored together (e.g. in a `GList`).
///
/// Given `#[any] trait Shape { .. }` this keeps the trait and adds `AnyShape`:
///
/// - `AnyShape` wraps an `Arc<dyn Shape>` and is cheap to `Clone`.
/// - `AnyShape::new(shape)` takes any `Shape + 'static` by value.
/// - `From<&S>` for any `S: Shape + Clone + 'static` clones the handle for you, so
///   `GList::push` and `list!` accept `&circle` directly. There is deliberately no by-value
///   `From`: it would conflict, because a downstream crate could implement the trait for `&T`.
/// - `Deref<Target = dyn Shape>`, so trait methods can be called on the wrapper.
///
/// The wrapper has the trait's visibility. The trait must be object safe and have no
/// generics. Add `Send + Sync` supertraits if the wrapper needs to cross threads.
#[proc_macro_attribute]
pub fn any(args: TokenStream, input: TokenStream) -> TokenStream {
  let args = TokenStream2::from(args);
  if !args.is_empty() {
    return Error::new_spanned(args, "#[any] takes no arguments")
      .into_compile_error()
      .into();
  }
  let item = parse_macro_input!(input as ItemTrait);
  expand_any(item).unwrap_or_else(Error::into_compile_error).into()
}

fn expand_any(item: ItemTrait) -> syn::Result<TokenStream2> {
  if !item.generics.params.is_empty() {
    return Err(Error::new_spanned(
      &item.generics,
      "#[any] does not support generic traits",
    ));
  }
  let vis = &item.vis;
  let tr = &item.ident;
  let any = format_ident!("Any{}", tr);
  let doc = format!("A cloneable, type-erased handle to any [`{tr}`].");

  Ok(quote! {
    #item

    #[doc = #doc]
    #[derive(::core::clone::Clone)]
    #vis struct #any(::std::sync::Arc<dyn #tr>);

    impl #any {
      /// Wraps a value by value.
      #vis fn new<S: #tr + 'static>(shape: S) -> Self {
        Self(::std::sync::Arc::new(shape))
      }
    }

    /// Clones the handle for the caller, so `&value` can be passed where an `Any` is expected.
    impl<S: #tr + ::core::clone::Clone + 'static> ::core::convert::From<&S> for #any {
      fn from(value: &S) -> Self {
        Self(::std::sync::Arc::new(::core::clone::Clone::clone(value)))
      }
    }

    impl ::core::ops::Deref for #any {
      type Target = dyn #tr;

      fn deref(&self) -> &Self::Target {
        &*self.0
      }
    }
  })
}
