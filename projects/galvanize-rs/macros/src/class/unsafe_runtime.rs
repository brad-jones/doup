//! Generates classes for `UnsafeRuntimeStrategy`, on the kernel in galvanize-rs's `unsafe_runtime`.
//!
//! The storage (`__NameFields`) and vtable (`__NameVTable`) are `repr(C)` and embed the *actual* declared
//! parent's as their first field, via the parent's `Class` associated types rather than anything a
//! descriptor claims. The handle is `repr(transparent)` over an invariant `Handle<Self>`.
//!
//! Each virtual method gets a slot in the declaring class's vtable, holding an `unsafe fn` thunk that views
//! its receiver as the class that installed it and runs the user's body (an abstract method's thunk is
//! unreachable, as concrete classes must override it). A subclass's vtable copies its parent's, then
//! replaces the slots it overrides. The wrappers of virtual methods and overrides call the slot in the
//! object's own table. `super_x` calls the slot in the declaring class's lexical parent's table, which each
//! vtable retains a reference to, passing the original receiver so calls it makes stay late-bound.

use super::common;
use super::descriptor::class_name;
use super::methods::{FutureLifetime, Kind, Method, arguments, future_lifetime};
use super::{Model, Type};
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, Result, ReturnType, fold::Fold};

/// A slot's type, the thunk's signature, and the `.await` the public wrapper needs, if any.
fn slot(method: &Method, receiver: &Type) -> (TokenStream, TokenStream, TokenStream) {
  let name = method.name();
  let thunk = format_ident!("__thunk_{name}");
  let output = &method.item.sig.output;
  let params: Vec<_> = method.item.sig.inputs.iter().skip(1).cloned().collect();
  if !method.is_async() {
    let types = params.iter().filter_map(|p| match p {
      FnArg::Typed(p) => Some(&p.ty),
      _ => None,
    });
    return (
      quote!(unsafe fn(&#receiver, #(#types),*) #output),
      quote!(unsafe fn #thunk(__this: &#receiver, #(#params),*) #output),
      quote!(),
    );
  }
  let params: Vec<_> = params.into_iter().map(|p| FutureLifetime.fold_fn_arg(p)).collect();
  let types = params.iter().filter_map(|p| match p {
    FnArg::Typed(p) => Some(&p.ty),
    _ => None,
  });
  let result = match output {
    ReturnType::Default => quote!(()),
    ReturnType::Type(_, ty) => quote!(#ty),
  };
  let lt = future_lifetime();
  let future = quote!(::core::pin::Pin<::std::boxed::Box<
    dyn ::core::future::Future<Output = #result> + ::core::marker::Send + #lt
  >>);
  (
    quote!(for<#lt> unsafe fn(&#lt #receiver, #(#types),*) -> #future),
    quote!(unsafe fn #thunk<#lt>(__this: &#lt #receiver, #(#params),*) -> #future),
    quote!(.await),
  )
}

pub fn generate(model: &Model) -> Result<TokenStream> {
  let krate = &model.krate;
  let private = quote!(#krate::__private);
  let rt = quote!(#private::unsafe_runtime);
  let Model {
    vis, ancestors, parent, ..
  } = model;
  let (fields_name, own_fields) = model.hidden_ty("Fields");
  let (table_name, own_table) = model.hidden_ty("VTable");
  let handle_name = model.handle_name();
  let full = &model.generics.full;
  let (ig, _, wc) = full.split_for_impl();
  let own = model.own();
  let params = &model.generics.params;
  let marker = quote!(::core::marker::PhantomData<fn(#(#params),*) -> (#(#params,)*)>);

  let mut slots = Vec::new();
  let mut initializers = Vec::new();
  let mut overrides = Vec::new();
  let mut handle_methods = Vec::new();
  for method in &model.methods {
    let item = &method.item;
    if method.kind == Kind::Ordinary {
      handle_methods.push(quote!(#item));
      continue;
    }
    let method_name = method.name();
    let body_name = format_ident!("__body_{method_name}");
    let thunk_name = format_ident!("__thunk_{method_name}");
    if !method.is_abstract {
      let mut body = item.clone();
      body.sig.ident = body_name.clone();
      body.vis = syn::Visibility::Inherited;
      body.attrs.retain(|a| !a.path().is_ident("doc"));
      handle_methods.push(quote!(#body));
    }
    let args = arguments(item)?;
    let receiver = if method.kind == Kind::Virtual {
      &own
    } else {
      &ancestors[model.declaring_ancestor(method)].handle
    };
    let (slot_type, thunk_sig, await_call) = slot(method, receiver);
    let invocation = quote!(view.#body_name(#(#args),*));
    let thunk_body = if method.is_abstract {
      let message = format!("`{}::{method_name}` is abstract", class_name(&own));
      quote!(::core::unreachable!(#message))
    } else if method.is_async() {
      quote! {
        let view: &Self = unsafe { #rt::view_unchecked(__this) };
        ::std::boxed::Box::pin(#invocation)
      }
    } else {
      quote! {
        let view: &Self = unsafe { #rt::view_unchecked(__this) };
        #invocation
      }
    };
    let allow_unused = method.is_abstract.then(|| quote!(#[allow(unused_variables)]));
    // The thunk is only installed in this class's table and its descendants' copies. Its slot's type
    // stops it being called detached from its object without `unsafe`.
    handle_methods.push(quote! {
      /// # Safety
      /// The receiver must be a `Self`, or a descendant of `Self`.
      #allow_unused
      #thunk_sig {
        #thunk_body
      }
    });
    let self_token = method.receiver()?;
    let (vis, attrs) = (&item.vis, &item.attrs);
    if method.kind == Kind::Virtual {
      slots.push(quote! {
        /// # Safety
        /// Calling this slot requires a receiver of the table's concrete class, not merely of the
        /// class whose table prefix it is in.
        pub #method_name: #slot_type,
      });
      initializers.push(quote!(#method_name: Self::#thunk_name,));
      let sig = &item.sig;
      handle_methods.push(quote! {
        #(#attrs)*
        #vis #sig {
          // SAFETY: the slot comes from this object's own table.
          unsafe { (#self_token.__inner.__handle.table().#method_name)(#self_token, #(#args),*) } #await_call
        }
      });
    } else {
      let index = model.declaring_ancestor(method);
      let mut path = quote!(table);
      let mut own_path = quote!(#self_token.__inner.__handle.table());
      for _ in index..ancestors.len() {
        path = quote!(#path.base);
        own_path = quote!(#own_path.base);
      }
      overrides.push(quote!(#path.#method_name = Self::#thunk_name;));
      let sig = &item.sig;
      // An override's wrapper documents it, and is only called directly in its own module when private.
      handle_methods.push(quote! {
        #(#attrs)*
        #[allow(dead_code)]
        #vis #sig {
          // SAFETY: the slot comes from this object's own table.
          unsafe { (#own_path.#method_name)(#self_token, #(#args),*) } #await_call
        }
      });
      if method.has_super {
        let mut sig = item.sig.clone();
        sig.ident = format_ident!("super_{method_name}");
        let mut super_path = quote!(#self_token.__inner.__handle.table().parent);
        for _ in index + 1..ancestors.len() {
          super_path = quote!(#super_path.base);
        }
        let doc = format!("Calls the parent class's implementation of `{method_name}`.");
        handle_methods.push(quote! {
          #[doc = #doc]
          #[inline]
          #[allow(dead_code)]
          #sig {
            // SAFETY: the parent's table holds slots of this object's ancestors.
            unsafe { (#super_path.#method_name)(#self_token, #(#args),*) } #await_call
          }
        });
      }
    }
  }

  let storage_fields = common::storage_fields(model);
  let storage_inits = common::storage_inits(model);
  let storage_base = parent.as_ref().map(|p| quote!(base: <#p as #rt::Class>::Fields,));
  let build_base = parent
    .as_ref()
    .map(|p| quote!(base: <#p as #rt::ClassData>::build(data.base),));
  let table_base = parent.as_ref().map(|p| {
    quote! {
      pub base: <#p as #rt::Class>::VTable,
      pub parent: &'static <#p as #rt::Class>::VTable,
    }
  });
  let table_init = parent.as_ref().map(|p| {
    quote! {
      base: ::core::clone::Clone::clone(#rt::vtable::<#p>()),
      parent: #rt::vtable::<#p>(),
    }
  });
  let table_clone_base = parent.as_ref().map(|_| {
    quote! {
      base: ::core::clone::Clone::clone(&self.base),
      parent: self.parent,
    }
  });
  let table_clone_slots = model.methods.iter().filter(|m| m.kind == Kind::Virtual).map(|m| {
    let name = m.name();
    quote!(#name: self.#name,)
  });
  let ancestry = parent.as_ref().map(|p| quote!(|| <#p as #rt::Class>::is_class(id)));
  let deref = parent.as_ref().map(|p| {
    quote! {
      /// Gives access to the parent class's fields and methods.
      impl #ig ::core::ops::Deref for #own #wc {
        type Target = #p;
        #[inline]
        fn deref(&self) -> &#p {
          // SAFETY: every object of this class is also a `#p`.
          unsafe { #rt::view_unchecked(self) }
        }
      }
    }
  });
  let data_ty = model.data_ty();
  let from_data = (!model.is_abstract).then(|| {
    quote! {
      #[inline]
      fn __from_data(data: #data_ty) -> Self {
        <Self as #rt::Class>::from_handle(#rt::Handle::new(<Self as #rt::ClassData>::build(data)))
      }
    }
  });
  let trait_impls = common::trait_impls(model)?;
  let shared = common::shared_items(model, quote!(self.__inner.__handle.identity()));
  let descriptor = common::descriptor(model, &own_fields, &own_table);

  Ok(quote! {
    #[doc(hidden)]
    #[repr(C)]
    #vis struct #fields_name #full #wc {
      #storage_base
      #(#storage_fields)*
      __marker: #marker,
    }

    #[doc(hidden)]
    #[repr(C)]
    #vis struct #table_name #full #wc {
      #table_base
      #(#slots)*
      __marker: #marker,
    }

    impl #ig ::core::clone::Clone for #own_table #wc {
      fn clone(&self) -> Self {
        Self { #table_clone_base #(#table_clone_slots)* __marker: ::core::marker::PhantomData }
      }
    }

    #[doc(hidden)]
    #[repr(transparent)]
    #vis struct #handle_name #full #wc {
      __handle: #rt::Handle<#own>,
      __invariant: #marker,
    }

    /// Clones the handle, so both refer to the same object.
    impl #ig ::core::clone::Clone for #own #wc {
      #[inline]
      fn clone(&self) -> Self {
        Self { __inner: #handle_name { __handle: ::core::clone::Clone::clone(&self.__inner.__handle), __invariant: ::core::marker::PhantomData } }
      }
    }

    // SAFETY: generated to the layout contract: transparent over Handle<Self>, repr(C) storage and table
    // with the declared parent's as a prefix, and slots that only accept this class or its descendants.
    unsafe impl #ig #rt::Class for #own #wc {
      type Fields = #own_fields;
      type VTable = #own_table;

      #[inline]
      fn handle(&self) -> &#rt::Handle<Self> {
        &self.__inner.__handle
      }

      #[inline]
      fn from_handle(handle: #rt::Handle<Self>) -> Self {
        Self { __inner: #handle_name { __handle: handle, __invariant: ::core::marker::PhantomData } }
      }

      fn is_class(id: ::core::any::TypeId) -> bool {
        id == ::core::any::TypeId::of::<Self>() #ancestry
      }

      fn build_vtable() -> #own_table {
        #[allow(unused_mut)]
        let mut table = #table_name {
          #table_init #(#initializers)* __marker: ::core::marker::PhantomData,
        };
        #(#overrides)*
        table
      }
    }

    impl #ig #rt::ClassData for #own #wc {
      type Data = #data_ty;

      fn build(data: #data_ty) -> #own_fields {
        #fields_name { #build_base #(#storage_inits)* __marker: ::core::marker::PhantomData }
      }
    }

    impl #ig #own #wc {
      #from_data

      #[inline]
      fn __fields(&self) -> &#own_fields {
        self.__inner.__handle.fields()
      }

      /// Returns `true` if the object is a `T`, or a subclass of `T`.
      #[inline]
      pub fn is<__T: #private::Class>(&self) -> bool {
        self.__inner.__handle.is::<__T>()
      }

      /// Returns a handle to the same object viewed as a `T`, if it is one.
      #[inline]
      pub fn downcast<__T: #rt::Class>(&self) -> ::core::option::Option<__T> {
        self.__inner.__handle.downcast::<__T>()
      }

      #(#handle_methods)*
    }

    #deref
    #shared
    #trait_impls
    #descriptor
  })
}
