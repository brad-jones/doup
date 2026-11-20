//! The `#[class_methods]` block: ordinary, virtual, abstract and overriding methods.

use super::fields::names_in;
use proc_macro2::{Span, TokenStream};
use quote::ToTokens;
use std::collections::{HashMap, HashSet};
use syn::{
  Attribute, Error, FnArg, Generics, Ident, ImplItem, ImplItemFn, ItemImpl, Meta, Pat, Path, PathArguments, Result,
  ReturnType, Signature, Token, Type, Visibility,
  fold::{self, Fold},
  parse::{ParseStream, Parser},
  parse_quote,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
  Ordinary,
  /// Declares a virtual method, which may be abstract.
  Virtual,
  Override,
}

pub struct Method {
  pub kind: Kind,
  /// The method with its `class_virtual`/`class_abstract`/`class_override` marker removed. An abstract
  /// method's empty block is a placeholder, it has no body.
  pub item: ImplItemFn,
  /// Whether this is a virtual method without a body.
  pub is_abstract: bool,
  /// The index, in the class's `implements`, of the trait this virtual method implements.
  pub implements: Option<usize>,
  /// Whether an override gets a `super_x` method, i.e. whether an ancestor implements the method.
  pub has_super: bool,
}

impl Method {
  pub fn name(&self) -> &Ident {
    &self.item.sig.ident
  }

  pub fn is_async(&self) -> bool {
    self.item.sig.asyncness.is_some()
  }

  /// The `self` token of the method's receiver, which generated code must use to refer to it.
  pub fn receiver(&self) -> Result<&Token![self]> {
    self
      .item
      .sig
      .receiver()
      .map(|r| &r.self_token)
      .ok_or_else(|| Error::new_spanned(&self.item.sig, "missing receiver"))
  }

  /// Whether this is a constructor of `class`, which stops `#[class]` generating `new()`: a method named
  /// `new`, or an associated function whose return type mentions `Self` or the class, e.g. `-> Self`,
  /// `-> Option<Self>` or `-> Result<Name, E>`. Functions returning `NameData`, such as `init`, aren't.
  pub fn is_constructor(&self, class: &Ident) -> bool {
    let sig = &self.item.sig;
    if sig.ident == "new" {
      return true;
    }
    let ReturnType::Type(_, ty) = &sig.output else {
      return false;
    };
    let names = names_in(ty.to_token_stream());
    sig.receiver().is_none() && (names.contains("Self") || names.contains(&class.to_string()))
  }
}

/// A virtual method declared by an ancestor.
pub struct InheritedMethod {
  /// The index of the ancestor that declares it.
  pub index: usize,
  /// The name of the ancestor that declares it, for errors.
  pub class: String,
  pub is_async: bool,
  pub vis: Visibility,
}

/// The virtual methods declared by a class's ancestors, by name.
pub type Inherited = HashMap<String, InheritedMethod>;

/// What `collect` needs to know about the class.
pub struct Context<'a> {
  pub inherited: &'a Inherited,
  /// The inherited abstract methods that no ancestor implements.
  pub unimplemented: &'a HashSet<String>,
  /// The traits named by the class's `implements` option.
  pub implements: &'a [Path],
  pub is_abstract: bool,
}

/// Checks the methods block is a plain inherent impl of the class.
pub fn validate_impl(methods: &ItemImpl, own: &Type, generics: &Generics) -> Result<()> {
  if methods.trait_.is_some() || methods.unsafety.is_some() || methods.defaultness.is_some() {
    return Err(Error::new_spanned(
      methods,
      "#[class_methods] must be on an inherent impl of the class, e.g. `impl Name { ... }`",
    ));
  }
  if methods.self_ty.to_token_stream().to_string() != own.to_token_stream().to_string()
    || !methods
      .generics
      .type_params()
      .map(|p| &p.ident)
      .eq(generics.type_params().map(|p| &p.ident))
  {
    return Err(Error::new_spanned(
      &methods.self_ty,
      format!(
        "#[class_methods] must implement `{}`, with the struct's type parameters",
        own.to_token_stream()
      ),
    ));
  }
  if methods.generics.where_clause.is_some()
    || methods.generics.lifetimes().next().is_some()
    || methods.generics.const_params().next().is_some()
    || methods
      .generics
      .type_params()
      .any(|p| !p.bounds.is_empty() || p.default.is_some())
  {
    return Err(Error::new_spanned(
      &methods.generics,
      "write bounds on the class struct rather than #[class_methods], they apply to its methods too",
    ));
  }
  Ok(())
}

/// Collects the methods of a `#[class_methods]` block, checking them against the names the class
/// generates (`reserved`) and the virtual methods its ancestors declare.
pub fn collect(block: ItemImpl, reserved: &mut HashSet<String>, context: &Context<'_>) -> Result<Vec<Method>> {
  let mut methods = Vec::new();
  let mut errors: Option<Error> = None;
  for item in block.items {
    match collect_one(item, reserved, context) {
      Ok(method) => methods.push(method),
      Err(e) => match errors.as_mut() {
        Some(errors) => errors.combine(e),
        None => errors = Some(e),
      },
    }
  }
  if let Some(errors) = errors {
    return Err(errors);
  }
  for method in &methods {
    if method.has_super && reserved.contains(&format!("super_{}", method.name())) {
      return Err(Error::new_spanned(
        method.name(),
        format!("`super_{}` conflicts with another member", method.name()),
      ));
    }
  }
  Ok(methods)
}

/// A method's `#[class_virtual]`, `#[class_abstract]` or `#[class_override]` marker.
struct Marker {
  kind: Kind,
  is_abstract: bool,
  /// The trait named by `#[class_virtual(Trait)]` or `#[class_abstract(Trait)]`.
  implements: Option<Path>,
}

fn collect_one(item: ImplItem, reserved: &mut HashSet<String>, context: &Context<'_>) -> Result<Method> {
  let (mut item, has_body) = match item {
    ImplItem::Fn(item) => (item, true),
    ImplItem::Verbatim(tokens) => (parse_bodyless(tokens)?, false),
    other => return Err(Error::new_spanned(other, "#[class_methods] only supports methods")),
  };
  let marker = take_marker(&mut item)?;
  let label = item.sig.ident.to_string();
  let inherited = context.inherited.get(&label);
  // A method named after an inherited virtual method overrides it, so it can't shadow it by accident. As in
  // Scala, replacing an implementation must be marked, while implementing an abstract method needn't be
  // (a misspelt implementation leaves the abstract method unimplemented, which is an error anyway).
  let kind = match (marker.kind, inherited) {
    (Kind::Ordinary, Some(_)) if context.unimplemented.contains(&label) => Kind::Override,
    (Kind::Ordinary, Some(declared)) => {
      return Err(Error::new_spanned(
        &item.sig.ident,
        format!(
          "`{label}` would override `{}::{label}`, mark it #[class_override] or rename it",
          declared.class
        ),
      ));
    }
    (kind, _) => kind,
  };
  if !has_body && !marker.is_abstract {
    return Err(Error::new_spanned(
      &item.sig,
      "only #[class_abstract] methods can leave out their body",
    ));
  }
  if has_body && marker.is_abstract {
    return Err(Error::new_spanned(
      &item.block,
      "an abstract method has no body, end its signature with `;`",
    ));
  }
  if marker.is_abstract && !context.is_abstract {
    return Err(Error::new_spanned(
      &item.sig.ident,
      "only abstract classes can declare abstract methods, declare the class with #[class(abstract)]",
    ));
  }
  if label.starts_with("__") || !reserved.insert(label.clone()) {
    return Err(Error::new_spanned(
      &item.sig.ident,
      "this method's name conflicts with a member generated by #[class]",
    ));
  }
  if kind == Kind::Ordinary {
    return Ok(Method {
      kind,
      item,
      is_abstract: false,
      implements: None,
      has_super: false,
    });
  }
  check_dispatchable(&mut item)?;
  check_virtual_signature(&item)?;
  let implements = match (kind, inherited) {
    (Kind::Virtual, Some(_)) => {
      return Err(Error::new_spanned(
        &item.sig.ident,
        "an ancestor already declares this virtual method, leave out the marker to override it",
      ));
    }
    (Kind::Override, None) => {
      return Err(Error::new_spanned(
        &item.sig.ident,
        "no ancestor declares a virtual method with this name",
      ));
    }
    (Kind::Override, Some(declared)) if declared.is_async != item.sig.asyncness.is_some() => {
      return Err(Error::new_spanned(
        &item.sig,
        "an override must match whether the inherited method is async",
      ));
    }
    (Kind::Override, Some(declared)) => {
      check_override_visibility(&item, &declared.vis)?;
      None
    }
    _ => implemented_trait(
      &item,
      marker.implements.as_ref(),
      marker.is_abstract,
      context.implements,
    )?,
  };
  Ok(Method {
    kind,
    has_super: kind == Kind::Override && !context.unimplemented.contains(&label),
    item,
    is_abstract: marker.is_abstract,
    implements,
  })
}

/// Parses a method without a body, `fn name(&self);`, which `syn` leaves unparsed.
fn parse_bodyless(tokens: TokenStream) -> Result<ImplItemFn> {
  let parser = |input: ParseStream<'_>| {
    let attrs = input.call(Attribute::parse_outer)?;
    let vis = input.parse()?;
    let sig: Signature = input.parse()?;
    input.parse::<Token![;]>()?;
    Ok(ImplItemFn {
      attrs,
      vis,
      defaultness: None,
      sig,
      block: parse_quote!({}),
    })
  };
  parser
    .parse2(tokens.clone())
    .map_err(|_| Error::new_spanned(tokens, "#[class_methods] only supports methods"))
}

/// Removes a method's `#[class_virtual]`, `#[class_abstract]` or `#[class_override]` marker.
fn take_marker(method: &mut ImplItemFn) -> Result<Marker> {
  let mut marker = Marker {
    kind: Kind::Ordinary,
    is_abstract: false,
    implements: None,
  };
  let mut found = false;
  let mut retained = Vec::new();
  for attr in &method.attrs {
    let (kind, is_abstract) = if attr.path().is_ident("class_virtual") {
      (Kind::Virtual, false)
    } else if attr.path().is_ident("class_abstract") {
      (Kind::Virtual, true)
    } else if attr.path().is_ident("class_override") {
      (Kind::Override, false)
    } else {
      retained.push(attr.clone());
      continue;
    };
    let implements = match &attr.meta {
      Meta::Path(_) => None,
      Meta::List(list) if kind == Kind::Virtual => Some(list.parse_args::<Path>().map_err(|_| {
        Error::new_spanned(
          attr,
          "expected the trait this method implements, e.g. #[class_virtual(Trait)]",
        )
      })?),
      _ => None,
    };
    let bare_override = kind == Kind::Override && !matches!(attr.meta, Meta::Path(_));
    if found || bare_override || matches!(attr.meta, Meta::NameValue(_)) {
      return Err(Error::new_spanned(
        attr,
        "use a single #[class_virtual], #[class_abstract] or a bare #[class_override]",
      ));
    }
    found = true;
    marker = Marker {
      kind,
      is_abstract,
      implements,
    };
  }
  method.attrs = retained;
  Ok(marker)
}

/// Checks the signature a virtual method or override needs to be dispatched, and marks it `#[inline]`.
fn check_dispatchable(method: &mut ImplItemFn) -> Result<()> {
  if !method.attrs.iter().any(|attr| attr.path().is_ident("inline")) {
    method.attrs.push(parse_quote!(#[inline]));
  }
  let sig = &method.sig;
  let receiver_ok = sig
    .receiver()
    .is_some_and(|r| r.reference.is_some() && r.mutability.is_none() && r.colon_token.is_none());
  if !receiver_ok
    || sig.unsafety.is_some()
    || sig.constness.is_some()
    || sig.abi.is_some()
    || sig.variadic.is_some()
    || !sig.generics.params.is_empty()
    || sig.generics.where_clause.is_some()
  {
    return Err(Error::new_spanned(
      sig,
      "virtual methods and overrides need a safe, non-generic `&self` signature",
    ));
  }
  arguments(method)?;
  Ok(())
}

/// Requires an override to repeat the visibility of the method it overrides.
fn check_override_visibility(method: &ImplItemFn, declared: &Visibility) -> Result<()> {
  let written = method.vis.to_token_stream().to_string();
  let expected = declared.to_token_stream().to_string();
  if written == expected {
    return Ok(());
  }
  let message = if expected.is_empty() {
    "an override must have the same visibility as the method it overrides, which has none".to_string()
  } else {
    format!("an override must have the same visibility as the method it overrides, `{expected}`")
  };
  if matches!(method.vis, Visibility::Inherited) {
    Err(Error::new_spanned(&method.sig.ident, message))
  } else {
    Err(Error::new_spanned(&method.vis, message))
  }
}

/// Works out which of the class's `implements` traits a virtual method implements, if any.
///
/// A method naming a trait implements it. Otherwise, a method without a visibility implements the class's
/// only trait, as methods in a trait impl don't take a visibility either.
fn implemented_trait(
  method: &ImplItemFn,
  named: Option<&Path>,
  is_abstract: bool,
  implements: &[Path],
) -> Result<Option<usize>> {
  let marker = if is_abstract { "class_abstract" } else { "class_virtual" };
  let Some(named) = named else {
    return match (&method.vis, implements.len()) {
      (Visibility::Inherited, 1) => Ok(Some(0)),
      (Visibility::Inherited, n) if n > 1 => Err(Error::new_spanned(
        &method.sig.ident,
        format!(
          "the class implements several traits, name the one this method implements, e.g. #[{marker}({})]",
          implements[0].to_token_stream()
        ),
      )),
      _ => Ok(None),
    };
  };
  if !matches!(method.vis, Visibility::Inherited) {
    return Err(Error::new_spanned(
      &method.vis,
      "a method implementing a trait doesn't take a visibility, as in a trait impl",
    ));
  }
  let text = named.to_token_stream().to_string();
  let exact = implements.iter().position(|p| p.to_token_stream().to_string() == text);
  // A bare name also matches a trait listed with a path or type arguments, if only one does.
  let by_name = || {
    let ident = named.get_ident()?;
    let mut matches = implements
      .iter()
      .enumerate()
      .filter(|(_, p)| p.segments.last().is_some_and(|s| s.ident == *ident));
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
  };
  exact.or_else(by_name).map(Some).ok_or_else(|| {
    Error::new_spanned(
      named,
      format!("`{text}` isn't one of the traits in this class's `implements` option"),
    )
  })
}

/// Rejects parameter and return types that a dispatch table can't represent.
fn check_virtual_signature(method: &ImplItemFn) -> Result<()> {
  struct Check(Option<Error>);

  impl Fold for Check {
    fn fold_type(&mut self, ty: Type) -> Type {
      let unsupported = matches!(&ty, Type::ImplTrait(_))
        || matches!(&ty, Type::Path(p) if p.qself.is_none() && p.path.segments.first().is_some_and(|s| s.ident == "Self"));
      if unsupported && self.0.is_none() {
        self.0 = Some(Error::new_spanned(
          &ty,
          "virtual methods can't use `Self` or `impl Trait` in their signature, name the class instead",
        ));
      }
      fold::fold_type(self, ty)
    }
  }

  let mut check = Check(None);
  for arg in method.sig.inputs.iter().skip(1) {
    if let FnArg::Typed(arg) = arg {
      check.fold_type(*arg.ty.clone());
    }
  }
  if let ReturnType::Type(_, ty) = &method.sig.output {
    check.fold_type(*ty.clone());
  }
  check.0.map_or(Ok(()), Err)
}

/// The names of a virtual method's parameters, after `&self`.
pub fn arguments(method: &ImplItemFn) -> Result<Vec<Ident>> {
  method
    .sig
    .inputs
    .iter()
    .skip(1)
    .map(|arg| {
      let FnArg::Typed(arg) = arg else {
        return Err(Error::new_spanned(arg, "expected a named parameter"));
      };
      let Pat::Ident(pattern) = &*arg.pat else {
        return Err(Error::new_spanned(
          arg,
          "the parameters of virtual methods must be plain identifiers",
        ));
      };
      if pattern.by_ref.is_some() || pattern.subpat.is_some() || pattern.ident == "__this" {
        return Err(Error::new_spanned(pattern, "unsupported or reserved parameter pattern"));
      }
      Ok(pattern.ident.clone())
    })
    .collect()
}

/// The lifetime of the future returned by an async virtual method.
pub fn future_lifetime() -> syn::Lifetime {
  syn::Lifetime::new("'__class_future", Span::call_site())
}

/// Ties the elided and `'_` lifetimes of an async virtual method's parameters to its future.
pub struct FutureLifetime;

impl Fold for FutureLifetime {
  fn fold_type_reference(&mut self, mut ty: syn::TypeReference) -> syn::TypeReference {
    if ty.lifetime.is_none() {
      ty.lifetime = Some(future_lifetime());
    }
    fold::fold_type_reference(self, ty)
  }

  fn fold_lifetime(&mut self, lifetime: syn::Lifetime) -> syn::Lifetime {
    if lifetime.ident == "_" {
      future_lifetime()
    } else {
      lifetime
    }
  }

  // Function signatures have their own lifetime binders, unlike references captured by the future.
  fn fold_type_bare_fn(&mut self, ty: syn::TypeBareFn) -> syn::TypeBareFn {
    ty
  }

  fn fold_path_arguments(&mut self, args: PathArguments) -> PathArguments {
    if matches!(args, PathArguments::Parenthesized(_)) {
      args
    } else {
      fold::fold_path_arguments(self, args)
    }
  }
}
