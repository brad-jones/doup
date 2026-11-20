//! The items both runtime strategies generate the same way. They reach a class's storage through the
//! hidden `__fields()` method each backend defines.

use super::Model;
use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{Type, spanned::Spanned};

/// `XData`, the initial values passed to `from_data`.
pub fn data_struct(model: &Model) -> TokenStream {
  let Model { vis, data_attrs, .. } = model;
  let data_name = model.data_name();
  let data_generics = &model.generics.data;
  let data_where = &model.generics.data.where_clause;
  let doc = format!("The initial values of a [`{}`].", model.name);
  let base = model.parent_data().map(|ty| {
    quote! {
      /// The parent class's initial values.
      #vis base: #ty,
    }
  });
  let fields = model.fields.iter().map(|f| {
    let (attrs, vis, ident, ty) = (&f.attrs, &f.vis, &f.ident, &f.ty);
    quote! { #(#attrs)* #vis #ident: #ty, }
  });
  quote! {
    #(#data_attrs)*
    #[doc = #doc]
    #[allow(dead_code)]
    #vis struct #data_name #data_generics #data_where {
      #base
      #(#fields)*
    }
  }
}

/// The storage's field declarations, one pointer per field.
pub fn storage_fields(model: &Model) -> Vec<TokenStream> {
  let krate = &model.krate;
  model
    .fields
    .iter()
    .map(|f| {
      let (ident, ty, strategy) = (&f.ident, &f.ty, &f.pointer_strategy);
      quote_spanned! {ty.span()=> #ident: <#strategy as #krate::PointerStrategy>::SyncPtr<#ty>, }
    })
    .collect()
}

/// Initializers for the storage's fields, from a `data: XData` in scope.
pub fn storage_inits(model: &Model) -> Vec<TokenStream> {
  let krate = &model.krate;
  model
    .fields
    .iter()
    .map(|f| {
      let ident = &f.ident;
      quote! { #ident: #krate::MutablePointer::new(data.#ident), }
    })
    .collect()
}

/// Every item that doesn't depend on the runtime strategy. `identity` is an expression giving the address
/// of `self`'s allocation.
pub fn shared_items(model: &Model, identity: TokenStream) -> TokenStream {
  let krate = &model.krate;
  let private = quote!(#krate::__private);
  let name = &model.name;
  let own = model.own();
  let (ig, _, wc) = model.generics.full.split_for_impl();
  let accessors = model.fields.iter().map(|f| accessors(model, f));
  let data = data_struct(model);
  let data_ty = model.data_ty();

  let parent_items = model.parent.as_ref().map(|parent| {
    quote! {
      impl #ig #private::SubClass for #own #wc {
        type Base = #parent;
        #[inline]
        fn base_ref(&self) -> &#parent {
          self
        }
      }
    }
  });

  let conversions = model.ancestors.iter().map(|a| {
    let target = &a.handle;
    quote! {
      /// Returns a handle to the same object, viewed as an ancestor.
      impl #ig ::core::convert::From<&#own> for #target #wc {
        #[inline]
        fn from(value: &#own) -> Self {
          let view: &#target = value;
          ::core::clone::Clone::clone(view)
        }
      }

      /// Returns a handle to the same object, viewed as an ancestor.
      impl #ig ::core::convert::From<#own> for #target #wc {
        #[inline]
        fn from(value: #own) -> Self {
          ::core::convert::From::from(&value)
        }
      }
    }
  });

  let mut debug_where = model.generics.debug_where.clone();
  if let Some(parent) = &model.parent {
    debug_where
      .predicates
      .push(syn::parse_quote!(#parent: #private::ClassDebug));
  }
  let debug_parent = model.parent.as_ref().map(|parent| {
    quote! { <#parent as #private::ClassDebug>::fmt_fields(::core::ops::Deref::deref(self), d); }
  });
  let debug_fields = model.fields.iter().map(|f| {
    let ident = &f.ident;
    let label = f.bare().to_string();
    quote! {
      d.field(#label, &#krate::MutablePointer::read_with(&self.__fields().#ident, ::core::clone::Clone::clone));
    }
  });
  let label = name.to_string();

  let open = model.open.then(|| quote!(impl #ig #private::OpenClass for #own #wc {}));
  let runtime_check = model.runtime_path.as_ref().map(|path| {
    let check = if model.runtime.is_unsafe() {
      quote!(assert_unsafe_runtime)
    } else {
      quote!(assert_safe_runtime)
    };
    quote!(const _: () = #private::#check::<#path>();)
  });

  quote! {
    #data

    #[allow(dead_code)]
    impl #ig #own #wc {
      /// Creates a new object from its initial values.
      #[inline]
      fn from_data(data: #data_ty) -> Self {
        Self::__from_data(data)
      }

      /// Returns `true` if both handles refer to the same object, whatever class they view it as.
      #[inline]
      pub fn ptr_eq<__T: #private::Class>(&self, other: &__T) -> bool {
        ::core::ptr::eq(#private::Class::__identity(self), #private::Class::__identity(other))
      }

      /// Returns a handle to the same object, viewed as this class or any class it (transitively)
      /// extends. See `Extends`.
      #[inline]
      pub fn upcast<__B, __I>(&self) -> __B
      where
        Self: #krate::Extends<__B, __I>,
      {
        #krate::Extends::upcast(self)
      }

      #(#accessors)*
    }

    impl #ig #private::Class for #own #wc {
      #[inline]
      fn __identity(&self) -> *const () {
        #identity
      }
    }

    #open
    #parent_items
    #(#conversions)*

    /// Compares by identity, see `ptr_eq`.
    impl #ig ::core::cmp::PartialEq for #own #wc {
      #[inline]
      fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
      }
    }

    impl #ig ::core::cmp::Eq for #own #wc {}

    /// Hashes the identity of the object, consistent with `PartialEq`.
    impl #ig ::core::hash::Hash for #own #wc {
      fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) {
        ::core::ptr::hash(#private::Class::__identity(self), state)
      }
    }

    impl #ig #private::ClassDebug for #own #debug_where {
      #[allow(unused_variables)]
      fn fmt_fields(&self, d: &mut ::core::fmt::DebugStruct<'_, '_>) {
        #debug_parent
        #(#debug_fields)*
      }
    }

    /// Prints the field values, including those of the ancestors, of the class this handle views the
    /// object as.
    impl #ig ::core::fmt::Debug for #own #debug_where {
      fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        let mut d = f.debug_struct(#label);
        #private::ClassDebug::fmt_fields(self, &mut d);
        d.finish()
      }
    }

    /// Clones the handle, so both refer to the same object.
    impl #ig ::core::convert::From<&#own> for #own #wc {
      #[inline]
      fn from(value: &#own) -> Self {
        ::core::clone::Clone::clone(value)
      }
    }

    #runtime_check
  }
}

fn accessors(model: &Model, field: &super::fields::ClassField) -> TokenStream {
  let krate = &model.krate;
  let (ident, vis, ty, setter_vis) = (&field.ident, &field.vis, &field.ty, &field.setter_vis);
  let bare = field.bare();
  let setter = format_ident!("set_{}", bare);
  let updater = format_ident!("update_{}", bare);

  let docs: Vec<_> = field.attrs.iter().filter(|a| a.path().is_ident("doc")).collect();
  let getter_doc = if docs.is_empty() {
    let doc = format!("Returns the current value of `{bare}`.");
    quote!(#[doc = #doc])
  } else {
    quote!(#(#docs)*)
  };
  let setter_doc = format!("Replaces the value of `{bare}`.");
  let updater_doc = format!(
    "Modifies `{bare}` in place and returns the result of `f`.\n\n`f` may be called more than once, and must not \
     access `{bare}` itself."
  );

  quote! {
    #getter_doc
    #[inline]
    #vis fn #ident(&self) -> #ty {
      #krate::MutablePointer::read_with(&self.__fields().#ident, ::core::clone::Clone::clone)
    }

    #[doc = #setter_doc]
    #[inline]
    #setter_vis fn #setter(&self, value: impl ::core::convert::Into<#ty>) {
      let value: #ty = value.into();
      #krate::MutablePointer::write_with(&self.__fields().#ident, |v| *v = ::core::clone::Clone::clone(&value))
    }

    #[doc = #updater_doc]
    #[inline]
    #setter_vis fn #updater<__R>(&self, f: impl ::core::ops::FnMut(&mut #ty) -> __R) -> __R {
      #krate::MutablePointer::write_with(&self.__fields().#ident, f)
    }
  }
}

/// An open class's descriptor, see the `descriptor` module. `dispatch` is the backend's object trait or
/// vtable type.
pub fn descriptor(model: &Model, fields: &Type, dispatch: &Type) -> TokenStream {
  if !model.open {
    return quote!();
  }
  let Model {
    vis, name, ancestors, ..
  } = model;
  let descriptor = format_ident!("__class_descriptor_{}", name);
  let params = &model.generics.params;
  let is_unsafe = model.runtime.is_unsafe();
  let own = model.own();
  let data = model.data_ty();
  let virtuals = model.own_virtuals();
  // A callback may run in another crate, so anchor this class's `$crate` to its own declaration.
  let record = quote_spanned! {name.span()=>
    { $crate::#own; $crate::#fields; $crate::#data; $crate::#dispatch; #(#virtuals)* }
  };
  quote! {
    #[doc(hidden)]
    #[macro_export]
    macro_rules! #descriptor {
      ($callback:path, $payload:tt) => {
        $callback! { $payload [#(#params)*] #is_unsafe #(#ancestors)* #record }
      };
    }

    #[doc(hidden)]
    #[allow(unused_imports)]
    #vis use #descriptor as #name;
  }
}
