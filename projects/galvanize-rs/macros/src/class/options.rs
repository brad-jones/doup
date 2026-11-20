//! Parsing `#[class(...)]`'s options.

use proc_macro2::TokenStream;
use syn::{
  Path, Result, Token, Type,
  meta::ParseNestedMeta,
  parse::{Parse, Parser},
  parse_quote,
  punctuated::Punctuated,
};

/// Which runtime strategy generates a hierarchy.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Runtime {
  Safe,
  Unsafe,
}

impl Runtime {
  pub fn is_unsafe(self) -> bool {
    self == Runtime::Unsafe
  }

  pub fn name(self) -> &'static str {
    match self {
      Runtime::Safe => "SafeRuntimeStrategy",
      Runtime::Unsafe => "UnsafeRuntimeStrategy",
    }
  }
}

/// A `runtime_strategy = ...` option, as written.
pub struct ExplicitRuntime {
  pub runtime: Runtime,
  pub path: Path,
}

/// Options given to `#[class(...)]` on the struct.
pub struct ClassOptions {
  pub krate: Path,
  /// Set by `open`, and implied by `abstract`.
  pub open: bool,
  pub is_abstract: bool,
  /// The traits named by `implements`, in order.
  pub implements: Vec<Path>,
  pub extends: Option<Type>,
  pub runtime: Option<ExplicitRuntime>,
  pub pointer_strategy: Type,
}

pub fn parse_class_options(attr: TokenStream) -> Result<ClassOptions> {
  let mut krate = None;
  let mut open = None;
  let mut is_abstract = None;
  let mut implements = None;
  let mut extends = None;
  let mut runtime = None;
  let mut pointer_strategy = None;

  syn::meta::parser(|meta| {
    if meta.path.is_ident("open") {
      set_once(&mut open, (), &meta)
    } else if meta.path.is_ident("abstract") {
      set_once(&mut is_abstract, (), &meta)
    } else if meta.path.is_ident("implements") {
      set_once(&mut implements, parse_implements(&meta)?, &meta)
    } else if meta.path.is_ident("extends") {
      set_once(&mut extends, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("runtime_strategy") {
      let path: Path = meta.value()?.parse()?;
      set_once(&mut runtime, parse_runtime(path)?, &meta)
    } else if meta.path.is_ident("pointer_strategy") {
      set_once(&mut pointer_strategy, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("crate") {
      set_once(&mut krate, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("strategy") {
      Err(meta.error("`strategy` was renamed to `pointer_strategy`"))
    } else {
      Err(meta.error(
        "unknown class option, expected `open`, `abstract`, `extends = ...`, `implements = ...`, \
         `runtime_strategy = ...`, `pointer_strategy = ...` or `crate = ...`",
      ))
    }
  })
  .parse2(attr)?;

  let krate: Path = krate.unwrap_or_else(|| parse_quote!(::galvanize_rs));
  let pointer_strategy = pointer_strategy.unwrap_or_else(|| parse_quote!(#krate::DefaultPointerStrategy));
  Ok(ClassOptions {
    krate,
    open: open.is_some() || is_abstract.is_some(),
    is_abstract: is_abstract.is_some(),
    implements: implements.unwrap_or_default(),
    extends,
    runtime,
    pointer_strategy,
  })
}

/// Parses `implements = Trait` or `implements = (TraitA, TraitB)`.
fn parse_implements(meta: &ParseNestedMeta) -> Result<Vec<Path>> {
  let value = meta.value()?;
  if !value.peek(syn::token::Paren) {
    return Ok(vec![value.parse()?]);
  }
  let content;
  syn::parenthesized!(content in value);
  let traits: Punctuated<Path, Token![,]> = content.parse_terminated(Path::parse, Token![,])?;
  if traits.is_empty() {
    return Err(meta.error("name at least one trait"));
  }
  let mut seen = std::collections::HashSet::new();
  for path in &traits {
    if !seen.insert(quote::ToTokens::to_token_stream(path).to_string()) {
      return Err(syn::Error::new_spanned(path, "this trait is already listed"));
    }
  }
  Ok(traits.into_iter().collect())
}

/// Picks the generator from the strategy's name, as a proc macro can't resolve types. The generated code
/// also checks that the path really names that strategy, see `assert_runtime`.
fn parse_runtime(path: Path) -> Result<ExplicitRuntime> {
  let ident = &path
    .segments
    .last()
    .ok_or_else(|| syn::Error::new_spanned(&path, "expected a runtime strategy"))?
    .ident;
  let runtime = if ident == "SafeRuntimeStrategy" || ident == "DefaultRuntimeStrategy" {
    Runtime::Safe
  } else if ident == "UnsafeRuntimeStrategy" {
    Runtime::Unsafe
  } else {
    return Err(syn::Error::new_spanned(
      &path,
      "expected `SafeRuntimeStrategy`, `UnsafeRuntimeStrategy` or `DefaultRuntimeStrategy` (aliases aren't \
       supported)",
    ));
  };
  Ok(ExplicitRuntime { runtime, path })
}

pub fn set_once<T>(slot: &mut Option<T>, value: T, meta: &ParseNestedMeta) -> Result<()> {
  if slot.is_some() {
    return Err(meta.error("duplicate option"));
  }
  *slot = Some(value);
  Ok(())
}
