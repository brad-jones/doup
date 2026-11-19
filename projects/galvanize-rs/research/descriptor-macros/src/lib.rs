//! Research-only `#[class]` and `#[class_methods]` attributes.
//! Descriptors transport ancestor types and virtual method names, not method bodies or runtime state.
#![forbid(unsafe_code)]

use proc_macro::TokenStream;
use proc_macro2::TokenStream as Tokens;
use quote::{ToTokens, format_ident, quote, quote_spanned};
use std::collections::{HashMap, HashSet};
use syn::{
  Error, Fields, FnArg, GenericArgument, GenericParam, Generics, Ident, ImplItem, ImplItemFn, ItemImpl, ItemStruct,
  LitBool, Pat, PathArguments, Result, Token, Type, braced, bracketed,
  fold::{self, Fold},
  parse::{Parse, ParseStream, Parser},
  parse_quote,
};

#[derive(Default)]
struct Options {
  open: bool,
  parent: Option<Type>,
}

fn options(tokens: Tokens) -> Result<Options> {
  let mut result = Options::default();
  syn::meta::parser(|meta| {
    if meta.path.is_ident("open") && !result.open {
      result.open = true;
      Ok(())
    } else if meta.path.is_ident("extends") && result.parent.is_none() {
      result.parent = Some(meta.value()?.parse()?);
      Ok(())
    } else {
      Err(meta.error("expected non-repeated `open` or `extends = Parent`"))
    }
  })
  .parse2(tokens)?;
  Ok(result)
}

fn finish(result: Result<Tokens>) -> TokenStream {
  result.unwrap_or_else(Error::into_compile_error).into()
}

/// Records a named struct until its following `#[class_methods]` block supplies its methods.
///
/// This spike exports descriptors for classes declared at a crate's root.
#[proc_macro_attribute]
pub fn class(attr: TokenStream, item: TokenStream) -> TokenStream {
  finish((|| {
    let attr: Tokens = attr.into();
    options(attr.clone())?;
    let item: ItemStruct = syn::parse(item)?;
    if !matches!(item.fields, Fields::Named(_)) {
      return Err(Error::new_spanned(&item, "the descriptor spike requires named fields"));
    }
    let helper = format_ident!("__descriptor_fields_{}", item.ident);
    Ok(quote! {
      macro_rules! #helper {
        ($methods:item) => {
          ::galvanize_rs_descriptor_macros::__complete! { [#attr] #item $methods }
        };
      }
    })
  })())
}

/// Completes a preceding `#[class]` declaration and publishes its ancestry descriptor.
#[proc_macro_attribute]
pub fn class_methods(attr: TokenStream, item: TokenStream) -> TokenStream {
  finish((|| {
    if !attr.is_empty() {
      return Err(Error::new(
        proc_macro2::Span::call_site(),
        "class_methods takes no arguments",
      ));
    }
    let item: ItemImpl = syn::parse(item)?;
    let Type::Path(path) = &*item.self_ty else {
      return Err(Error::new_spanned(&item.self_ty, "expected an inherent class impl"));
    };
    let name = &path
      .path
      .segments
      .last()
      .ok_or_else(|| Error::new_spanned(path, "expected class"))?
      .ident;
    let helper = format_ident!("__descriptor_fields_{name}");
    Ok(quote! { #helper! { #item } })
  })())
}

struct Input {
  attr: Tokens,
  item: ItemStruct,
  methods: ItemImpl,
}

impl Parse for Input {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let attr;
    bracketed!(attr in input);
    Ok(Self {
      attr: attr.parse()?,
      item: input.parse()?,
      methods: input.parse()?,
    })
  }
}

#[derive(Clone)]
struct Ancestor {
  handle: Type,
  fields: Type,
  data: Type,
  object: Type,
  methods: Vec<Ident>,
}

impl Parse for Ancestor {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let content;
    braced!(content in input);
    let handle = content.parse()?;
    content.parse::<Token![;]>()?;
    let fields = content.parse()?;
    content.parse::<Token![;]>()?;
    let data = content.parse()?;
    content.parse::<Token![;]>()?;
    let object = content.parse()?;
    content.parse::<Token![;]>()?;
    let mut methods = Vec::new();
    while !content.is_empty() {
      methods.push(content.parse()?);
    }
    Ok(Self {
      handle,
      fields,
      data,
      object,
      methods,
    })
  }
}

impl ToTokens for Ancestor {
  fn to_tokens(&self, tokens: &mut Tokens) {
    let Self {
      handle,
      fields,
      data,
      object,
      methods,
    } = self;
    tokens.extend(quote! { { #handle; #fields; #data; #object; #(#methods)* } });
  }
}

struct Inherited {
  input: Input,
  params: Vec<Ident>,
  open: bool,
  ancestors: Vec<Ancestor>,
}

impl Parse for Inherited {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let payload;
    braced!(payload in input);
    let params;
    bracketed!(params in input);
    let mut names = Vec::new();
    while !params.is_empty() {
      names.push(params.parse()?);
    }
    let open: LitBool = input.parse()?;
    let mut ancestors = Vec::new();
    while !input.is_empty() {
      ancestors.push(input.parse()?);
    }
    Ok(Self {
      input: payload.parse()?,
      params: names,
      open: open.value,
      ancestors,
    })
  }
}

#[doc(hidden)]
#[proc_macro]
pub fn __complete(input: TokenStream) -> TokenStream {
  finish((|| {
    let input: Input = syn::parse(input)?;
    let opts = options(input.attr.clone())?;
    if let Some(Type::Path(parent)) = opts.parent {
      let mut descriptor = parent.path;
      for segment in &mut descriptor.segments {
        segment.arguments = PathArguments::None;
      }
      let Input { attr, item, methods } = input;
      Ok(quote! {
        #descriptor! { ::galvanize_rs_descriptor_macros::__inherit, { [#attr] #item #methods } }
      })
    } else if opts.parent.is_some() {
      Err(Error::new_spanned(&input.item, "extends must name a class path"))
    } else {
      generate(input, Vec::new())
    }
  })())
}

struct Substitute(HashMap<String, Type>);

impl Fold for Substitute {
  fn fold_type(&mut self, ty: Type) -> Type {
    if let Type::Path(path) = &ty
      && path.qself.is_none()
      && let Some(name) = path.path.get_ident()
      && let Some(replacement) = self.0.get(&name.to_string())
    {
      return replacement.clone();
    }
    fold::fold_type(self, ty)
  }
}

#[doc(hidden)]
#[proc_macro]
pub fn __inherit(input: TokenStream) -> TokenStream {
  finish((|| {
    let mut inherited: Inherited = syn::parse(input)?;
    let parent = options(inherited.input.attr.clone())?
      .parent
      .ok_or_else(|| Error::new_spanned(&inherited.input.item, "missing parent"))?;
    if !inherited.open {
      return Err(Error::new_spanned(parent, "cannot extend a class without `open`"));
    }
    let Type::Path(path) = &parent else {
      return Err(Error::new_spanned(parent, "expected parent path"));
    };
    let segment = path
      .path
      .segments
      .last()
      .ok_or_else(|| Error::new_spanned(path, "expected parent"))?;
    let arguments: Vec<Type> = match &segment.arguments {
      PathArguments::None => Vec::new(),
      PathArguments::AngleBracketed(args) => args
        .args
        .iter()
        .map(|arg| match arg {
          GenericArgument::Type(ty) => Ok(ty.clone()),
          _ => Err(Error::new_spanned(arg, "the spike supports type arguments only")),
        })
        .collect::<Result<_>>()?,
      other => return Err(Error::new_spanned(other, "expected type arguments")),
    };
    if inherited.params.len() != arguments.len() {
      return Err(Error::new_spanned(
        parent,
        "supply every parent type argument explicitly",
      ));
    }
    let mut substitute = Substitute(
      inherited
        .params
        .iter()
        .map(ToString::to_string)
        .zip(arguments)
        .collect(),
    );
    for ancestor in &mut inherited.ancestors {
      ancestor.handle = substitute.fold_type(ancestor.handle.clone());
      ancestor.fields = substitute.fold_type(ancestor.fields.clone());
      ancestor.data = substitute.fold_type(ancestor.data.clone());
      ancestor.object = substitute.fold_type(ancestor.object.clone());
    }
    if inherited.ancestors.is_empty() {
      return Err(Error::new_spanned(parent, "empty ancestor descriptor"));
    }
    generate(inherited.input, inherited.ancestors)
  })())
}

fn generics(input: &Generics) -> Result<Generics> {
  let mut result = input.clone();
  for param in &mut result.params {
    let GenericParam::Type(param) = param else {
      return Err(Error::new_spanned(
        param,
        "the spike supports type parameters, not lifetimes or const generics",
      ));
    };
    if param.ident.to_string().starts_with("__") {
      return Err(Error::new_spanned(
        param,
        "type parameter names beginning with __ are reserved",
      ));
    }
    let bounds: [syn::TypeParamBound; 4] = [
      parse_quote!(::core::clone::Clone),
      parse_quote!(::core::marker::Send),
      parse_quote!(::core::marker::Sync),
      parse_quote!('static),
    ];
    param.bounds.extend(bounds);
  }
  Ok(result)
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
  Ordinary,
  Virtual,
  Override,
}

fn method_kind(method: &mut ImplItemFn) -> Result<Kind> {
  let mut kind = Kind::Ordinary;
  let mut retained = Vec::new();
  for attr in &method.attrs {
    let next = if attr.path().is_ident("class_virtual") {
      Kind::Virtual
    } else if attr.path().is_ident("class_override") {
      Kind::Override
    } else {
      retained.push(attr.clone());
      continue;
    };
    if kind != Kind::Ordinary || !matches!(attr.meta, syn::Meta::Path(_)) {
      return Err(Error::new_spanned(
        attr,
        "use one bare #[class_virtual] or #[class_override]",
      ));
    }
    kind = next;
  }
  method.attrs = retained;
  if !method.attrs.iter().any(|attr| attr.path().is_ident("inline")) {
    method.attrs.push(parse_quote!(#[inline]));
  }
  if kind != Kind::Ordinary {
    let sig = &method.sig;
    let receiver_ok = sig
      .receiver()
      .is_some_and(|r| r.reference.is_some() && r.mutability.is_none() && r.colon_token.is_none());
    if !receiver_ok
      || sig.asyncness.is_some()
      || sig.unsafety.is_some()
      || sig.constness.is_some()
      || sig.abi.is_some()
      || !sig.generics.params.is_empty()
      || sig.generics.where_clause.is_some()
    {
      return Err(Error::new_spanned(
        sig,
        "virtual methods require a synchronous, safe, non-generic `&self` signature",
      ));
    }
    if !matches!(method.vis, syn::Visibility::Public(_)) {
      return Err(Error::new_spanned(
        &method.vis,
        "virtual methods and overrides must be public in this spike",
      ));
    }
    arguments(method)?;
  }
  Ok(kind)
}

fn arguments(method: &ImplItemFn) -> Result<Vec<Ident>> {
  method
    .sig
    .inputs
    .iter()
    .skip(1)
    .map(|arg| {
      let FnArg::Typed(arg) = arg else {
        return Err(Error::new_spanned(arg, "expected named parameter"));
      };
      let Pat::Ident(pattern) = &*arg.pat else {
        return Err(Error::new_spanned(arg, "method parameters must be identifiers"));
      };
      if pattern.by_ref.is_some() || pattern.subpat.is_some() || pattern.ident == "__this" {
        return Err(Error::new_spanned(pattern, "unsupported or reserved parameter pattern"));
      }
      Ok(pattern.ident.clone())
    })
    .collect()
}

fn generate(input: Input, ancestors: Vec<Ancestor>) -> Result<Tokens> {
  let opts = options(input.attr)?;
  let item = input.item;
  let name = &item.ident;
  let vis = &item.vis;
  let fields_name = format_ident!("__{name}Fields");
  let object_name = format_ident!("__{name}Object");
  let data_name = format_ident!("{name}Data");
  let descriptor_name = format_ident!("__descriptor_export_{name}");
  let full = generics(&item.generics)?;
  let (impl_generics, ty_generics, where_clause) = full.split_for_impl();
  let own_handle: Type = parse_quote!(#name #ty_generics);
  let own_fields: Type = parse_quote!(#fields_name #ty_generics);
  let own_data: Type = parse_quote!(#data_name #ty_generics);
  let own_object: Type = parse_quote!(#object_name #ty_generics);
  let parent = ancestors.last();

  if input.methods.trait_.is_some()
    || input.methods.unsafety.is_some()
    || input.methods.self_ty.to_token_stream().to_string() != own_handle.to_token_stream().to_string()
    || !input
      .methods
      .generics
      .type_params()
      .map(|p| &p.ident)
      .eq(item.generics.type_params().map(|p| &p.ident))
  {
    return Err(Error::new_spanned(
      &input.methods,
      "expected an inherent impl of this class with its declared type parameters",
    ));
  }
  if input.methods.generics.where_clause.is_some()
    || input
      .methods
      .generics
      .params
      .iter()
      .any(|p| !matches!(p, GenericParam::Type(t) if t.bounds.is_empty() && t.default.is_none()))
  {
    return Err(Error::new_spanned(
      &input.methods.generics,
      "put generic bounds on the class struct in this spike",
    ));
  }
  for attr in &item.attrs {
    if !attr.path().is_ident("doc") {
      return Err(Error::new_spanned(
        attr,
        "the spike supports only doc attributes on class structs",
      ));
    }
  }

  let mut inherited_methods = HashMap::new();
  for (index, ancestor) in ancestors.iter().enumerate() {
    for method in &ancestor.methods {
      inherited_methods.insert(method.to_string(), index);
    }
  }
  let mut generated_names: HashSet<String> = ["from_data", "is", "downcast", "upcast", "ptr_eq"]
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
      || !generated_names.insert(ident.to_string())
      || !generated_names.insert(format!("set_{ident}"))
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
    let kind = method_kind(&mut method)?;
    let label = method.sig.ident.to_string();
    if label.starts_with("__") || generated_names.contains(&label) || !generated_names.insert(label.clone()) {
      return Err(Error::new_spanned(
        &method.sig.ident,
        "method conflicts with a generated member",
      ));
    }
    match kind {
      Kind::Virtual if inherited_methods.contains_key(&label) => {
        return Err(Error::new_spanned(
          &method.sig.ident,
          "inherited virtual already exists; use #[class_override]",
        ));
      }
      Kind::Override if !inherited_methods.contains_key(&label) => {
        return Err(Error::new_spanned(
          &method.sig.ident,
          "no ancestor declares this virtual method",
        ));
      }
      _ => {}
    }
    methods.push((kind, method));
  }
  for (kind, method) in &methods {
    if *kind == Kind::Override && generated_names.contains(&format!("super_{}", method.sig.ident)) {
      return Err(Error::new_spanned(
        &method.sig.ident,
        "super method conflicts with a user method",
      ));
    }
  }

  let own_virtuals: Vec<_> = methods
    .iter()
    .filter(|(kind, _)| *kind == Kind::Virtual)
    .map(|(_, method)| method.sig.ident.clone())
    .collect();
  let own = Ancestor {
    handle: own_handle.clone(),
    fields: own_fields.clone(),
    data: own_data,
    object: own_object.clone(),
    methods: own_virtuals.clone(),
  };
  let chain: Vec<_> = ancestors.iter().cloned().chain([own]).collect();
  let root = &chain[0].object;
  let parent_bound = parent.map(|p| &p.object);
  let supertraits = parent_bound.map_or_else(
    || quote!(::core::any::Any + ::core::marker::Send + ::core::marker::Sync),
    |p| quote!(#p),
  );
  let root_members = parent.is_none().then(|| {
    quote! {
      fn __is(&self, target: ::core::any::TypeId) -> bool;
      fn __cast(self: ::std::sync::Arc<Self>, out: &mut dyn ::core::any::Any);
    }
  });
  let trait_methods = methods
    .iter()
    .filter(|(kind, _)| *kind == Kind::Virtual)
    .map(|(_, method)| {
      let method_name = &method.sig.ident;
      let body_name = format_ident!("__body_{method_name}");
      let args = arguments(method)?;
      let params = method.sig.inputs.iter().skip(1);
      let output = &method.sig.output;
      Ok(quote! {
        fn #method_name(&self, __this: &#own_handle, #(#params),*) #output {
          match <Self as #own_object>::__parent(self) {
            Some(parent) => parent.#method_name(__this, #(#args),*),
            None => __this.#body_name(#(#args),*),
          }
        }
      })
    })
    .collect::<Result<Vec<_>>>()?;

  let object_impls = chain
    .iter()
    .enumerate()
    .map(|(index, ancestor)| {
      let trait_ty = &ancestor.object;
      let field_ty = &ancestor.fields;
      let parent_value = if index < ancestors.len() {
        quote!(Some(&self.base))
      } else {
        quote!(None)
      };
      let mut field_path = quote!(self);
      for _ in index..ancestors.len() {
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
              if let Some(slot) = out.downcast_mut::<Option<#handles>>() {
                *slot = Some(<#handles>::__from_obj(self));
                return;
              }
            )*
            panic!("descriptor cast requested a type outside the hierarchy");
          }
        }
      });
      let overrides = methods
        .iter()
        .filter(|(kind, method)| {
          *kind == Kind::Override && inherited_methods.get(&method.sig.ident.to_string()) == Some(&index)
        })
        .map(|(_, method)| {
          let method_name = &method.sig.ident;
          let body_name = format_ident!("__body_{method_name}");
          let receiver = &ancestor.handle;
          let params = method.sig.inputs.iter().skip(1);
          let output = &method.sig.output;
          let args = arguments(method)?;
          Ok(quote! {
            #[inline]
            fn #method_name(&self, __this: &#receiver, #(#params),*) #output {
              __this.downcast::<#own_handle>()
                .expect("override receiver must belong to its declaring class")
                .#body_name(#(#args),*)
            }
          })
        })
        .collect::<Result<Vec<_>>>()?;
      Ok(quote! {
        impl #impl_generics #trait_ty for #own_fields #where_clause {
          fn __parent(&self) -> Option<&dyn #trait_ty> { #parent_value }
          fn __fields(&self) -> &#field_ty { &#field_path }
          #root_impl
          #(#overrides)*
        }
      })
    })
    .collect::<Result<Vec<_>>>()?;

  let mut handle_methods = Vec::new();
  for (kind, method) in &methods {
    if *kind == Kind::Ordinary {
      handle_methods.push(quote!(#method));
      continue;
    }
    let mut body = method.clone();
    let method_name = &method.sig.ident;
    body.sig.ident = format_ident!("__body_{method_name}");
    body.vis = syn::Visibility::Inherited;
    handle_methods.push(quote!(#body));
    let args = arguments(method)?;
    let receiver = &method
      .sig
      .receiver()
      .ok_or_else(|| Error::new_spanned(method, "missing receiver"))?
      .self_token;
    if *kind == Kind::Virtual {
      let sig = &method.sig;
      let attrs = &method.attrs;
      handle_methods.push(quote! {
        #(#attrs)*
        pub #sig { <_ as #own_object>::#method_name(#receiver.__obj.as_ref(), #receiver, #(#args),*) }
      });
    } else {
      let mut sig = method.sig.clone();
      sig.ident = format_ident!("super_{method_name}");
      let ancestor = &ancestors[inherited_methods[&method_name.to_string()]];
      let declaring_trait = &ancestor.object;
      let parent_fields = &parent
        .ok_or_else(|| Error::new_spanned(method, "override requires parent"))?
        .fields;
      handle_methods.push(quote! {
        #sig {
          <#parent_fields as #declaring_trait>::#method_name(
            &<_ as #own_object>::__fields(#receiver.__obj.as_ref()).base, #receiver, #(#args),*
          )
        }
      });
    }
  }
  let getters = field_names
    .iter()
    .zip(&field_types)
    .zip(&field_vis)
    .map(|((field, ty), visibility)| {
      let setter = format_ident!("set_{field}");
      quote! {
        #[doc = concat!("Returns a clone of `", stringify!(#field), "`.")]
        #[inline]
        #visibility fn #field(&self) -> #ty {
          <_ as #own_object>::__fields(self.__obj.as_ref()).#field.read()
            .unwrap_or_else(::std::sync::PoisonError::into_inner).clone()
        }
        #[doc = concat!("Replaces `", stringify!(#field), "`.")]
        #[inline]
        #visibility fn #setter(&self, value: #ty) {
          *<_ as #own_object>::__fields(self.__obj.as_ref()).#field.write()
            .unwrap_or_else(::std::sync::PoisonError::into_inner) = value;
        }
      }
    });
  let obj_type = if opts.open {
    quote!(dyn #own_object)
  } else {
    quote!(#own_fields)
  };
  let parent_handle = parent.map(|a| &a.handle);
  let parent_fields = parent.map(|a| &a.fields);
  let data_base = parent.map(|p| {
    let data = &p.data;
    quote!(pub base: #data,)
  });
  let storage_base = parent_fields.map(|p| quote!(pub base: #p,));
  let handle_base = parent_handle.map(|p| quote!(__base: #p,));
  let init_base = parent_handle.map(|p| quote!(base: <#p>::__build(data.base),));
  let from_base = parent_handle.map(|p| quote!(__base: <#p>::__from_obj(obj.clone()),));
  let clone_base = parent_handle.map(|_| quote!(__base: self.__base.clone(),));
  let deref = parent_handle.map(|p| {
    quote! {
      impl #impl_generics ::core::ops::Deref for #own_handle #where_clause {
        type Target = #p;
        fn deref(&self) -> &Self::Target { &self.__base }
      }
    }
  });
  let conversions = chain.iter().map(|a| {
    let target = &a.handle;
    quote! {
      impl #impl_generics From<&#own_handle> for #target #where_clause {
        fn from(value: &#own_handle) -> Self {
          let view: &#target = value;
          view.clone()
        }
      }
    }
  });
  let params: Vec<_> = full.type_params().map(|p| &p.ident).collect();
  let open = opts.open;
  let docs = &item.attrs;
  // A callback may originate in the parent's crate. Anchor this class's $crate to its own declaration.
  let exported_own = quote_spanned! {name.span()=> { $crate::#name #ty_generics; $crate::#fields_name #ty_generics; $crate::#data_name #ty_generics; $crate::#object_name #ty_generics; #(#own_virtuals)* } };
  Ok(quote! {
    #[doc(hidden)]
    #vis struct #fields_name #full #where_clause {
      #storage_base
      #(#field_names: ::std::sync::RwLock<#field_types>,)*
    }
    #[doc = concat!("Initial data for [`", stringify!(#name), "`].")]
    #vis struct #data_name #full #where_clause {
      #data_base
      #(#field_vis #field_names: #field_types,)*
    }
    #[doc(hidden)]
    #vis trait #object_name #full: #supertraits #where_clause {
      fn __parent(&self) -> Option<&dyn #own_object>;
      fn __fields(&self) -> &#own_fields;
      #root_members
      #(#trait_methods)*
    }
    #(#object_impls)*
    #(#docs)*
    #vis struct #name #full #where_clause {
      __obj: ::std::sync::Arc<#obj_type>,
      #handle_base
    }
    impl #impl_generics #own_handle #where_clause {
      #[doc(hidden)]
      #[inline]
      pub fn __from_obj(obj: ::std::sync::Arc<#obj_type>) -> Self {
        Self { #from_base __obj: obj }
      }
      #[doc(hidden)]
      pub fn __build(data: #data_name #ty_generics) -> #own_fields {
        #fields_name { #init_base #(#field_names: ::std::sync::RwLock::new(data.#field_names),)* }
      }
      /// Constructs one shared hierarchy allocation from nested initial data.
      #[inline]
      pub fn from_data(data: #data_name #ty_generics) -> Self {
        Self::__from_obj(::std::sync::Arc::new(Self::__build(data)))
      }
      /// Tests the dynamic class or one of its ancestors.
      #[inline]
      pub fn is<__ClassTarget: 'static>(&self) -> bool {
        <_ as #root>::__is(self.__obj.as_ref(), ::core::any::TypeId::of::<__ClassTarget>())
      }
      /// Returns an owning view when the dynamic class is compatible.
      #[inline]
      pub fn downcast<__ClassTarget: 'static>(&self) -> Option<__ClassTarget> {
        if !self.is::<__ClassTarget>() { return None; }
        let mut result = None::<__ClassTarget>;
        <_ as #root>::__cast(self.__obj.clone(), &mut result);
        result
      }
      /// Clones a view of this class or any ancestor, selected by the result type.
      #[inline]
      pub fn upcast<__ClassTarget>(&self) -> __ClassTarget where for<'a> __ClassTarget: From<&'a Self> { __ClassTarget::from(self) }
      /// Compares allocation identity, ignoring trait-object metadata.
      pub fn ptr_eq(&self, other: &Self) -> bool {
        ::core::ptr::addr_eq(::std::sync::Arc::as_ptr(&self.__obj), ::std::sync::Arc::as_ptr(&other.__obj))
      }
      #(#getters)*
      #(#handle_methods)*
    }
    impl #impl_generics Clone for #own_handle #where_clause {
      #[inline]
      fn clone(&self) -> Self { Self { __obj: self.__obj.clone(), #clone_base } }
    }
    impl #impl_generics ::core::fmt::Debug for #own_handle #where_clause {
      fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct(stringify!(#name)).field("identity", &::std::sync::Arc::as_ptr(&self.__obj)).finish()
      }
    }
    #deref
    #(#conversions)*
    #[doc(hidden)]
    #[macro_export]
    macro_rules! #descriptor_name {
      ($callback:path, $payload:tt) => {
        $callback! { $payload [#(#params)*] #open #(#ancestors)* #exported_own }
      };
    }
    #[doc(hidden)]
    #vis use #descriptor_name as #name;
  })
}
