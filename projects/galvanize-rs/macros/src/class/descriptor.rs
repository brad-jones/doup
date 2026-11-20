//! Compile-time class descriptors.
//!
//! An open class exports a `macro_rules!` descriptor under its own name, in the macro namespace. A subclass
//! invokes its parent's descriptor with a callback (`__inherit`), which receives the parent's ancestry: its
//! runtime strategy, and for the parent and each of its ancestors, the names of their generated types, the
//! virtual methods they declare (with their visibility, and whether they're async or abstract) and the names
//! of the methods they override. Descriptors don't contain method bodies or signatures, and only exist while
//! compiling.

use quote::{ToTokens, quote};
use std::collections::HashMap;
use syn::{
  Ident, ItemImpl, ItemStruct, LitBool, Result, Token, Type, Visibility, braced, bracketed,
  fold::{self, Fold},
  parse::{Parse, ParseStream},
};

/// A `#[class]` struct and its `#[class_methods]` block, as passed between the macros.
pub struct Input {
  pub attr: proc_macro2::TokenStream,
  pub item: ItemStruct,
  pub methods: ItemImpl,
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

impl ToTokens for Input {
  fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
    let Self { attr, item, methods } = self;
    tokens.extend(quote! { [#attr] #item #methods });
  }
}

#[derive(Clone)]
pub struct VirtualMethod {
  pub name: Ident,
  /// The visibility it was declared with, which overrides must repeat.
  pub vis: Visibility,
  pub is_async: bool,
  pub is_abstract: bool,
}

impl Parse for VirtualMethod {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let name = input.parse()?;
    let vis;
    bracketed!(vis in input);
    let vis = vis.parse()?;
    let is_async: LitBool = input.parse()?;
    let is_abstract: LitBool = input.parse()?;
    input.parse::<Token![;]>()?;
    Ok(Self {
      name,
      vis,
      is_async: is_async.value,
      is_abstract: is_abstract.value,
    })
  }
}

impl ToTokens for VirtualMethod {
  fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
    let Self {
      name,
      vis,
      is_async,
      is_abstract,
    } = self;
    tokens.extend(quote! { #name [#vis] #is_async #is_abstract; });
  }
}

/// One class in a descriptor's ancestry.
#[derive(Clone)]
pub struct Ancestor {
  /// The class handle, e.g. `Animal`.
  pub handle: Type,
  /// The hidden storage type.
  pub fields: Type,
  /// The `XData` type.
  pub data: Type,
  /// The safe runtime's object trait, or the unsafe runtime's vtable type.
  pub dispatch: Type,
  /// The virtual methods the class declares (not those it inherits).
  pub methods: Vec<VirtualMethod>,
  /// The inherited virtual methods the class overrides.
  pub overrides: Vec<Ident>,
}

impl Parse for Ancestor {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let content;
    braced!(content in input);
    let ty = || -> Result<Type> {
      let ty = content.parse()?;
      content.parse::<Token![;]>()?;
      Ok(ty)
    };
    let (handle, fields, data, dispatch) = (ty()?, ty()?, ty()?, ty()?);
    let virtuals;
    bracketed!(virtuals in content);
    let mut methods = Vec::new();
    while !virtuals.is_empty() {
      methods.push(virtuals.parse()?);
    }
    let overridden;
    bracketed!(overridden in content);
    let mut overrides = Vec::new();
    while !overridden.is_empty() {
      overrides.push(overridden.parse()?);
    }
    Ok(Self {
      handle,
      fields,
      data,
      dispatch,
      methods,
      overrides,
    })
  }
}

impl ToTokens for Ancestor {
  fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
    let Self {
      handle,
      fields,
      data,
      dispatch,
      methods,
      overrides,
    } = self;
    tokens.extend(quote! { { #handle; #fields; #data; #dispatch; [#(#methods)*] [#(#overrides)*] } });
  }
}

/// The name of a class, as written in errors, from its handle type (which may start with `$crate`).
pub fn class_name(handle: &Type) -> String {
  match handle {
    Type::Path(path) => path
      .path
      .segments
      .last()
      .map_or_else(|| handle.to_token_stream().to_string(), |s| s.ident.to_string()),
    other => other.to_token_stream().to_string(),
  }
}

/// What a parent's descriptor passes to `__inherit`.
pub struct Inheritance {
  pub input: Input,
  /// The parent's type parameters, which its ancestry is written in terms of.
  pub params: Vec<Ident>,
  pub is_unsafe: bool,
  /// The root first, the parent last.
  pub ancestors: Vec<Ancestor>,
}

impl Parse for Inheritance {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let payload;
    braced!(payload in input);
    let params;
    bracketed!(params in input);
    let mut names = Vec::new();
    while !params.is_empty() {
      names.push(params.parse()?);
    }
    let is_unsafe: LitBool = input.parse()?;
    let mut ancestors = Vec::new();
    while !input.is_empty() {
      ancestors.push(input.parse()?);
    }
    Ok(Self {
      input: payload.parse()?,
      params: names,
      is_unsafe: is_unsafe.value,
      ancestors,
    })
  }
}

/// Replaces a parent's type parameters with the arguments the subclass gives them.
pub struct Substitute(pub HashMap<String, Type>);

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

impl Substitute {
  pub fn ancestor(&mut self, ancestor: &mut Ancestor) {
    for ty in [
      &mut ancestor.handle,
      &mut ancestor.fields,
      &mut ancestor.data,
      &mut ancestor.dispatch,
    ] {
      *ty = self.fold_type(ty.clone());
    }
  }
}
