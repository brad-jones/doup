//! The fields of a class and their `#[class_prop(...)]` options.

use super::options::{ClassOptions, set_once};
use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, quote_spanned};
use std::collections::HashSet;
use syn::{Attribute, Error, Ident, ItemStruct, Result, Type, Visibility, ext::IdentExt, spanned::Spanned};

/// Names of the members the macros always generate, so fields can't use them.
pub const RESERVED_NAMES: &[&str] = &["from_data", "ptr_eq", "upcast", "is", "downcast"];

/// The name of the parent's slot in a subclass's `XData` and storage.
pub const BASE_NAME: &str = "base";

/// Options given to `#[class_prop(...)]` on a field.
#[derive(Default)]
struct FieldOptions {
  readonly: Option<Span>,
  pointer_strategy: Option<Type>,
}

/// A field of the class with its `#[class_prop]` attributes removed.
pub struct ClassField {
  pub ident: Ident,
  pub vis: Visibility,
  /// The field type, with `Self` replaced by the handle type.
  pub ty: Type,
  /// The field's attributes other than `class_prop`, applied to `XData`'s field.
  pub attrs: Vec<Attribute>,
  pub pointer_strategy: Type,
  pub setter_vis: Visibility,
  /// Names mentioned by the field type as written, i.e. before `Self` was replaced.
  pub ty_names: HashSet<String>,
  /// Names mentioned by the field's pointer strategy.
  pub strategy_names: HashSet<String>,
}

impl ClassField {
  /// The field's name without any `r#` prefix, used for accessor names.
  pub fn bare(&self) -> Ident {
    self.ident.unraw()
  }
}

/// The handle type, `Name<params..>`, which replaces `Self` in field types.
pub struct SelfType {
  pub ident: Ident,
  pub ty_generics: TokenStream,
}

/// Collects the fields, removing and applying their `#[class_prop(...)]` attributes.
pub fn take_fields(item: &mut ItemStruct, options: &ClassOptions, self_ty: &SelfType) -> Result<Vec<ClassField>> {
  let mut errors: Option<Error> = None;
  let mut fields = Vec::new();

  for field in item.fields.iter_mut() {
    match take_field(field, options, self_ty) {
      Ok(f) => fields.push(f),
      Err(e) => match errors.as_mut() {
        Some(errors) => errors.combine(e),
        None => errors = Some(e),
      },
    }
  }

  match errors {
    Some(errors) => Err(errors),
    None => Ok(fields),
  }
}

fn take_field(field: &mut syn::Field, options: &ClassOptions, self_ty: &SelfType) -> Result<ClassField> {
  let ident = field
    .ident
    .clone()
    .ok_or_else(|| Error::new_spanned(&*field, "expected a named field"))?;
  let bare = ident.unraw().to_string();
  if RESERVED_NAMES.contains(&bare.as_str())
    || (options.extends.is_some() && bare == BASE_NAME)
    || bare.starts_with("__")
  {
    return Err(Error::new(
      ident.span(),
      format!("`{ident}` is reserved by #[class], rename the field"),
    ));
  }

  if let Some(attr) = field.attrs.iter().find(|a| a.path().is_ident("class")) {
    return Err(Error::new_spanned(
      attr,
      "#[class] can't be used on a field, use #[class_prop(...)] instead",
    ));
  }

  let (prop_attrs, attrs): (Vec<_>, Vec<_>) = field.attrs.drain(..).partition(|a| a.path().is_ident("class_prop"));
  // Options may be split across several `#[class_prop(...)]` attributes, but each may only be given once.
  let mut merged = FieldOptions::default();
  for attr in &prop_attrs {
    let parsed = parse_field_options(attr)?;
    let duplicate = (parsed.readonly.is_some() && merged.readonly.is_some())
      || (parsed.pointer_strategy.is_some() && merged.pointer_strategy.is_some());
    if duplicate {
      return Err(Error::new_spanned(attr, "duplicate option"));
    }
    merged.readonly = merged.readonly.or(parsed.readonly);
    merged.pointer_strategy = merged.pointer_strategy.or(parsed.pointer_strategy);
  }

  let setter_vis = match merged.readonly {
    Some(span) if matches!(field.vis, Visibility::Inherited) => {
      return Err(Error::new(
        span,
        "`readonly` has no effect on a private field, its setter is already private",
      ));
    }
    Some(_) => Visibility::Inherited,
    None => field.vis.clone(),
  };

  let pointer_strategy = merged
    .pointer_strategy
    .unwrap_or_else(|| options.pointer_strategy.clone());
  Ok(ClassField {
    ident,
    vis: field.vis.clone(),
    ty: replace_self(&field.ty, self_ty)?,
    attrs,
    strategy_names: names_in(pointer_strategy.to_token_stream()),
    pointer_strategy: replace_self(&pointer_strategy, self_ty)?,
    setter_vis,
    ty_names: names_in(field.ty.to_token_stream()),
  })
}

fn parse_field_options(attr: &Attribute) -> Result<FieldOptions> {
  let mut options = FieldOptions::default();

  attr.parse_nested_meta(|meta| {
    if meta.path.is_ident("readonly") {
      set_once(&mut options.readonly, meta.path.span(), &meta)
    } else if meta.path.is_ident("pointer_strategy") {
      set_once(&mut options.pointer_strategy, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("strategy") {
      Err(meta.error("`strategy` was renamed to `pointer_strategy`"))
    } else {
      Err(meta.error("unknown field option, expected `readonly` or `pointer_strategy = ...`"))
    }
  })?;

  Ok(options)
}

/// Replaces every `Self` in `ty` with the handle type.
///
/// Inside the generated structs `Self` would otherwise refer to `XData` or the hidden storage struct
/// rather than to the class.
pub fn replace_self(ty: &Type, self_ty: &SelfType) -> Result<Type> {
  fn replace(tokens: TokenStream, self_ty: &SelfType) -> TokenStream {
    tokens
      .into_iter()
      .map(|tree| match tree {
        TokenTree::Ident(ident) if ident == "Self" => {
          let mut name = self_ty.ident.clone();
          name.set_span(ident.span());
          let ty_generics = &self_ty.ty_generics;
          quote_spanned!(ident.span()=> #name #ty_generics)
        }
        TokenTree::Group(group) => {
          let mut new = Group::new(group.delimiter(), replace(group.stream(), self_ty));
          new.set_span(group.span());
          TokenTree::Group(new).into()
        }
        other => other.into(),
      })
      .collect()
  }

  syn::parse2(replace(ty.to_token_stream(), self_ty))
}

/// Returns every identifier and lifetime (as `'a`) in `tokens`, including those nested in groups.
///
/// This is a syntactic scan, so it also finds names inside macro invocations such as `ty!(T)`.
pub fn names_in(tokens: TokenStream) -> HashSet<String> {
  fn collect(tokens: TokenStream, names: &mut HashSet<String>) {
    let mut iter = tokens.into_iter().peekable();
    while let Some(tree) = iter.next() {
      match tree {
        TokenTree::Ident(ident) => {
          names.insert(ident.to_string());
        }
        TokenTree::Punct(punct) if punct.as_char() == '\'' => {
          if let Some(TokenTree::Ident(ident)) = iter.peek() {
            names.insert(format!("'{ident}"));
            iter.next();
          }
        }
        TokenTree::Group(group) => collect(group.stream(), names),
        _ => {}
      }
    }
  }

  let mut names = HashSet::new();
  collect(tokens, &mut names);
  names
}
