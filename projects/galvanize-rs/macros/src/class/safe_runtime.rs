//! Generates classes for `SafeRuntimeStrategy`, without any `unsafe` code.
//!
//! Each class declares an object trait (`__NameObject`), a subtrait of its parent's. The most-derived
//! storage implements its own and each ancestor's trait, nesting the parent's storage by value. An open
//! class's handle holds an `Arc<dyn __NameObject>` and its parent's handle (one view per level), all
//! pointing at the same allocation; a final class's handle holds an `Arc` of its concrete storage.
//!
//! A virtual call dispatches on the trait object. A method a class doesn't override forwards to its parent
//! storage's implementation, until it reaches the class that declared it (where an abstract method's
//! default is unreachable, as concrete classes must override it). An override reconstructs its class's
//! typed handle with a checked downcast, then runs the user's body on it. `super_x` calls the parent
//! storage's implementation directly, passing the original handle so calls it makes stay late-bound.

use super::common;
use super::descriptor::Ancestor;
use super::descriptor::class_name;
use super::methods::{FutureLifetime, Kind, Method, arguments};
use super::{Model, Type};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Error, Result, ReturnType, fold::Fold};

pub fn generate(model: &Model) -> Result<TokenStream> {
  let krate = &model.krate;
  let private = quote!(#krate::__private);
  let Model {
    vis, ancestors, open, ..
  } = model;
  let (fields_name, own_fields) = model.hidden_ty("Fields");
  let (object_name, own_object) = model.hidden_ty("Object");
  let handle_name = model.handle_name();
  let full = &model.generics.full;
  let (ig, _, wc) = full.split_for_impl();
  let own = model.own();
  let parent = ancestors.last();

  let chain: Vec<Ancestor> = ancestors
    .iter()
    .cloned()
    .chain([Ancestor {
      handle: own.clone(),
      fields: own_fields.clone(),
      data: model.data_ty(),
      dispatch: own_object.clone(),
      methods: model.own_virtuals(),
      overrides: model.own_overrides(),
      class_trait: None,
      statics: Vec::new(),
      static_overrides: Vec::new(),
    }])
    .collect();
  let root = &chain[0].dispatch;
  let supertraits = parent.map_or_else(
    || quote!(::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    |p| {
      let p = &p.dispatch;
      quote!(#p)
    },
  );
  let root_members = parent.is_none().then(|| {
    quote! {
      fn __is(&self, target: ::core::any::TypeId) -> bool;
      fn __cast(self: ::std::sync::Arc<Self>, out: &mut dyn ::core::any::Any);
    }
  });

  let trait_methods = model
    .methods
    .iter()
    .filter(|m| m.kind == Kind::Virtual)
    .map(|method| {
      let method_name = method.name();
      let body_name = format_ident!("__body_{method_name}");
      let args = arguments(&method.item)?;
      let signature = object_signature(method, &own);
      let body = if method.is_abstract {
        let message = format!("`{}::{method_name}` is abstract", class_name(&own));
        quote!(::core::unreachable!(#message))
      } else if method.is_async() {
        quote!(::std::boxed::Box::pin(__this.#body_name(#(#args),*)))
      } else {
        quote!(__this.#body_name(#(#args),*))
      };
      Ok(quote! {
        #signature {
          match <Self as #own_object>::__parent(self) {
            ::core::option::Option::Some(parent) => parent.#method_name(__this, #(#args),*),
            ::core::option::Option::None => #body,
          }
        }
      })
    })
    .collect::<Result<Vec<_>>>()?;

  let object_impls = chain
    .iter()
    .enumerate()
    .map(|(index, ancestor)| object_impl(model, &chain, index, ancestor))
    .collect::<Result<Vec<_>>>()?;

  let handle_methods = handle_methods(model, &own_object)?;
  let trait_impls = super::interface::trait_impls(model)?;
  let statics = super::statics::generate(model)?;
  let data_ty = model.data_ty();
  let from_data = (!model.is_abstract).then(|| {
    quote! {
      #[inline]
      fn __from_data(data: #data_ty) -> Self {
        Self::__from_obj(::std::sync::Arc::new(Self::__build(data)))
      }
    }
  });
  let storage_fields = common::storage_fields(model);
  let storage_inits = common::storage_inits(model);
  let obj_type = if *open {
    quote!(dyn #own_object)
  } else {
    quote!(#own_fields)
  };
  let parent_handle = parent.map(|a| &a.handle);
  let storage_base = parent.map(|p| {
    let fields = &p.fields;
    quote!(pub base: #fields,)
  });
  let handle_base = parent_handle.map(|p| quote!(__base: #p,));
  let init_base = parent_handle.map(|p| quote!(base: <#p>::__build(data.base),));
  // Binding the clone first lets it coerce to the parent's trait object at the call.
  let from_base =
    parent_handle.map(|p| quote!(__base: <#p>::__from_obj({ let obj = ::std::sync::Arc::clone(&obj); obj }),));
  let clone_base = parent_handle.map(|_| quote!(__base: ::core::clone::Clone::clone(&self.__inner.__base),));
  let deref = parent_handle.map(|p| {
    quote! {
      /// Gives access to the parent class's fields and methods.
      impl #ig ::core::ops::Deref for #own #wc {
        type Target = #p;
        #[inline]
        fn deref(&self) -> &#p {
          &self.__inner.__base
        }
      }
    }
  });
  let shared = common::shared_items(
    model,
    quote!(::std::sync::Arc::as_ptr(&self.__inner.__obj).cast::<()>()),
  );
  let descriptor = common::descriptor(model, &own_fields, &own_object);

  Ok(quote! {
    #[doc(hidden)]
    #vis struct #fields_name #full #wc {
      #storage_base
      #(#storage_fields)*
    }

    #[doc(hidden)]
    #vis trait #object_name #full: #supertraits #wc {
      fn __parent(&self) -> ::core::option::Option<&dyn #own_object>;
      fn __fields(&self) -> &#own_fields;
      #root_members
      #(#trait_methods)*
    }

    #(#object_impls)*

    #[doc(hidden)]
    #vis struct #handle_name #full #wc {
      __obj: ::std::sync::Arc<#obj_type>,
      #handle_base
    }

    impl #ig #own #wc {
      #[doc(hidden)]
      #[inline]
      pub fn __from_obj(obj: ::std::sync::Arc<#obj_type>) -> Self {
        Self { __inner: #handle_name { #from_base __obj: obj } }
      }

      #[doc(hidden)]
      pub fn __build(data: #data_ty) -> #own_fields {
        #fields_name { #init_base #(#storage_inits)* }
      }

      #from_data

      #[inline]
      fn __fields(&self) -> &#own_fields {
        <_ as #own_object>::__fields(::core::ops::Deref::deref(&self.__inner.__obj))
      }

      /// Returns `true` if the object is a `T`, or a subclass of `T`.
      #[inline]
      pub fn is<__T: #private::Class>(&self) -> bool {
        <_ as #root>::__is(::core::ops::Deref::deref(&self.__inner.__obj), ::core::any::TypeId::of::<__T>())
      }

      /// Returns a handle to the same object viewed as a `T`, if it is one.
      #[inline]
      pub fn downcast<__T: #private::Class>(&self) -> ::core::option::Option<__T> {
        if !self.is::<__T>() {
          return ::core::option::Option::None;
        }
        let mut result = ::core::option::Option::None::<__T>;
        <_ as #root>::__cast(::std::sync::Arc::clone(&self.__inner.__obj), &mut result);
        result
      }

      #(#handle_methods)*
    }

    /// Clones the handle, so both refer to the same object.
    impl #ig ::core::clone::Clone for #own #wc {
      #[inline]
      fn clone(&self) -> Self {
        Self { __inner: #handle_name { __obj: ::std::sync::Arc::clone(&self.__inner.__obj), #clone_base } }
      }
    }

    #deref
    #shared
    #trait_impls
    #statics
    #descriptor
  })
}

/// The object-trait signature of a virtual method, with an explicit handle receiver `__this`.
fn object_signature(method: &Method, receiver: &Type) -> TokenStream {
  let name = method.name();
  let params = method.item.sig.inputs.iter().skip(1);
  let output = &method.item.sig.output;
  if !method.is_async() {
    return quote! { fn #name(&self, __this: &#receiver, #(#params),*) #output };
  }
  let params = params.cloned().map(|arg| FutureLifetime.fold_fn_arg(arg));
  let output = match output {
    ReturnType::Default => quote!(()),
    ReturnType::Type(_, ty) => quote!(#ty),
  };
  quote! {
    fn #name<'__class_future>(
      &'__class_future self, __this: &'__class_future #receiver, #(#params),*
    ) -> ::core::pin::Pin<::std::boxed::Box<
      dyn ::core::future::Future<Output = #output> + ::core::marker::Send + '__class_future
    >>
  }
}

/// The class's storage's implementation of the object trait of `chain[index]`, an ancestor or itself.
fn object_impl(model: &Model, chain: &[Ancestor], index: usize, ancestor: &Ancestor) -> Result<TokenStream> {
  let (ig, _, wc) = model.generics.full.split_for_impl();
  let own = model.own();
  let (_, own_fields) = model.hidden_ty("Fields");
  let depth = model.ancestors.len();
  let trait_ty = &ancestor.dispatch;
  let field_ty = &ancestor.fields;
  let parent_value = if index < depth {
    quote!(::core::option::Option::Some(&self.base))
  } else {
    quote!(::core::option::Option::None)
  };
  let mut field_path = quote!(self);
  for _ in index..depth {
    field_path = quote!(#field_path.base);
  }
  let root_impl = (index == 0).then(|| {
    let handles: Vec<_> = chain.iter().rev().map(|a| &a.handle).collect();
    quote! {
      fn __is(&self, target: ::core::any::TypeId) -> bool {
        false #(|| target == ::core::any::TypeId::of::<#handles>())*
      }
      fn __cast(self: ::std::sync::Arc<Self>, out: &mut dyn ::core::any::Any) {
        #(
          if let ::core::option::Option::Some(slot) = out.downcast_mut::<::core::option::Option<#handles>>() {
            *slot = ::core::option::Option::Some(<#handles>::__from_obj(self));
            return;
          }
        )*
        ::core::panic!("class cast requested a type outside the hierarchy");
      }
    }
  });
  let overrides = model
    .methods
    .iter()
    .filter(|m| m.kind == Kind::Override && model.declaring_ancestor(m) == index)
    .map(|method| {
      let body_name = format_ident!("__body_{}", method.name());
      let signature = object_signature(method, &ancestor.handle);
      let args = arguments(&method.item)?;
      let view = quote! {
        __this.downcast::<#own>().expect("an override's receiver belongs to its class")
      };
      let body = if method.is_async() {
        quote! {
          ::std::boxed::Box::pin(async move {
            let view = #view;
            view.#body_name(#(#args),*).await
          })
        }
      } else {
        quote!(#view.#body_name(#(#args),*))
      };
      Ok(quote! {
        #[inline]
        #signature {
          #body
        }
      })
    })
    .collect::<Result<Vec<_>>>()?;
  Ok(quote! {
    impl #ig #trait_ty for #own_fields #wc {
      #[inline]
      fn __parent(&self) -> ::core::option::Option<&dyn #trait_ty> { #parent_value }
      #[inline]
      fn __fields(&self) -> &#field_ty { &#field_path }
      #root_impl
      #(#overrides)*
    }
  })
}

/// The user's methods, plus a wrapper for each virtual method and override, which dispatches to the
/// object's implementation, and a `super_x` for each override of an implemented method.
fn handle_methods(model: &Model, own_object: &Type) -> Result<Vec<TokenStream>> {
  let mut out = Vec::new();
  for method in &model.methods {
    let item = &method.item;
    if method.kind == Kind::Ordinary {
      out.push(quote!(#item));
      continue;
    }
    let method_name = method.name();
    if !method.is_abstract {
      let mut body = item.clone();
      body.sig.ident = format_ident!("__body_{method_name}");
      body.vis = syn::Visibility::Inherited;
      body.attrs.retain(|a| !a.path().is_ident("doc"));
      out.push(quote!(#body));
    }
    let args = arguments(item)?;
    let await_call = method.is_async().then(|| quote!(.await));
    let receiver = method.receiver()?;
    let (declaring, allow) = if method.kind == Kind::Virtual {
      (own_object, None)
    } else {
      // An override's wrapper documents it, and is only called directly in its own module when private.
      let declaring = &model.ancestors[model.declaring_ancestor(method)].dispatch;
      (declaring, Some(quote!(#[allow(dead_code)])))
    };
    let (vis, sig, attrs) = (&item.vis, &item.sig, &item.attrs);
    out.push(quote! {
      #(#attrs)*
      #allow
      #vis #sig {
        <_ as #declaring>::#method_name(::core::ops::Deref::deref(&#receiver.__inner.__obj), #receiver, #(#args),*) #await_call
      }
    });
    if method.has_super {
      let mut sig = item.sig.clone();
      sig.ident = format_ident!("super_{method_name}");
      let parent_fields = &model
        .ancestors
        .last()
        .ok_or_else(|| Error::new_spanned(item, "an override needs a parent"))?
        .fields;
      let doc = format!("Calls the parent class's implementation of `{method_name}`.");
      out.push(quote! {
        #[doc = #doc]
        #[inline]
        #[allow(dead_code)]
        #sig {
          <#parent_fields as #declaring>::#method_name(
            &<_ as #own_object>::__fields(::core::ops::Deref::deref(&#receiver.__inner.__obj)).base, #receiver, #(#args),*
          ) #await_call
        }
      });
    }
  }
  Ok(out)
}
