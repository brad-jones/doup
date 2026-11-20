//! The generic parameters of a class and of each generated item.

use super::fields::{ClassField, names_in};
use quote::ToTokens;
use std::collections::HashSet;
use syn::{Error, GenericParam, Generics, Ident, Result, Type, WhereClause, parse_quote};

pub struct ClassGenerics {
  /// For the storage, the handle and their impls: the struct's own generics plus the bounds every
  /// class needs.
  pub full: Generics,
  /// For `XData`, which only has the parameters its field types and parent use.
  pub data: Generics,
  /// `full`'s where clause plus a `Debug` bound for every type parameter used by a field type.
  pub debug_where: WhereClause,
  /// The names of the type parameters, in order.
  pub params: Vec<Ident>,
}

/// Works out the generics of each generated item from the struct's generics.
pub fn class_generics(generics: &Generics, fields: &[ClassField], extends: Option<&Type>) -> Result<ClassGenerics> {
  let extends_names = extends.map(|ty| names_in(ty.to_token_stream())).unwrap_or_default();

  // Every parameter must reach the storage through a field type, strategy or the parent, otherwise rustc
  // reports it as unused on the hidden storage, which is confusing. `Self` doesn't count as a use, as a
  // parameter that's only used recursively is still unused.
  let used: HashSet<&String> = fields
    .iter()
    .flat_map(|f| f.ty_names.iter().chain(&f.strategy_names))
    .chain(&extends_names)
    .collect();
  let mut errors: Option<Error> = None;
  let mut push = |e: Error| match errors.as_mut() {
    Some(errors) => errors.combine(e),
    None => errors = Some(e),
  };
  for param in &generics.params {
    match param {
      GenericParam::Type(p) if p.ident.to_string().starts_with("__") => push(Error::new_spanned(
        p,
        "type parameter names beginning with `__` are reserved by #[class]",
      )),
      GenericParam::Type(p) if !used.contains(&p.ident.to_string()) => push(Error::new_spanned(
        p,
        format!(
          "`{}` isn't used by any field, pointer strategy or parent class",
          p.ident
        ),
      )),
      GenericParam::Type(_) => {}
      other => push(Error::new_spanned(
        other,
        "#[class] supports type parameters, not lifetime or const parameters",
      )),
    }
  }
  if let Some(errors) = errors {
    return Err(errors);
  }

  let params: Vec<Ident> = generics.type_params().map(|p| p.ident.clone()).collect();
  let in_fields = |ident: &Ident| fields.iter().any(|f| f.ty_names.contains(&ident.to_string()));

  // Objects are shared between threads and identified by `TypeId`, so every parameter is `'static` and
  // every value stored is `Send + Sync`. Like `#[derive]`, bound the parameters rather than the field
  // types, as bounding a private field type would trip the `private_bounds` lint. Parameters only used by
  // a pointer strategy aren't values, so they only need to be `'static`.
  let mut full = generics.clone();
  let where_clause = full.make_where_clause();
  for ident in &params {
    if in_fields(ident) || extends_names.contains(&ident.to_string()) {
      where_clause.predicates.push(parse_quote!(
        #ident: ::core::clone::Clone + ::core::marker::Send + ::core::marker::Sync + 'static
      ));
    } else {
      where_clause.predicates.push(parse_quote!(#ident: 'static));
    }
  }
  // Parameters only used by the parent get their `Debug` bounds via the parent's `ClassDebug` impl.
  let mut debug_where = where_clause.clone();
  for ident in params.iter().filter(|ident| in_fields(ident)) {
    debug_where.predicates.push(parse_quote!(#ident: ::core::fmt::Debug));
  }

  let data_names: HashSet<String> = fields
    .iter()
    .flat_map(|f| names_in(f.ty.to_token_stream()))
    .chain(extends_names.iter().cloned())
    .collect();
  let data = retain_params(&full, &data_names);

  Ok(ClassGenerics {
    full,
    data,
    debug_where,
    params,
  })
}

/// Returns `generics` with only the type parameters named in `keep`, dropping any bound, where
/// predicate or default that mentions a removed parameter.
fn retain_params(generics: &Generics, keep: &HashSet<String>) -> Generics {
  let removed: HashSet<String> = generics
    .type_params()
    .map(|p| p.ident.to_string())
    .filter(|name| !keep.contains(name))
    .collect();
  let mentions_removed = |tokens: &dyn ToTokens| names_in(tokens.to_token_stream()).iter().any(|n| removed.contains(n));

  let mut generics = generics.clone();
  generics.params = std::mem::take(&mut generics.params)
    .into_iter()
    .filter_map(|param| match param {
      GenericParam::Type(mut p) if keep.contains(&p.ident.to_string()) => {
        p.bounds = std::mem::take(&mut p.bounds)
          .into_iter()
          .filter(|b| !mentions_removed(b))
          .collect();
        if p.default.as_ref().is_some_and(|d| mentions_removed(d)) {
          p.eq_token = None;
          p.default = None;
        }
        Some(GenericParam::Type(p))
      }
      _ => None,
    })
    .collect();
  if let Some(where_clause) = generics.where_clause.as_mut() {
    where_clause.predicates = std::mem::take(&mut where_clause.predicates)
      .into_iter()
      .filter(|p| !mentions_removed(p))
      .collect();
  }
  generics
}
