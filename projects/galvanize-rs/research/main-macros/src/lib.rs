//! Research attributes generating the hardened prefix-layout design.
#![forbid(unsafe_code)]

use galvanize_rs_inheritance_macro_support::{
  self as support, Ancestor, FutureLifetime, Input, Kind, VirtualMethod, arguments,
};
use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use std::collections::{HashMap, HashSet};
use syn::{
  Error, FnArg, ImplItem, ImplItemFn, Result, ReturnType, Type,
  fold::{self, Fold},
  parse_quote,
};

/// Declares a class, completed by its following methods block.
#[proc_macro_attribute]
pub fn class(attr: TokenStream, item: TokenStream) -> TokenStream {
  support::class(attr.into(), item.into(), quote!(::galvanize_rs_main_macros)).into()
}

/// Supplies methods and exports compile-time ancestry metadata.
#[proc_macro_attribute]
pub fn class_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
  support::class_methods(attr.into(), item.into()).into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __complete(input: TokenStream) -> TokenStream {
  support::complete(input.into(), quote!(::galvanize_rs_main_macros), generate).into()
}

#[doc(hidden)]
#[proc_macro]
pub fn __inherit(input: TokenStream) -> TokenStream {
  support::inherit(input.into(), generate).into()
}

struct SignatureCheck(Option<Error>);

impl Fold for SignatureCheck {
  fn fold_type(&mut self, ty: Type) -> Type {
    if matches!(&ty, Type::ImplTrait(_))
      || matches!(&ty, Type::Path(p) if p.path.segments.first().is_some_and(|s| s.ident == "Self"))
    {
      self.0 = Some(Error::new_spanned(
        &ty,
        "Self-dependent and impl Trait virtual signatures are unsupported",
      ));
    }
    fold::fold_type(self, ty)
  }
}

fn slot(method: &ImplItemFn, receiver: &Type) -> (Tokens, Tokens, Tokens) {
  let name = &method.sig.ident;
  let thunk = format_ident!("__thunk_{name}");
  let output = &method.sig.output;
  let params: Vec<_> = method.sig.inputs.iter().skip(1).cloned().collect();
  if method.sig.asyncness.is_none() {
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
  let future = quote!(::core::pin::Pin<::std::boxed::Box<
    dyn ::core::future::Future<Output = #result> + ::core::marker::Send + '__class_future
  >>);
  (
    quote!(for<'__class_future> unsafe fn(&'__class_future #receiver, #(#types),*) -> #future),
    quote!(unsafe fn #thunk<'__class_future>(__this: &'__class_future #receiver, #(#params),*) -> #future),
    quote!(.await),
  )
}

fn generate(input: Input, ancestors: Vec<Ancestor>) -> Result<Tokens> {
  let opts = support::options(input.attr)?;
  let item = input.item;
  let name = &item.ident;
  let vis = &item.vis;
  let docs = &item.attrs;
  let fields_name = format_ident!("__{name}Fields");
  let table_name = format_ident!("__{name}VTable");
  let data_name = format_ident!("{name}Data");
  let descriptor_name = format_ident!("__descriptor_export_{name}");
  let full = support::generics(&item.generics)?;
  let (ig, tg, wc) = full.split_for_impl();
  let own: Type = parse_quote!(#name #tg);
  let params: Vec<_> = full.type_params().map(|p| &p.ident).collect();
  let marker = quote!(::core::marker::PhantomData<fn(#(#params),*) -> (#(#params,)*)>);
  let rt = quote!(::galvanize_rs_main_runtime);
  // Layout and ancestry always use the declared parent, never a descriptor's claimed field types.
  let parent = &opts.parent;
  if parent.is_some() != !ancestors.is_empty() {
    return Err(Error::new_spanned(
      name,
      "parent descriptor does not match the class declaration",
    ));
  }
  if input.methods.trait_.is_some()
    || input.methods.unsafety.is_some()
    || input.methods.self_ty.to_token_stream().to_string() != own.to_token_stream().to_string()
    || !input
      .methods
      .generics
      .type_params()
      .map(|p| &p.ident)
      .eq(item.generics.type_params().map(|p| &p.ident))
    || input.methods.generics.where_clause.is_some()
    || input
      .methods
      .generics
      .type_params()
      .any(|p| !p.bounds.is_empty() || p.default.is_some())
  {
    return Err(Error::new_spanned(
      &input.methods,
      "use an inherent class impl; put generic bounds on the struct",
    ));
  }
  for attr in docs {
    if !attr.path().is_ident("doc") {
      return Err(Error::new_spanned(
        attr,
        "only doc attributes are supported on class structs",
      ));
    }
  }
  let mut inherited = HashMap::new();
  for (i, ancestor) in ancestors.iter().enumerate() {
    for method in &ancestor.methods {
      inherited.insert(method.name.to_string(), (i, method.is_async));
    }
  }
  let mut reserved: HashSet<String> = ["from_data", "is", "downcast", "upcast", "ptr_eq"]
    .into_iter()
    .map(String::from)
    .collect();
  let mut field_names = Vec::new();
  let mut field_types = Vec::new();
  let mut field_vis = Vec::new();
  for field in &item.fields {
    let ident = field
      .ident
      .as_ref()
      .ok_or_else(|| Error::new_spanned(field, "expected named field"))?;
    if ident == "base"
      || ident.to_string().starts_with("__")
      || !field.attrs.is_empty()
      || !reserved.insert(ident.to_string())
      || !reserved.insert(format!("set_{ident}"))
    {
      return Err(Error::new_spanned(
        field,
        "reserved field name or unsupported field attributes",
      ));
    }
    field_names.push(ident);
    field_types.push(&field.ty);
    field_vis.push(&field.vis);
  }
  let mut methods = Vec::new();
  for method in input.methods.items {
    let ImplItem::Fn(mut method) = method else {
      return Err(Error::new_spanned(method, "class_methods accepts methods only"));
    };
    let kind = support::method_kind(&mut method)?;
    let label = method.sig.ident.to_string();
    if label.starts_with("__") || !reserved.insert(label.clone()) {
      return Err(Error::new_spanned(
        &method.sig.ident,
        "method conflicts with a generated member",
      ));
    }
    if kind != Kind::Ordinary {
      let mut check = SignatureCheck(None);
      for arg in method.sig.inputs.iter().skip(1) {
        if let FnArg::Typed(arg) = arg {
          check.fold_type(*arg.ty.clone());
        }
      }
      if let ReturnType::Type(_, ty) = &method.sig.output {
        check.fold_type(*ty.clone());
      }
      if let Some(error) = check.0 {
        return Err(error);
      }
    }
    match (kind, inherited.get(&label)) {
      (Kind::Virtual, Some(_)) => {
        return Err(Error::new_spanned(
          &method.sig.ident,
          "inherited virtual exists; use class_override",
        ));
      }
      (Kind::Override, None) => {
        return Err(Error::new_spanned(
          &method.sig.ident,
          "no ancestor declares this virtual method",
        ));
      }
      (Kind::Override, Some((_, is_async))) if *is_async != method.sig.asyncness.is_some() => {
        return Err(Error::new_spanned(
          &method.sig,
          "override must match the inherited method's asyncness",
        ));
      }
      _ => {}
    }
    methods.push((kind, method));
  }
  for (kind, method) in &methods {
    if *kind == Kind::Override && reserved.contains(&format!("super_{}", method.sig.ident)) {
      return Err(Error::new_spanned(
        &method.sig.ident,
        "super method conflicts with a user method",
      ));
    }
  }
  let mut slots = Vec::new();
  let mut initializers = Vec::new();
  let mut overrides = Vec::new();
  let mut handle_methods = Vec::new();
  let mut virtuals = Vec::new();
  for (kind, method) in &methods {
    if *kind == Kind::Ordinary {
      handle_methods.push(quote!(#method));
      continue;
    }
    let method_name = &method.sig.ident;
    let body_name = format_ident!("__body_{method_name}");
    let thunk_name = format_ident!("__thunk_{method_name}");
    let mut body = method.clone();
    body.sig.ident = body_name.clone();
    body.vis = syn::Visibility::Inherited;
    handle_methods.push(quote!(#body));
    let args = arguments(method)?;
    let receiver = if *kind == Kind::Virtual {
      &own
    } else {
      &ancestors[inherited[&method_name.to_string()].0].handle
    };
    let (slot_type, thunk_sig, await_call) = slot(method, receiver);
    let invocation = quote!(view.#body_name(#(#args),*));
    let thunk_body = if method.sig.asyncness.is_some() {
      quote!(::std::boxed::Box::pin(#invocation))
    } else {
      invocation
    };
    // The unsafe thunk is installed only in this class's table and descendant copies. Its slot type
    // prevents detached safe invocation; lifetime-preserving borrowed views need no temporary owner.
    handle_methods.push(quote! {
      /// # Safety
      /// The receiver's dynamic class must be Self or a descendant of Self.
      #thunk_sig {
        let view: &Self = unsafe { #rt::view_unchecked(__this) };
        #thunk_body
      }
    });
    let self_token = &method
      .sig
      .receiver()
      .ok_or_else(|| Error::new_spanned(method, "missing receiver"))?
      .self_token;
    if *kind == Kind::Virtual {
      slots.push(quote! {
        /// # Safety
        /// Calling this slot requires a receiver compatible with the table's concrete class,
        /// not merely the static class of the table prefix.
        pub #method_name: #slot_type,
      });
      initializers.push(quote!(#method_name: Self::#thunk_name,));
      virtuals.push(VirtualMethod {
        name: method_name.clone(),
        is_async: method.sig.asyncness.is_some(),
      });
      let sig = &method.sig;
      let attrs = &method.attrs;
      handle_methods.push(quote! {
        #(#attrs)*
        pub #sig {
          unsafe { (#self_token.__handle.table().#method_name)(#self_token, #(#args),*) } #await_call
        }
      });
    } else {
      let index = inherited[&method_name.to_string()].0;
      let mut path = quote!(table);
      for _ in index..ancestors.len() {
        path = quote!(#path.base);
      }
      overrides.push(quote!(#path.#method_name = Self::#thunk_name;));
      let mut sig = method.sig.clone();
      sig.ident = format_ident!("super_{method_name}");
      let mut super_path = quote!(#self_token.__handle.table().parent);
      for _ in index + 1..ancestors.len() {
        super_path = quote!(#super_path.base);
      }
      handle_methods.push(quote! {
        #[inline]
        #sig {
          unsafe { (#super_path.#method_name)(#self_token, #(#args),*) } #await_call
        }
      });
    }
  }
  let storage_base = parent.as_ref().map(|p| quote!(base: <#p as #rt::Class>::Fields,));
  // Qualified associated types avoid trusting data/field names transported in descriptors.
  let data_base = parent.as_ref().map(|p| quote!(pub base: <#p as #rt::ClassData>::Data,));
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
      base: #rt::vtable::<#p>().clone(),
      parent: #rt::vtable::<#p>(),
    }
  });
  let require_open = parent.as_ref().map(|p| {
    quote! {
      fn require_open<C: #rt::OpenClass>() {}
      require_open::<#p>();
    }
  });
  let ancestry = parent.as_ref().map(|p| quote!(|| <#p as #rt::Class>::is_class(id)));
  let open_impl = opts.open.then(|| quote!(impl #ig #rt::OpenClass for #own #wc {}));
  let deref = parent.as_ref().map(|p| {
    quote! {
      impl #ig ::core::ops::Deref for #own #wc {
        type Target = #p;
        #[inline]
        fn deref(&self) -> &Self::Target {
          unsafe { #rt::view_unchecked(self) }
        }
      }
    }
  });
  let conversions = ancestors.iter().map(|a| &a.handle).chain([&own]).map(|target| {
    quote! {
      impl #ig From<&#own> for #target #wc {
        #[inline]
        fn from(value: &#own) -> Self {
          let view: &#target = value;
          view.clone()
        }
      }
    }
  });
  let getters = field_names
    .iter()
    .zip(&field_types)
    .zip(&field_vis)
    .map(|((field, ty), visibility)| {
      let setter = format_ident!("set_{field}");
      quote! {
        #[inline]
        #visibility fn #field(&self) -> #ty {
          self.__handle.fields().#field.read().unwrap_or_else(::std::sync::PoisonError::into_inner).clone()
        }
        #[inline]
        #visibility fn #setter(&self, value: #ty) {
          *self.__handle.fields().#field.write().unwrap_or_else(::std::sync::PoisonError::into_inner) = value;
        }
      }
    });
  let open = opts.open;
  let exported = quote_spanned! {name.span()=>
    { $crate::#name #tg; $crate::#fields_name #tg; $crate::#data_name #tg; $crate::#table_name #tg; #(#virtuals)* }
  };
  Ok(quote! {
    #[doc(hidden)]
    #[repr(C)]
    #vis struct #fields_name #full #wc {
      #storage_base
      #(#field_names: ::std::sync::RwLock<#field_types>,)*
      __marker: #marker,
    }
    #vis struct #data_name #full #wc {
      #data_base
      #(#field_vis #field_names: #field_types,)*
    }
    #[doc(hidden)]
    #[repr(C)]
    #[derive(Clone)]
    #vis struct #table_name #full #wc {
      #table_base
      #(#slots)*
      __marker: #marker,
    }
    #(#docs)*
    #[repr(transparent)]
    #vis struct #name #full #wc {
      __handle: #rt::Handle<Self>,
      __invariant: #marker,
    }
    impl #ig Clone for #own #wc {
      #[inline]
      fn clone(&self) -> Self {
        Self { __handle: self.__handle.clone(), __invariant: ::core::marker::PhantomData }
      }
    }
    impl #ig ::core::fmt::Debug for #own #wc {
      fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        ::core::fmt::Debug::fmt(&self.__handle, f)
      }
    }
    unsafe impl #ig #rt::Class for #own #wc {
      type Fields = #fields_name #tg;
      type VTable = #table_name #tg;
      #[inline]
      fn handle(&self) -> &#rt::Handle<Self> { &self.__handle }
      #[inline]
      fn from_handle(handle: #rt::Handle<Self>) -> Self {
        Self { __handle: handle, __invariant: ::core::marker::PhantomData }
      }
      fn is_class(id: ::core::any::TypeId) -> bool {
        id == ::core::any::TypeId::of::<Self>() #ancestry
      }
      fn build_vtable() -> Self::VTable {
        #require_open
        #[allow(unused_mut)]
        let mut table = #table_name {
          #table_init #(#initializers)* __marker: ::core::marker::PhantomData,
        };
        #(#overrides)*
        table
      }
    }
    impl #ig #rt::ClassData for #own #wc {
      type Data = #data_name #tg;
      fn build(data: Self::Data) -> <Self as #rt::Class>::Fields {
        #fields_name { #build_base #(#field_names: ::std::sync::RwLock::new(data.#field_names),)* __marker: ::core::marker::PhantomData }
      }
    }
    impl #ig #own #wc {
      #[inline]
      pub fn from_data(data: #data_name #tg) -> Self {
        <Self as #rt::Class>::from_handle(#rt::Handle::new(<Self as #rt::ClassData>::build(data)))
      }
      #[inline]
      pub fn is<__ClassTarget: #rt::Class>(&self) -> bool { self.__handle.is::<__ClassTarget>() }
      #[inline]
      pub fn downcast<__ClassTarget: #rt::Class>(&self) -> Option<__ClassTarget> { self.__handle.downcast::<__ClassTarget>() }
      #[inline]
      pub fn upcast<__ClassTarget>(&self) -> __ClassTarget where for<'a> __ClassTarget: From<&'a Self> { __ClassTarget::from(self) }
      pub fn ptr_eq<__ClassTarget: #rt::Class>(&self, other: &__ClassTarget) -> bool { self.__handle.ptr_eq(other.handle()) }
      #(#getters)* #(#handle_methods)*
    }
    #open_impl #deref #(#conversions)*
    #[doc(hidden)]
    #[macro_export]
    macro_rules! #descriptor_name {
      ($callback:path, $payload:tt) => {
        $callback! { $payload [#(#params)*] #open #(#ancestors)* #exported }
      };
    }
    #[doc(hidden)]
    #vis use #descriptor_name as #name;
  })
}
