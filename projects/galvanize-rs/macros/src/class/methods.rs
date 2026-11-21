//! The `#[class_methods]` block: ordinary, virtual, abstract and overriding methods.

use super::fields::names_in;
use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, format_ident, quote};
use std::collections::{HashMap, HashSet};
use syn::{
  Attribute, Error, Expr, FnArg, GenericParam, Generics, Ident, ImplItem, ImplItemFn, ItemImpl, Meta, Pat, Path,
  PathArguments, Result, ReturnType, Signature, Token, Type, Visibility,
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
  /// The index, in the class's `implements`, of the trait named by its marker, e.g. `#[class_virtual(Trait)]`.
  pub named_trait: Option<usize>,
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
    is_constructor(&self.item.sig, class)
  }
}

/// See [`Method::is_constructor`].
pub fn is_constructor(sig: &Signature, class: &Ident) -> bool {
  if sig.ident == "new" {
    return true;
  }
  let ReturnType::Type(_, ty) = &sig.output else {
    return false;
  };
  let names = names_in(ty.to_token_stream());
  sig.receiver().is_none() && (names.contains("Self") || names.contains(&class.to_string()))
}

/// What sort of member a virtual member is.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MemberKind {
  /// A method taking `&self`.
  Instance,
  /// A class-level function, without a receiver.
  Static,
  /// A class-level constant.
  Const,
}

impl MemberKind {
  fn describe(self) -> &'static str {
    match self {
      MemberKind::Instance => "a method",
      MemberKind::Static => "a class-level function (without `self`)",
      MemberKind::Const => "a class-level constant",
    }
  }
}

/// A constant of a `#[class_methods]` block. Only an abstract one leaves out its value.
pub struct ConstItem {
  pub attrs: Vec<Attribute>,
  pub vis: Visibility,
  pub ident: Ident,
  pub ty: Type,
  pub expr: Option<Expr>,
}

impl ToTokens for ConstItem {
  fn to_tokens(&self, tokens: &mut TokenStream) {
    let Self {
      attrs,
      vis,
      ident,
      ty,
      expr,
    } = self;
    tokens.extend(quote!(#(#attrs)* #vis const #ident: #ty = #expr;));
  }
}

/// A class-level member's item.
pub enum StaticItem {
  Fn(ImplItemFn),
  Const(ConstItem),
}

/// A class-level member: a virtual, abstract or overriding associated function (without a receiver) or
/// constant, which is late-bound, see `statics`.
pub struct Static {
  /// `Virtual` for a declaration, `Override` for an override or implementation of an inherited one.
  pub kind: Kind,
  /// The item with its marker removed. An abstract function's empty block is a placeholder.
  pub item: StaticItem,
  pub is_abstract: bool,
  /// The index, in the class's `implements`, of the trait named by its marker.
  pub named_trait: Option<usize>,
  /// Whether an override can call `Self::super_x`, i.e. whether an ancestor implements the member.
  pub has_super: bool,
}

impl Static {
  pub fn name(&self) -> &Ident {
    match &self.item {
      StaticItem::Fn(item) => &item.sig.ident,
      StaticItem::Const(item) => &item.ident,
    }
  }

  pub fn vis(&self) -> &Visibility {
    match &self.item {
      StaticItem::Fn(item) => &item.vis,
      StaticItem::Const(item) => &item.vis,
    }
  }

  pub fn attrs(&self) -> &[Attribute] {
    match &self.item {
      StaticItem::Fn(item) => &item.attrs,
      StaticItem::Const(item) => &item.attrs,
    }
  }
}

/// The members of a `#[class_methods]` block.
pub struct Members {
  /// Instance methods, and ordinary associated functions.
  pub methods: Vec<Method>,
  /// Class-level members.
  pub statics: Vec<Static>,
  /// Ordinary constants, which are neither inherited nor late-bound.
  pub consts: Vec<ConstItem>,
}

/// A virtual member declared by an ancestor.
pub struct InheritedMethod {
  /// The index of the ancestor that declares it.
  pub index: usize,
  /// The name of the ancestor that declares it, for errors.
  pub class: String,
  pub is_async: bool,
  pub vis: Visibility,
  pub member: MemberKind,
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

/// Collects the members of a `#[class_methods]` block, checking them against the names the class
/// generates (`reserved`) and the virtual members its ancestors declare.
pub fn collect(block: ItemImpl, reserved: &mut HashSet<String>, context: &Context<'_>) -> Result<Members> {
  let mut members = Members {
    methods: Vec::new(),
    statics: Vec::new(),
    consts: Vec::new(),
  };
  let mut errors: Option<Error> = None;
  for item in block.items {
    match collect_one(item, reserved, context) {
      Ok(Member::Method(method)) => members.methods.push(method),
      Ok(Member::Static(member)) => members.statics.push(member),
      Ok(Member::Const(item)) => members.consts.push(item),
      Err(e) => match errors.as_mut() {
        Some(errors) => errors.combine(e),
        None => errors = Some(e),
      },
    }
  }
  if let Some(errors) = errors {
    return Err(errors);
  }
  for method in &members.methods {
    if method.has_super && reserved.contains(&format!("super_{}", method.name())) {
      return Err(Error::new_spanned(
        method.name(),
        format!("`super_{}` conflicts with another member", method.name()),
      ));
    }
  }
  Ok(members)
}

enum Member {
  Method(Method),
  Static(Static),
  Const(ConstItem),
}

/// A member of the block, before its marker is taken.
enum Parsed {
  Fn { item: ImplItemFn, has_body: bool },
  Const(ConstItem),
}

impl Parsed {
  fn attrs_mut(&mut self) -> &mut Vec<Attribute> {
    match self {
      Parsed::Fn { item, .. } => &mut item.attrs,
      Parsed::Const(item) => &mut item.attrs,
    }
  }

  fn ident(&self) -> &Ident {
    match self {
      Parsed::Fn { item, .. } => &item.sig.ident,
      Parsed::Const(item) => &item.ident,
    }
  }

  fn vis(&self) -> &Visibility {
    match self {
      Parsed::Fn { item, .. } => &item.vis,
      Parsed::Const(item) => &item.vis,
    }
  }

  fn has_body(&self) -> bool {
    match self {
      Parsed::Fn { has_body, .. } => *has_body,
      Parsed::Const(item) => item.expr.is_some(),
    }
  }

  fn member(&self) -> MemberKind {
    match self {
      Parsed::Fn { item, .. } if item.sig.receiver().is_some() => MemberKind::Instance,
      Parsed::Fn { .. } => MemberKind::Static,
      Parsed::Const(_) => MemberKind::Const,
    }
  }
}

fn parse_item(item: ImplItem) -> Result<Parsed> {
  match item {
    ImplItem::Fn(item) => Ok(Parsed::Fn { item, has_body: true }),
    ImplItem::Const(item) => {
      if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(Error::new_spanned(&item.generics, "class constants can't be generic"));
      }
      Ok(Parsed::Const(ConstItem {
        attrs: item.attrs,
        vis: item.vis,
        ident: item.ident,
        ty: item.ty,
        expr: Some(item.expr),
      }))
    }
    ImplItem::Verbatim(tokens) => parse_bodyless(tokens),
    other => Err(Error::new_spanned(
      other,
      "#[class_methods] only supports methods, associated functions and constants",
    )),
  }
}

/// A method's `#[class_virtual]`, `#[class_abstract]` or `#[class_override]` marker.
struct Marker {
  kind: Kind,
  is_abstract: bool,
  /// The trait named by `#[class_virtual(Trait)]` or `#[class_abstract(Trait)]`.
  implements: Option<Path>,
}

fn collect_one(item: ImplItem, reserved: &mut HashSet<String>, context: &Context<'_>) -> Result<Member> {
  let mut parsed = parse_item(item)?;
  let marker = take_marker(parsed.attrs_mut())?;
  let member = parsed.member();
  let ident = parsed.ident().clone();
  let label = ident.to_string();
  let noun = if member == MemberKind::Const {
    "constant"
  } else {
    "method"
  };
  let inherited = context.inherited.get(&label);
  // A member named after an inherited virtual member overrides it, so it can't shadow it by accident. As in
  // Scala, replacing an implementation must be marked, while implementing an abstract member needn't be
  // (a misspelt implementation leaves the abstract member unimplemented, which is an error anyway).
  let kind = match (marker.kind, inherited) {
    (Kind::Ordinary, Some(_)) if context.unimplemented.contains(&label) => Kind::Override,
    (Kind::Ordinary, Some(declared)) => {
      return Err(Error::new_spanned(
        &ident,
        format!(
          "`{label}` would override `{}::{label}`, mark it #[class_override] or rename it",
          declared.class
        ),
      ));
    }
    (kind, _) => kind,
  };
  let has_body = parsed.has_body();
  if !has_body && !marker.is_abstract {
    return Err(match &parsed {
      Parsed::Fn { item, .. } => {
        Error::new_spanned(&item.sig, "only #[class_abstract] methods can leave out their body")
      }
      Parsed::Const(_) => Error::new_spanned(&ident, "only #[class_abstract] constants can leave out their value"),
    });
  }
  if has_body && marker.is_abstract {
    return Err(match &parsed {
      Parsed::Fn { item, .. } => Error::new_spanned(
        &item.block,
        "an abstract method has no body, end its signature with `;`",
      ),
      Parsed::Const(item) => Error::new_spanned(&item.expr, "an abstract constant has no value, end it with `;`"),
    });
  }
  if marker.is_abstract && !context.is_abstract {
    return Err(Error::new_spanned(
      &ident,
      format!("only abstract classes can declare abstract {noun}s, declare the class with #[class(abstract)]"),
    ));
  }
  if label.starts_with("__") || !reserved.insert(label.clone()) {
    return Err(Error::new_spanned(
      &ident,
      format!("this {noun}'s name conflicts with a member generated by #[class]"),
    ));
  }
  if kind == Kind::Ordinary {
    return Ok(match parsed {
      Parsed::Fn { item, .. } => Member::Method(Method {
        kind,
        item,
        is_abstract: false,
        named_trait: None,
        has_super: false,
      }),
      Parsed::Const(item) => Member::Const(item),
    });
  }
  if let Some(declared) = inherited
    && declared.member != member
  {
    return Err(Error::new_spanned(
      &ident,
      format!(
        "`{}::{label}` is {}, so this must be one too",
        declared.class,
        declared.member.describe()
      ),
    ));
  }
  let is_async = match &mut parsed {
    Parsed::Fn { item, .. } if member == MemberKind::Instance => {
      check_dispatchable(item)?;
      check_virtual_signature(item)?;
      item.sig.asyncness.is_some()
    }
    Parsed::Fn { item, .. } => {
      check_static_signature(item)?;
      item.sig.asyncness.is_some()
    }
    Parsed::Const(_) => false,
  };
  let named_trait = match (kind, inherited) {
    (Kind::Virtual, Some(_)) => {
      return Err(Error::new_spanned(
        &ident,
        format!("an ancestor already declares this virtual {noun}, leave out the marker to override it"),
      ));
    }
    (Kind::Override, None) => {
      return Err(Error::new_spanned(
        &ident,
        format!("no ancestor declares a virtual {noun} with this name"),
      ));
    }
    (Kind::Override, Some(declared)) if declared.is_async != is_async => {
      let span = match &parsed {
        Parsed::Fn { item, .. } => item.sig.to_token_stream(),
        Parsed::Const(_) => ident.to_token_stream(),
      };
      return Err(Error::new_spanned(
        span,
        "an override must match whether the inherited method is async",
      ));
    }
    (Kind::Override, Some(declared)) => {
      check_override_visibility(parsed.vis(), &ident, &declared.vis)?;
      None
    }
    _ => named_trait(parsed.vis(), marker.implements.as_ref(), context.implements)?,
  };
  let has_super = kind == Kind::Override && !context.unimplemented.contains(&label);
  let is_abstract = marker.is_abstract;
  Ok(match (parsed, member) {
    (Parsed::Fn { item, .. }, MemberKind::Instance) => Member::Method(Method {
      kind,
      has_super,
      item,
      is_abstract,
      named_trait,
    }),
    (Parsed::Fn { item, .. }, _) => Member::Static(Static {
      kind,
      item: StaticItem::Fn(item),
      is_abstract,
      named_trait,
      has_super,
    }),
    (Parsed::Const(item), _) => Member::Static(Static {
      kind,
      item: StaticItem::Const(item),
      is_abstract,
      named_trait,
      has_super,
    }),
  })
}

/// Parses a method without a body, `fn name(&self);`, or a constant without a value, `const X: T;`, which
/// `syn` leaves unparsed.
fn parse_bodyless(tokens: TokenStream) -> Result<Parsed> {
  let parser = |input: ParseStream<'_>| {
    let attrs = input.call(Attribute::parse_outer)?;
    let vis = input.parse()?;
    if input.peek(Token![const]) && !input.peek2(Token![fn]) && !input.peek2(Token![unsafe]) {
      input.parse::<Token![const]>()?;
      let ident = input.parse()?;
      input.parse::<Token![:]>()?;
      let ty = input.parse()?;
      input.parse::<Token![;]>()?;
      return Ok(Parsed::Const(ConstItem {
        attrs,
        vis,
        ident,
        ty,
        expr: None,
      }));
    }
    let sig: Signature = input.parse()?;
    input.parse::<Token![;]>()?;
    Ok(Parsed::Fn {
      item: ImplItemFn {
        attrs,
        vis,
        defaultness: None,
        sig,
        block: parse_quote!({}),
      },
      has_body: false,
    })
  };
  parser.parse2(tokens.clone()).map_err(|_| {
    Error::new_spanned(
      tokens,
      "#[class_methods] only supports methods, associated functions and constants",
    )
  })
}

/// Removes a member's `#[class_virtual]`, `#[class_abstract]` or `#[class_override]` marker.
fn take_marker(attrs: &mut Vec<Attribute>) -> Result<Marker> {
  let mut marker = Marker {
    kind: Kind::Ordinary,
    is_abstract: false,
    implements: None,
  };
  let mut found = false;
  let mut retained = Vec::new();
  for attr in attrs.iter() {
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
  *attrs = retained;
  Ok(marker)
}

/// Checks the signature of a class-level function, which is dispatched statically, through a trait.
fn check_static_signature(method: &ImplItemFn) -> Result<()> {
  let sig = &method.sig;
  if sig.constness.is_some() || sig.unsafety.is_some() || sig.abi.is_some() || sig.variadic.is_some() {
    return Err(Error::new_spanned(
      sig,
      "class-level functions can't be `const`, `unsafe` or `extern`",
    ));
  }
  Ok(())
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

/// Requires an override to repeat the visibility of the member it overrides.
fn check_override_visibility(vis: &Visibility, ident: &Ident, declared: &Visibility) -> Result<()> {
  let written = vis.to_token_stream().to_string();
  let expected = declared.to_token_stream().to_string();
  if written == expected {
    return Ok(());
  }
  let message = if expected.is_empty() {
    "an override must have the same visibility as the method it overrides, which has none".to_string()
  } else {
    format!("an override must have the same visibility as the method it overrides, `{expected}`")
  };
  if matches!(vis, Visibility::Inherited) {
    Err(Error::new_spanned(ident, message))
  } else {
    Err(Error::new_spanned(vis, message))
  }
}

/// Finds the trait a virtual member's marker names, e.g. `#[class_virtual(Trait)]`, among the class's
/// `implements` traits. Members that don't name one are matched to traits later, by `__implement`.
fn named_trait(vis: &Visibility, named: Option<&Path>, implements: &[Path]) -> Result<Option<usize>> {
  let Some(named) = named else {
    return Ok(None);
  };
  if !matches!(vis, Visibility::Inherited) {
    return Err(Error::new_spanned(
      vis,
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

/// A signature with plain parameter names, for an item that passes its arguments on to another.
pub struct Forward {
  /// The signature, without `const`, `mut` or patterns, which a function without a body can't have.
  pub sig: Signature,
  /// The receiver, if any.
  pub receiver: Option<Token![self]>,
  pub args: Vec<Ident>,
  /// The explicit generic arguments to pass on, which can't be given alongside `impl Trait` parameters
  /// (which infer them anyway).
  pub turbofish: Option<TokenStream>,
}

impl Forward {
  pub fn new(sig: &Signature) -> Self {
    let mut sig = sig.clone();
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
          let ident = match &*arg.pat {
            Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => pat.ident.clone(),
            _ => format_ident!("__arg{}", index),
          };
          *arg.pat = parse_quote!(#ident);
          args.push(ident);
        }
      }
    }
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
    Self {
      sig,
      receiver,
      args,
      turbofish,
    }
  }

  /// A call of `function` (e.g. `<Self as Trait>::name`), with the receiver, if any, and the arguments,
  /// awaited if the signature is async.
  pub fn call(&self, function: TokenStream) -> TokenStream {
    self.call_with(function, self.receiver.map(|r| quote!(#r)))
  }

  /// A call of `function`, passing `first` (e.g. a view of the receiver) before the arguments.
  pub fn call_with(&self, function: TokenStream, first: Option<TokenStream>) -> TokenStream {
    let Self {
      args, turbofish, sig, ..
    } = self;
    let first = first.map(|first| quote!(#first,));
    let mut call = quote!(#function #turbofish(#first #(#args),*));
    if sig.asyncness.is_some() {
      call = quote!(#call.await);
    }
    if sig.unsafety.is_some() {
      call = quote!(unsafe { #call });
    }
    call
  }
}

fn has_impl_trait(sig: &Signature) -> bool {
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
