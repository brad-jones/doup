//! Compile-time class descriptors.
//!
//! An open class exports a `macro_rules!` descriptor under its own name, in the macro namespace. A subclass
//! invokes its parent's descriptor with a callback (`__inherit`), which receives the parent's ancestry: its
//! runtime strategy, and for the parent and each of its ancestors, the names of their generated types, the
//! virtual methods they declare (with their visibility, and whether they're async or abstract) and the names
//! of the methods they override. They also carry the class's class trait (see `statics`), and the
//! signatures (but not the bodies) of the class-level members it declares, which subclasses repeat on
//! their own forwarders. Descriptors don't contain method bodies or instance method signatures, and only
//! exist while compiling.

use proc_macro2::{Punct, Spacing, TokenStream, TokenTree};
use quote::{ToTokens, quote};
use std::collections::HashMap;
use syn::{
  Attribute, Ident, ItemImpl, ItemStruct, LitBool, Path, Result, Signature, Token, Type, Visibility, braced, bracketed,
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

/// The signature of a class-level member, see `statics`.
#[derive(Clone)]
pub enum StaticSig {
  Fn(Signature),
  Const(Ident, Type),
}

/// A class-level member a class declares: a virtual or abstract associated function or constant.
#[derive(Clone)]
pub struct StaticMember {
  pub vis: Visibility,
  pub is_abstract: bool,
  /// Its doc comments, which document its forwarders.
  pub docs: Vec<Attribute>,
  pub sig: StaticSig,
}

impl StaticMember {
  pub fn name(&self) -> &Ident {
    match &self.sig {
      StaticSig::Fn(sig) => &sig.ident,
      StaticSig::Const(ident, _) => ident,
    }
  }

  /// The member as written in a descriptor, its signature anchored to the declaring crate, see [`anchor`].
  pub fn to_anchored_tokens(&self) -> TokenStream {
    let Self {
      vis,
      is_abstract,
      docs,
      sig,
    } = self;
    match sig {
      StaticSig::Fn(sig) => {
        let sig = anchor(sig.to_token_stream());
        quote! { fn [#vis] #is_abstract [#(#docs)*] #sig; }
      }
      StaticSig::Const(ident, ty) => {
        let ty = anchor(ty.to_token_stream());
        quote! { const [#vis] #is_abstract [#(#docs)*] #ident: #ty; }
      }
    }
  }
}

impl Parse for StaticMember {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let is_fn = input.peek(Token![fn]);
    if is_fn {
      input.parse::<Token![fn]>()?;
    } else {
      input.parse::<Token![const]>()?;
    }
    let vis;
    bracketed!(vis in input);
    let vis = vis.parse()?;
    let is_abstract = input.parse::<LitBool>()?.value;
    let docs;
    bracketed!(docs in input);
    let docs = docs.call(Attribute::parse_outer)?;
    let sig = if is_fn {
      StaticSig::Fn(input.parse()?)
    } else {
      let ident = input.parse()?;
      input.parse::<Token![:]>()?;
      StaticSig::Const(ident, input.parse()?)
    };
    input.parse::<Token![;]>()?;
    Ok(Self {
      vis,
      is_abstract,
      docs,
      sig,
    })
  }
}

impl ToTokens for StaticMember {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let Self {
      vis,
      is_abstract,
      docs,
      sig,
    } = self;
    tokens.extend(match sig {
      StaticSig::Fn(sig) => quote! { fn [#vis] #is_abstract [#(#docs)*] #sig; },
      StaticSig::Const(ident, ty) => quote! { const [#vis] #is_abstract [#(#docs)*] #ident: #ty; },
    });
  }
}

/// Replaces `crate` and `self` in a signature with `$crate`, so that the paths an open class (which is at
/// its crate's root) writes relative to its crate still name the same items in a subclass elsewhere.
pub fn anchor(tokens: TokenStream) -> TokenStream {
  tokens
    .into_iter()
    .flat_map(|tree| -> Vec<TokenTree> {
      match tree {
        TokenTree::Ident(ident) if ident == "crate" || ident == "self" => {
          let mut dollar = Punct::new('$', Spacing::Alone);
          dollar.set_span(ident.span());
          vec![dollar.into(), Ident::new("crate", ident.span()).into()]
        }
        TokenTree::Group(group) => {
          let mut anchored = proc_macro2::Group::new(group.delimiter(), anchor(group.stream()));
          anchored.set_span(group.span());
          vec![anchored.into()]
        }
        other => vec![other],
      }
    })
    .collect()
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
  /// The class's class trait, if it has one, see `statics`.
  pub class_trait: Option<Path>,
  /// The class-level members the class declares (not those it inherits).
  pub statics: Vec<StaticMember>,
  /// The inherited class-level members the class overrides.
  pub static_overrides: Vec<Ident>,
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
    let methods = parse_list(&content)?;
    let overrides = parse_list(&content)?;
    let class_trait = {
      let path;
      bracketed!(path in content);
      if path.is_empty() { None } else { Some(path.parse()?) }
    };
    let statics = parse_list(&content)?;
    let static_overrides = parse_list(&content)?;
    Ok(Self {
      handle,
      fields,
      data,
      dispatch,
      methods,
      overrides,
      class_trait,
      statics,
      static_overrides,
    })
  }
}

/// Parses a bracketed list of items, e.g. `[a b c]`.
fn parse_list<T: Parse>(input: ParseStream<'_>) -> Result<Vec<T>> {
  let content;
  bracketed!(content in input);
  let mut items = Vec::new();
  while !content.is_empty() {
    items.push(content.parse()?);
  }
  Ok(items)
}

impl ToTokens for Ancestor {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let Self {
      handle,
      fields,
      data,
      dispatch,
      methods,
      overrides,
      class_trait,
      statics,
      static_overrides,
    } = self;
    tokens.extend(quote! {
      { #handle; #fields; #data; #dispatch; [#(#methods)*] [#(#overrides)*]
        [#class_trait] [#(#statics)*] [#(#static_overrides)*] }
    });
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
    if let Some(path) = ancestor.class_trait.take() {
      ancestor.class_trait = Some(self.fold_path(path));
    }
    for member in &mut ancestor.statics {
      member.sig = match member.sig.clone() {
        StaticSig::Fn(sig) => StaticSig::Fn(self.fold_signature(sig)),
        StaticSig::Const(ident, ty) => StaticSig::Const(ident, self.fold_type(ty)),
      };
    }
  }
}
