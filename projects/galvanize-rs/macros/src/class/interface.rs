//! `implements` and `#[class_interface]`.
//!
//! A proc macro can't see a trait's methods, so `#[class_interface]` records their names in a descriptor:
//! a `macro_rules!` re-exported under the trait's name, in the macro namespace, so it travels with the
//! trait wherever it's imported.
//!
//! A class generates its trait impls in two nested blocks. The outer block imports `__no_interface` under
//! an alias for each trait, and the inner block imports the trait's path under the same alias, which
//! shadows the fallback in the macro namespace only when the trait has a descriptor. Invoking the alias
//! calls `__implement`, with the trait's method names or without them, which chains through the class's
//! traits and generates their impls once it knows about all of them.

use super::Model;
use super::methods::{Kind, Method};
use proc_macro2::{Group, Literal, TokenStream};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use syn::{
  Error, FnArg, GenericParam, Ident, ItemTrait, LitBool, Pat, Path, PathArguments, Result, Token, TraitItem, Type,
  Visibility, braced, bracketed,
  fold::{self, Fold},
  parse::{Parse, ParseStream, Parser},
  parse_quote,
  spanned::Spanned,
};

/// The traits that share their name with one of std's derive macros, which a class can't look up a
/// descriptor for, as the macro would be found instead.
const DERIVE_NAMES: &[&str] = &[
  "Clone",
  "Copy",
  "Debug",
  "Default",
  "Eq",
  "Hash",
  "Ord",
  "PartialEq",
  "PartialOrd",
];

/// `#[class_interface]`: the trait, unchanged, and its descriptor.
pub fn class_interface(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
  if !attr.is_empty() {
    return Err(Error::new_spanned(attr, "#[class_interface] doesn't take any options"));
  }
  let item: ItemTrait =
    syn::parse2(item.clone()).map_err(|_| Error::new_spanned(item, "#[class_interface] must be on a trait"))?;
  let ident = &item.ident;
  let descriptor = format_ident!("__class_interface_{}", ident);
  let names = item.items.iter().filter_map(|item| match item {
    TraitItem::Fn(method) => Some(&method.sig.ident),
    _ => None,
  });
  // Only `#[macro_export]` can make a `macro_rules!` visible to other crates, and it requires the trait to
  // be at the crate root. Classes in other crates fall back to matching by visibility instead.
  let vis = match &item.vis {
    Visibility::Public(_) => quote!(pub(crate)),
    vis => vis.to_token_stream(),
  };
  Ok(quote! {
    #item

    #[doc(hidden)]
    #[allow(unused_macros)]
    macro_rules! #descriptor {
      ($callback:path, $payload:tt) => {
        $callback! { $payload [#(#names)*] }
      };
    }

    #[doc(hidden)]
    #[allow(unused_imports)]
    #vis use #descriptor as #ident;
  })
}

/// Stands in for the descriptor of a trait without `#[class_interface]`, passing no method names.
pub fn no_interface(input: TokenStream) -> Result<TokenStream> {
  let parser = |input: ParseStream<'_>| {
    let callback: Path = input.parse()?;
    input.parse::<Token![,]>()?;
    let payload: Group = input.parse()?;
    Ok(quote!(#callback! { #payload }))
  };
  parser.parse2(input)
}

/// Looks up the descriptor of each trait named by `implements`, then implements them, see the module docs.
pub fn trait_impls(model: &Model) -> Result<TokenStream> {
  if model.implements.is_empty() {
    return Ok(TokenStream::new());
  }
  let krate = &model.krate;
  let aliases: Vec<Ident> = (0..model.implements.len())
    .map(|index| format_ident!("__Interface{}", index))
    .collect();
  let lookups = model.implements.iter().zip(&aliases).filter_map(|(path, alias)| {
    let mut path = path.clone();
    for segment in &mut path.segments {
      segment.arguments = PathArguments::None;
    }
    let name = path.segments.last()?.ident.to_string();
    (!DERIVE_NAMES.contains(&name.as_str())).then(|| {
      quote! {
        #[allow(unused_imports)]
        use #path as #alias;
      }
    })
  });
  let (ig, _, wc) = model.generics.full.split_for_impl();
  let state = State {
    krate: krate.to_token_stream(),
    traits: model.implements.clone(),
    pending: aliases[1..].to_vec(),
    resolved: Vec::new(),
    impl_generics: ig.to_token_stream(),
    own: model.own(),
    where_clause: wc.to_token_stream(),
    candidates: model.methods.iter().map(Candidate::new).collect(),
  };
  let first = &aliases[0];
  // The invocation is nested once more, as one in the same block as the imports could shadow them.
  Ok(quote! {
    const _: () = {
      #(
        #[allow(unused_imports)]
        use #krate::__private::__no_interface as #aliases;
      )*
      const _: () = {
        #(#lookups)*
        const _: () = {
          #first! { #krate::__private::__implement, { #state } }
        };
      };
    };
  })
}

/// `__implement`: records a trait's method names (or that it has no descriptor), then either looks up the
/// next trait's descriptor or generates the impls.
pub fn implement(input: TokenStream) -> Result<TokenStream> {
  let parser = |input: ParseStream<'_>| {
    let state;
    braced!(state in input);
    let state: State = state.parse()?;
    let names = if input.is_empty() {
      None
    } else {
      let content;
      bracketed!(content in input);
      Some(parse_idents(&content)?)
    };
    Ok((state, names))
  };
  let (mut state, names) = parser.parse2(input)?;
  state.resolved.push(names);
  if !state.pending.is_empty() {
    let next = state.pending.remove(0);
    let krate = &state.krate;
    return Ok(quote!(#next! { #krate::__private::__implement, { #state } }));
  }
  state.generate()
}

/// What `__implement` passes along the chain of descriptors.
struct State {
  krate: TokenStream,
  traits: Vec<Path>,
  /// The aliases of the traits whose descriptor hasn't been looked up yet.
  pending: Vec<Ident>,
  /// For each trait looked up so far, its method names, if it has a descriptor.
  resolved: Vec<Option<Vec<Ident>>>,
  impl_generics: TokenStream,
  own: Type,
  where_clause: TokenStream,
  candidates: Vec<Candidate>,
}

impl State {
  fn generate(&self) -> Result<TokenStream> {
    let mut bodies = vec![TokenStream::new(); self.traits.len()];
    let mut errors: Option<Error> = None;
    for candidate in &self.candidates {
      match self.traits_of(candidate) {
        Ok(indices) => {
          for index in indices {
            bodies[index].extend(candidate.forwarder.clone());
          }
        }
        Err(error) => match errors.as_mut() {
          Some(errors) => errors.combine(error),
          None => errors = Some(error),
        },
      }
    }
    // Skips the impls, which would only add errors about the methods they're missing.
    if let Some(errors) = errors {
      return Err(errors);
    }
    let Self {
      impl_generics,
      own,
      where_clause,
      ..
    } = self;
    let impls = self.traits.iter().zip(bodies).map(|(path, body)| {
      quote_spanned! {path.span()=>
        impl #impl_generics #path for #own #where_clause {
          #body
        }
      }
    });
    Ok(quote!(#(#impls)*))
  }

  /// The traits a method implements: the one its marker names, those with a descriptor that declare a
  /// method of the same name, or else, for a virtual method without a visibility, the only trait without
  /// a descriptor.
  fn traits_of(&self, candidate: &Candidate) -> Result<Vec<usize>> {
    if let Some(index) = candidate.named {
      return Ok(vec![index]);
    }
    let declared: Vec<usize> = self
      .resolved
      .iter()
      .enumerate()
      .filter(|(_, names)| names.as_ref().is_some_and(|names| names.contains(&candidate.name)))
      .map(|(index, _)| index)
      .collect();
    if !declared.is_empty() || !candidate.by_visibility {
      return Ok(declared);
    }
    let undescribed: Vec<usize> = self
      .resolved
      .iter()
      .enumerate()
      .filter(|(_, names)| names.is_none())
      .map(|(index, _)| index)
      .collect();
    match undescribed.as_slice() {
      [] | [_] => Ok(undescribed),
      [first, ..] => {
        let marker = if candidate.is_abstract {
          "class_abstract"
        } else {
          "class_virtual"
        };
        Err(Error::new(
          candidate.name.span(),
          format!(
            "the class implements several traits, name the one this method implements, e.g. #[{marker}({})]",
            self.traits[*first].to_token_stream()
          ),
        ))
      }
    }
  }
}

impl Parse for State {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let bracket = || -> Result<TokenStream> {
      let content;
      bracketed!(content in input);
      content.parse()
    };
    let krate = bracket()?;
    let traits = {
      let content;
      bracketed!(content in input);
      let mut traits = Vec::new();
      while !content.is_empty() {
        let path;
        bracketed!(path in content);
        traits.push(path.parse()?);
      }
      traits
    };
    let pending = {
      let content;
      bracketed!(content in input);
      parse_idents(&content)?
    };
    let resolved = {
      let content;
      bracketed!(content in input);
      let mut resolved = Vec::new();
      while !content.is_empty() {
        if content.parse::<Option<Token![_]>>()?.is_some() {
          resolved.push(None);
          continue;
        }
        let names;
        bracketed!(names in content);
        resolved.push(Some(parse_idents(&names)?));
      }
      resolved
    };
    let impl_generics = bracket()?;
    let own = {
      let content;
      bracketed!(content in input);
      content.parse()?
    };
    let where_clause = bracket()?;
    let candidates = {
      let content;
      bracketed!(content in input);
      let mut candidates = Vec::new();
      while !content.is_empty() {
        candidates.push(content.parse()?);
      }
      candidates
    };
    Ok(Self {
      krate,
      traits,
      pending,
      resolved,
      impl_generics,
      own,
      where_clause,
      candidates,
    })
  }
}

impl ToTokens for State {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let Self {
      krate,
      traits,
      pending,
      resolved,
      impl_generics,
      own,
      where_clause,
      candidates,
    } = self;
    let resolved = resolved.iter().map(|names| match names {
      None => quote!(_),
      Some(names) => quote!([#(#names)*]),
    });
    tokens.extend(quote! {
      [#krate] [#([#traits])*] [#(#pending)*] [#(#resolved)*] [#impl_generics] [#own] [#where_clause]
      [#(#candidates)*]
    });
  }
}

/// A method of the class that may implement one of its traits.
struct Candidate {
  name: Ident,
  /// The index of the trait its marker names.
  named: Option<usize>,
  /// Whether it implements a trait without a descriptor: a virtual method without a visibility.
  by_visibility: bool,
  is_abstract: bool,
  /// The method of the trait impl, forwarding to the class's.
  forwarder: TokenStream,
}

impl Candidate {
  fn new(method: &Method) -> Self {
    Self {
      name: method.name().clone(),
      named: method.named_trait,
      by_visibility: method.kind == Kind::Virtual && matches!(method.item.vis, Visibility::Inherited),
      is_abstract: method.is_abstract,
      forwarder: forwarder(method),
    }
  }
}

impl Parse for Candidate {
  fn parse(input: ParseStream<'_>) -> Result<Self> {
    let content;
    braced!(content in input);
    let name = content.parse()?;
    let named = {
      let index;
      bracketed!(index in content);
      if index.is_empty() {
        None
      } else {
        Some(index.parse::<syn::LitInt>()?.base10_parse()?)
      }
    };
    let by_visibility = content.parse::<LitBool>()?.value;
    let is_abstract = content.parse::<LitBool>()?.value;
    let forwarder;
    braced!(forwarder in content);
    Ok(Self {
      name,
      named,
      by_visibility,
      is_abstract,
      forwarder: forwarder.parse()?,
    })
  }
}

impl ToTokens for Candidate {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let Self {
      name,
      named,
      by_visibility,
      is_abstract,
      forwarder,
    } = self;
    let named = named.map(Literal::usize_unsuffixed);
    tokens.extend(quote!({ #name [#named] #by_visibility #is_abstract { #forwarder } }));
  }
}

/// A trait method calling the class's method of the same name, which `Self::x` names in preference to the
/// trait's. Only the method's docs and `cfg`s are repeated. Without docs of its own, rustdoc shows the
/// trait's.
fn forwarder(method: &Method) -> TokenStream {
  let item = &method.item;
  let mut sig = item.sig.clone();
  sig.constness = None;
  let mut receiver = None;
  let mut args = Vec::new();
  for (index, arg) in sig.inputs.iter_mut().enumerate() {
    match arg {
      FnArg::Receiver(r) => {
        if r.reference.is_none() {
          r.mutability = None;
        }
        receiver = Some(r.self_token);
      }
      FnArg::Typed(arg) => {
        // Patterns are only for the class's method; the forwarder just passes the arguments on.
        let ident = match &*arg.pat {
          Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => pat.ident.clone(),
          _ => format_ident!("__arg{}", index),
        };
        *arg.pat = parse_quote!(#ident);
        args.push(ident);
      }
    }
  }
  // Explicit generic arguments aren't allowed alongside `impl Trait` parameters, which infer them anyway.
  let params: Vec<&Ident> = sig
    .generics
    .params
    .iter()
    .filter_map(|param| match param {
      GenericParam::Type(param) => Some(&param.ident),
      GenericParam::Const(param) => Some(&param.ident),
      GenericParam::Lifetime(_) => None,
    })
    .collect();
  let turbofish = (!params.is_empty() && !has_impl_trait(&sig)).then(|| quote!(::<#(#params),*>));
  let name = &sig.ident;
  let receiver = receiver.map(|r| quote!(#r,));
  let mut call = quote_spanned!(name.span()=> Self::#name #turbofish(#receiver #(#args),*));
  if sig.asyncness.is_some() {
    call = quote!(#call.await);
  }
  if sig.unsafety.is_some() {
    call = quote!(unsafe { #call });
  }
  let attrs = item.attrs.iter().filter(|a| {
    let path = a.path();
    path.is_ident("doc") || path.is_ident("cfg") || path.is_ident("cfg_attr")
  });
  // Spanned so that errors about a method, e.g. that the trait doesn't have it, point at it.
  quote_spanned! {name.span()=>
    #(#attrs)*
    #[inline]
    #sig {
      #call
    }
  }
}

fn parse_idents(input: ParseStream<'_>) -> Result<Vec<Ident>> {
  let mut idents = Vec::new();
  while !input.is_empty() {
    idents.push(input.parse()?);
  }
  Ok(idents)
}

fn has_impl_trait(sig: &syn::Signature) -> bool {
  struct Find(bool);

  impl Fold for Find {
    fn fold_type(&mut self, ty: Type) -> Type {
      self.0 |= matches!(ty, Type::ImplTrait(_));
      fold::fold_type(self, ty)
    }
  }

  let mut find = Find(false);
  for arg in &sig.inputs {
    if let FnArg::Typed(arg) = arg {
      find.fold_type(*arg.ty.clone());
    }
  }
  find.0
}
