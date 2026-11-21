//! Class-level members: virtual, abstract and overriding associated functions (without a receiver) and
//! constants. They're late-bound like Python's `cls` or PHP's `static::`: in their bodies, `Self` is the
//! class they're accessed through, so `Sub::locate()` runs an inherited `locate` with `Self = Sub`.
//!
//! Rust only binds `Self` late in a trait's default methods, so a class that declares class-level members
//! (or, if it's open, overrides some) gets a hidden *class trait*, `__NameClass`, a subtrait of its nearest
//! ancestor's. It holds the members the class declares, with their bodies as default methods, and the
//! bodies of the overrides of an open class, which its descendants share. A class with no unimplemented
//! abstract class-level member implements every class trait of its hierarchy, choosing for each member the
//! nearest override, and gets inherent forwarders for all of them, so neither callers nor subclasses need
//! to import a trait. A final class's overrides go straight into its impls, as no subclass can inherit
//! them, so `Self` is concrete in them.
//!
//! Subclasses repeat the signatures their ancestors declare (see `descriptor`), so the types they name must
//! resolve where the subclass is declared too.

use super::Model;
use super::descriptor::{StaticMember, StaticSig};
use super::methods::{Forward, Kind, Static, StaticItem};
use proc_macro2::{Group, Spacing, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote};
use std::collections::HashMap;
use syn::{Attribute, Ident, Path, Result, Type, parse_quote};

fn trait_ident(model: &Model) -> Ident {
  format_ident!("__{}Class", model.name)
}

fn default_ident(name: &Ident) -> Ident {
  format_ident!("__default_{}", name)
}

fn override_ident(name: &Ident) -> Ident {
  format_ident!("__override_{}", name)
}

/// The class's own class trait, `__NameClass<T..>`, if it has one.
pub fn own_class_trait(model: &Model) -> Option<Path> {
  let declares = model.statics.iter().any(|s| s.kind == Kind::Virtual);
  let shares_overrides = model.open && model.statics.iter().any(|s| s.kind == Kind::Override);
  (declares || shares_overrides).then(|| {
    let ident = trait_ident(model);
    let tg = model.generics.full.split_for_impl().1;
    parse_quote!(#ident #tg)
  })
}

/// The class-level members the class declares, as its descriptor records them.
pub fn own_records(model: &Model) -> Vec<StaticMember> {
  model
    .statics
    .iter()
    .filter(|s| s.kind == Kind::Virtual)
    .map(|s| StaticMember {
      vis: s.vis().clone(),
      is_abstract: s.is_abstract,
      docs: docs(s.attrs()).cloned().collect(),
      sig: match &s.item {
        StaticItem::Fn(item) => StaticSig::Fn(item.sig.clone()),
        StaticItem::Const(item) => StaticSig::Const(item.ident.clone(), item.ty.clone()),
      },
    })
    .collect()
}

/// The names of the inherited class-level members the class overrides.
pub fn own_overrides(model: &Model) -> Vec<Ident> {
  model
    .statics
    .iter()
    .filter(|s| s.kind == Kind::Override)
    .map(|s| s.name().clone())
    .collect()
}

fn docs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
  attrs.iter().filter(|a| a.path().is_ident("doc"))
}

fn non_docs(attrs: &[Attribute]) -> impl Iterator<Item = &Attribute> {
  attrs.iter().filter(|a| !a.path().is_ident("doc"))
}

/// A class of the hierarchy, as far as its class-level members go: an ancestor, or the class itself (last).
struct Level {
  handle: Type,
  class_trait: Option<Path>,
  declared: Vec<StaticMember>,
  overrides: Vec<Ident>,
}

fn levels(model: &Model) -> Vec<Level> {
  model
    .ancestors
    .iter()
    .map(|a| Level {
      handle: a.handle.clone(),
      class_trait: a.class_trait.clone(),
      declared: a.statics.clone(),
      overrides: a.static_overrides.clone(),
    })
    .chain([Level {
      handle: model.own(),
      class_trait: own_class_trait(model),
      declared: own_records(model),
      overrides: own_overrides(model),
    }])
    .collect()
}

/// Which implementation of a member, declared by `levels[declaring]`, the class uses.
enum Provider {
  /// The declaration's default.
  Default,
  /// The final class's own override, inlined into its impl.
  Inline,
  /// The override shared by an open class through its class trait.
  Shared(Path),
}

fn provider(model: &Model, levels: &[Level], declaring: usize, name: &Ident) -> Provider {
  let last = levels.len() - 1;
  for index in (declaring + 1..=last).rev() {
    if levels[index].overrides.contains(name) {
      if index == last && !model.open {
        return Provider::Inline;
      }
      if let Some(path) = &levels[index].class_trait {
        return Provider::Shared(path.clone());
      }
    }
  }
  Provider::Default
}

/// The replacement of `Self::super_x` for each override that has a parent implementation to call.
fn super_calls(model: &Model, levels: &[Level]) -> HashMap<String, TokenStream> {
  let last = levels.len() - 1;
  let mut calls = HashMap::new();
  for member in model.statics.iter().filter(|s| s.kind == Kind::Override && s.has_super) {
    let name = member.name();
    let declaring = model.declaring_ancestor_of(name);
    let above = (declaring + 1..last)
      .rev()
      .find(|&index| levels[index].overrides.contains(name))
      .and_then(|index| {
        levels[index]
          .class_trait
          .as_ref()
          .map(|path| (path, override_ident(name)))
      });
    let target = above.or_else(|| {
      let declared = levels[declaring].declared.iter().find(|m| m.name() == name)?;
      let path = levels[declaring].class_trait.as_ref()?;
      (!declared.is_abstract).then(|| (path, default_ident(name)))
    });
    if let Some((path, ident)) = target {
      calls.insert(format!("super_{name}"), quote!(<Self as #path>::#ident));
    }
  }
  calls
}

/// Replaces each `Self::super_x` with the parent's implementation of `x`, which, unlike an instance
/// method's `super_x`, can't be a member of its own: a class trait sees its ancestors', so their
/// `super_x`s would be ambiguous.
fn rewrite_super(tokens: TokenStream, calls: &HashMap<String, TokenStream>) -> TokenStream {
  if calls.is_empty() {
    return tokens;
  }
  let trees: Vec<TokenTree> = tokens.into_iter().collect();
  let mut out = TokenStream::new();
  let mut index = 0;
  while index < trees.len() {
    if let (TokenTree::Ident(this), Some(TokenTree::Punct(a)), Some(TokenTree::Punct(b)), Some(TokenTree::Ident(name))) = (
      &trees[index],
      trees.get(index + 1),
      trees.get(index + 2),
      trees.get(index + 3),
    ) && this == "Self"
      && a.as_char() == ':'
      && a.spacing() == Spacing::Joint
      && b.as_char() == ':'
      && let Some(call) = calls.get(&name.to_string())
    {
      out.extend(call.clone());
      index += 4;
      continue;
    }
    match &trees[index] {
      TokenTree::Group(group) => {
        let mut rewritten = Group::new(group.delimiter(), rewrite_super(group.stream(), calls));
        rewritten.set_span(group.span());
        out.extend([TokenTree::Group(rewritten)]);
      }
      other => out.extend([other.clone()]),
    }
    index += 1;
  }
  out
}

/// The class trait, the impls of its hierarchy's class traits, the forwarders and the ordinary constants.
pub fn generate(model: &Model) -> Result<TokenStream> {
  let own = model.own();
  let (ig, _, wc) = model.generics.full.split_for_impl();
  let levels = levels(model);
  let calls = super_calls(model, &levels);
  let rewrite = |tokens: TokenStream| rewrite_super(tokens, &calls);

  let consts = &model.consts;
  let consts = (!consts.is_empty()).then(|| {
    quote! {
      impl #ig #own #wc {
        #(#consts)*
      }
    }
  });

  let class_trait = own_class_trait(model).map(|path| class_trait(model, &path, &rewrite));
  let complete = model.class_level_complete && levels.iter().any(|l| l.class_trait.is_some());
  let (impls, forwarders) = if complete {
    (impls(model, &levels, &rewrite), Some(forwarders(model, &levels)))
  } else {
    (Vec::new(), None)
  };
  let forwarders = forwarders.map(|forwarders| {
    quote! {
      #[allow(dead_code)]
      impl #ig #own #wc {
        #(#forwarders)*
      }
    }
  });
  Ok(quote! {
    #consts
    #class_trait
    #(#impls)*
    #forwarders
  })
}

fn class_trait(model: &Model, own_trait: &Path, rewrite: &dyn Fn(TokenStream) -> TokenStream) -> TokenStream {
  let vis = &model.vis;
  let own = model.own();
  let ident = trait_ident(model);
  let full = &model.generics.full;
  let wc = &full.where_clause;
  let supertrait = model
    .ancestors
    .iter()
    .rev()
    .find_map(|a| a.class_trait.as_ref())
    .map_or_else(|| quote!(::core::marker::Sized), |path| quote!(#path));
  let items = model.statics.iter().map(|member| {
    let name = member.name();
    let docs: Vec<_> = docs(member.attrs()).collect();
    let others: Vec<_> = non_docs(member.attrs()).collect();
    match (&member.item, member.kind) {
      (StaticItem::Fn(item), Kind::Virtual) => {
        let forward = Forward::new(&item.sig);
        let sig = &forward.sig;
        if member.is_abstract {
          return quote!(#(#docs)* #sig;);
        }
        let hidden = default_ident(name);
        let mut body_sig = item.sig.clone();
        body_sig.ident = hidden.clone();
        let block = rewrite(item.block.to_token_stream());
        let call = forward.call(quote!(<Self as #own_trait>::#hidden));
        quote! {
          #(#docs)*
          #[inline]
          #sig { #call }

          #[doc(hidden)]
          #(#others)*
          #body_sig #block
        }
      }
      (StaticItem::Const(item), Kind::Virtual) => {
        let ty = &item.ty;
        let Some(expr) = &item.expr else {
          return quote!(#(#docs)* const #name: #ty;);
        };
        let hidden = default_ident(name);
        let expr = rewrite(expr.to_token_stream());
        quote! {
          #(#docs)*
          const #name: #ty = <Self as #own_trait>::#hidden;

          #[doc(hidden)]
          #(#others)*
          const #hidden: #ty = #expr;
        }
      }
      (StaticItem::Fn(item), _) if model.open => {
        let mut body_sig = item.sig.clone();
        body_sig.ident = override_ident(name);
        let block = rewrite(item.block.to_token_stream());
        quote! {
          #[doc(hidden)]
          #(#others)*
          #body_sig #block
        }
      }
      (StaticItem::Const(item), _) if model.open => {
        let (hidden, ty) = (override_ident(name), &item.ty);
        let expr = rewrite(item.expr.to_token_stream());
        quote! {
          #[doc(hidden)]
          #(#others)*
          const #hidden: #ty = #expr;
        }
      }
      _ => quote!(),
    }
  });
  let doc = format!(
    "The class-level members of [`{}`], which its subclasses inherit.",
    model.name
  );
  quote! {
    #[doc = #doc]
    #[doc(hidden)]
    #[allow(async_fn_in_trait, non_upper_case_globals, non_snake_case)]
    #vis trait #ident #full: #supertrait #wc {
      #[doc(hidden)]
      fn __view(&self) -> &#own;

      #(#items)*
    }
  }
}

/// The class's own member named `name`, if it declares or overrides it.
fn own_member<'a>(model: &'a Model, name: &Ident) -> Option<&'a Static> {
  model.statics.iter().find(|s| s.name() == name)
}

/// The class's impls of its hierarchy's class traits.
fn impls(model: &Model, levels: &[Level], rewrite: &dyn Fn(TokenStream) -> TokenStream) -> Vec<TokenStream> {
  let own = model.own();
  let (ig, _, wc) = model.generics.full.split_for_impl();
  levels
    .iter()
    .enumerate()
    .filter_map(|(declaring, level)| {
      let path = level.class_trait.as_ref()?;
      let handle = &level.handle;
      let items = level.declared.iter().map(|declared| {
        let name = declared.name();
        let written = own_member(model, name);
        match provider(model, levels, declaring, name) {
          Provider::Default => quote!(),
          Provider::Inline => match written.map(|s| &s.item) {
            Some(StaticItem::Fn(item)) => {
              let others = non_docs(&item.attrs);
              let sig = &item.sig;
              let block = rewrite(item.block.to_token_stream());
              quote!(#(#others)* #sig #block)
            }
            Some(StaticItem::Const(item)) => {
              let ty = &item.ty;
              let expr = rewrite(item.expr.to_token_stream());
              quote!(const #name: #ty = #expr;)
            }
            None => quote!(),
          },
          Provider::Shared(provider) => {
            let hidden = override_ident(name);
            match (written.map(|s| &s.item), &declared.sig) {
              (Some(StaticItem::Fn(item)), _) => shared_fn(&item.sig, &provider, &hidden),
              (_, StaticSig::Fn(sig)) => shared_fn(sig, &provider, &hidden),
              (Some(StaticItem::Const(item)), _) => {
                let ty = &item.ty;
                quote!(const #name: #ty = <Self as #provider>::#hidden;)
              }
              (_, StaticSig::Const(_, ty)) => quote!(const #name: #ty = <Self as #provider>::#hidden;),
            }
          }
        }
      });
      Some(quote! {
        impl #ig #path for #own #wc {
          #[inline]
          fn __view(&self) -> &#handle {
            self
          }

          #(#items)*
        }
      })
    })
    .collect()
}

fn shared_fn(sig: &syn::Signature, provider: &Path, hidden: &Ident) -> TokenStream {
  let forward = Forward::new(sig);
  let sig = &forward.sig;
  let call = forward.call(quote!(<Self as #provider>::#hidden));
  quote! {
    #[inline]
    #sig { #call }
  }
}

/// An inherent forwarder for every class-level member of the hierarchy, so that calling one doesn't need
/// its class trait in scope.
fn forwarders(model: &Model, levels: &[Level]) -> Vec<TokenStream> {
  levels
    .iter()
    .filter_map(|level| level.class_trait.as_ref().map(|path| (level, path)))
    .flat_map(|(level, path)| {
      level.declared.iter().map(move |declared| {
        let name = declared.name();
        let written = own_member(model, name);
        let (vis, docs): (_, Vec<&Attribute>) = match written {
          Some(member) => (member.vis(), docs(member.attrs()).collect()),
          None => (&declared.vis, declared.docs.iter().collect()),
        };
        let item = match (written.map(|s| &s.item), &declared.sig) {
          (Some(StaticItem::Fn(item)), _) => forward_fn(&item.sig, path, vis),
          (_, StaticSig::Fn(sig)) => forward_fn(sig, path, vis),
          (Some(StaticItem::Const(item)), _) => {
            let ty = &item.ty;
            quote!(#vis const #name: #ty = <Self as #path>::#name;)
          }
          (_, StaticSig::Const(_, ty)) => quote!(#vis const #name: #ty = <Self as #path>::#name;),
        };
        quote! {
          #(#docs)*
          #item
        }
      })
    })
    .collect()
}

fn forward_fn(sig: &syn::Signature, path: &Path, vis: &syn::Visibility) -> TokenStream {
  let forward = Forward::new(sig);
  let name = &sig.ident;
  let call = forward.call(quote!(<Self as #path>::#name));
  let sig = &forward.sig;
  quote! {
    #[inline]
    #vis #sig { #call }
  }
}
