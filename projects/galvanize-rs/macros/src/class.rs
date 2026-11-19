use proc_macro2::{Group, Span, TokenStream, TokenTree};
use quote::{ToTokens, format_ident, quote, quote_spanned};
use std::collections::HashSet;
use syn::{
  Attribute, Error, Fields, GenericParam, Generics, Ident, ItemStruct, Path, Result, Type, Visibility, WhereClause,
  ext::IdentExt, meta::ParseNestedMeta, parse::Parser, parse_quote, spanned::Spanned,
};

/// Names of methods the macro always generates, so fields can't use them.
const RESERVED_NAMES: &[&str] = &["from_data", "ptr_eq", "upcast"];

/// The name of the base class slot in `XData`, and of its getter.
const BASE_NAME: &str = "base";

/// Options given to `#[class(...)]` on the struct.
struct ClassOptions {
  krate: Path,
  strategy: Type,
  /// The class this one extends.
  extends: Option<Type>,
}

/// Options given to `#[class_prop(...)]` on a field.
#[derive(Default)]
struct FieldOptions {
  readonly: Option<Span>,
  strategy: Option<Type>,
}

/// A field of the class with its `#[class_prop]` attributes removed.
struct ClassField {
  ident: Ident,
  vis: Visibility,
  /// The field type, with `Self` replaced by the handle type.
  ty: Type,
  attrs: Vec<Attribute>,
  strategy: Type,
  setter_vis: Visibility,
  /// Names mentioned by the field type as written, i.e. before `Self` was replaced.
  ty_names: HashSet<String>,
  /// Names mentioned by the field's strategy.
  strategy_names: HashSet<String>,
}

/// The generics of each generated item.
struct ClassGenerics {
  /// For the storage struct, the handle and its impls: the struct's own generics plus a `Clone`
  /// bound for every type parameter used by a field type.
  full: Generics,
  /// For `XData`, which only has the parameters its field types use.
  data: Generics,
  /// `full`'s where clause plus a `Debug` bound for every type parameter used by a field type.
  debug_where: WhereClause,
}

/// The handle type, `Name<params..>`, which replaces `Self` in field types.
struct SelfType {
  ident: Ident,
  ty_generics: TokenStream,
}

pub fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
  let mut item: ItemStruct = syn::parse2(item)?;
  let options = parse_class_options(attr)?;
  validate_struct(&item)?;
  if let Some(extends) = &options.extends {
    validate_extends(extends, &item.generics)?;
  }

  let self_ty = SelfType {
    ident: item.ident.clone(),
    ty_generics: item.generics.split_for_impl().1.to_token_stream(),
  };
  let fields = take_fields(&mut item, &options, &self_ty)?;
  let generics = class_generics(&item.generics, &fields, options.extends.as_ref())?;
  Ok(generate(&item, &options, &generics, &fields))
}

fn parse_class_options(attr: TokenStream) -> Result<ClassOptions> {
  let mut krate = None;
  let mut strategy = None;
  let mut extends = None;

  syn::meta::parser(|meta| {
    if meta.path.is_ident("strategy") {
      set_once(&mut strategy, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("crate") {
      set_once(&mut krate, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("extends") {
      set_once(&mut extends, meta.value()?.parse()?, &meta)
    } else {
      Err(meta.error("unknown class option, expected `extends = ...`, `strategy = ...` or `crate = ...`"))
    }
  })
  .parse2(attr)?;

  let krate: Path = krate.unwrap_or_else(|| parse_quote!(::galvanize_rs));
  let strategy = strategy.unwrap_or_else(|| parse_quote!(#krate::DefaultStrategy));
  Ok(ClassOptions {
    krate,
    strategy,
    extends,
  })
}

/// Rejects base types that can never be a class, which would otherwise give confusing errors
/// from the generated impls.
fn validate_extends(extends: &Type, generics: &Generics) -> Result<()> {
  if names_in(extends.to_token_stream()).contains("Self") {
    return Err(Error::new_spanned(extends, "a class can't extend itself"));
  }
  if let Type::Path(path) = extends
    && path.qself.is_none()
    && let Some(ident) = path.path.get_ident()
    && generics.type_params().any(|p| p.ident == *ident)
  {
    return Err(Error::new_spanned(
      extends,
      "a class can't extend a type parameter, name a #[class] instead",
    ));
  }
  Ok(())
}

fn parse_field_options(attr: &Attribute) -> Result<FieldOptions> {
  let mut options = FieldOptions::default();

  attr.parse_nested_meta(|meta| {
    if meta.path.is_ident("readonly") {
      set_once(&mut options.readonly, meta.path.span(), &meta)
    } else if meta.path.is_ident("strategy") {
      set_once(&mut options.strategy, meta.value()?.parse()?, &meta)
    } else {
      Err(meta.error("unknown field option, expected `readonly` or `strategy = ...`"))
    }
  })?;

  Ok(options)
}

fn set_once<T>(slot: &mut Option<T>, value: T, meta: &ParseNestedMeta) -> Result<()> {
  if slot.is_some() {
    return Err(meta.error("duplicate option"));
  }
  *slot = Some(value);
  Ok(())
}

fn validate_struct(item: &ItemStruct) -> Result<()> {
  if !matches!(item.fields, Fields::Named(_)) {
    return Err(Error::new(
      item.ident.span(),
      "#[class] requires a struct with named fields",
    ));
  }
  Ok(())
}

/// Collects the fields, removing and applying their `#[class_prop(...)]` attributes.
fn take_fields(item: &mut ItemStruct, options: &ClassOptions, self_ty: &SelfType) -> Result<Vec<ClassField>> {
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
  let ident = field.ident.clone().expect("validated as a named field");
  let bare = ident.unraw().to_string();
  if RESERVED_NAMES.contains(&bare.as_str()) || (options.extends.is_some() && bare == BASE_NAME) {
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
      || (parsed.strategy.is_some() && merged.strategy.is_some());
    if duplicate {
      return Err(Error::new_spanned(attr, "duplicate option"));
    }
    merged.readonly = merged.readonly.or(parsed.readonly);
    merged.strategy = merged.strategy.or(parsed.strategy);
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

  let strategy = merged.strategy.unwrap_or_else(|| options.strategy.clone());
  Ok(ClassField {
    ident,
    vis: field.vis.clone(),
    ty: replace_self(&field.ty, self_ty)?,
    attrs,
    strategy_names: names_in(strategy.to_token_stream()),
    strategy: replace_self(&strategy, self_ty)?,
    setter_vis,
    ty_names: names_in(field.ty.to_token_stream()),
  })
}

/// Replaces every `Self` in `ty` with the handle type.
///
/// Inside the generated structs `Self` would otherwise refer to `XData` or the hidden storage
/// struct rather than to the class.
fn replace_self(ty: &Type, self_ty: &SelfType) -> Result<Type> {
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
fn names_in(tokens: TokenStream) -> HashSet<String> {
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

fn param_name(param: &GenericParam) -> String {
  match param {
    GenericParam::Type(p) => p.ident.to_string(),
    GenericParam::Lifetime(p) => p.lifetime.to_string(),
    GenericParam::Const(p) => p.ident.to_string(),
  }
}

/// Works out the generics of each generated item from the struct's generics.
fn class_generics(generics: &Generics, fields: &[ClassField], extends: Option<&Type>) -> Result<ClassGenerics> {
  let extends_names = extends.map(|ty| names_in(ty.to_token_stream())).unwrap_or_default();

  // Every parameter must reach the storage struct through a field type, strategy or the base
  // class, otherwise rustc reports it as unused on the hidden storage struct, which is confusing.
  // `Self` doesn't count as a use, as a parameter that's only used recursively is still unused.
  let used: HashSet<&String> = fields
    .iter()
    .flat_map(|f| f.ty_names.iter().chain(&f.strategy_names))
    .chain(&extends_names)
    .collect();
  let mut errors: Option<Error> = None;
  for param in &generics.params {
    let name = param_name(param);
    if !used.contains(&name) {
      let e = Error::new_spanned(
        param,
        format!("`{name}` isn't used by any field, strategy or base class"),
      );
      match errors.as_mut() {
        Some(errors) => errors.combine(e),
        None => errors = Some(e),
      }
    }
  }
  if let Some(errors) = errors {
    return Err(errors);
  }

  // Like `#[derive]`, bound the type parameters rather than the field types. Bounding a private
  // field type would trip the `private_bounds` lint on a public class. Parameters only used via
  // `Self` (`Option<Self>` is `Clone` regardless) or a strategy don't need the bounds. The base
  // class is treated like a field type, as its own fields bound its parameters with `Clone`.
  let in_fields = |ident: &Ident| fields.iter().any(|f| f.ty_names.contains(&ident.to_string()));
  let cloned: Vec<&Ident> = generics
    .type_params()
    .map(|p| &p.ident)
    .filter(|ident| in_fields(ident) || extends_names.contains(&ident.to_string()))
    .collect();

  let mut full = generics.clone();
  let where_clause = full.make_where_clause();
  for ident in &cloned {
    where_clause.predicates.push(parse_quote!(#ident: ::core::clone::Clone));
  }

  // Parameters only used by the base class get their `Debug` bounds via the base's `ClassDebug`
  // impl, added in `generate`.
  let mut debug_where = where_clause.clone();
  for ident in cloned.iter().filter(|ident| in_fields(ident)) {
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
  })
}

/// Returns `generics` with only the parameters named in `keep`, dropping any bound, where
/// predicate or default that mentions a removed parameter.
fn retain_params(generics: &Generics, keep: &HashSet<String>) -> Generics {
  let removed: HashSet<String> = generics
    .params
    .iter()
    .map(param_name)
    .filter(|name| !keep.contains(name))
    .collect();
  let mentions_removed = |tokens: &dyn ToTokens| names_in(tokens.to_token_stream()).iter().any(|n| removed.contains(n));

  let mut generics = generics.clone();
  generics.params = std::mem::take(&mut generics.params)
    .into_iter()
    .filter(|param| keep.contains(&param_name(param)))
    .map(|mut param| {
      match &mut param {
        GenericParam::Type(p) => {
          p.bounds = std::mem::take(&mut p.bounds)
            .into_iter()
            .filter(|b| !mentions_removed(b))
            .collect();
          if p.default.as_ref().is_some_and(|d| mentions_removed(d)) {
            p.eq_token = None;
            p.default = None;
          }
        }
        GenericParam::Lifetime(p) => {
          p.bounds = std::mem::take(&mut p.bounds)
            .into_iter()
            .filter(|b| !mentions_removed(b))
            .collect();
        }
        GenericParam::Const(p) => {
          if p.default.as_ref().is_some_and(|d| mentions_removed(d)) {
            p.eq_token = None;
            p.default = None;
          }
        }
      }
      param
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

fn generate(item: &ItemStruct, options: &ClassOptions, generics: &ClassGenerics, fields: &[ClassField]) -> TokenStream {
  let krate = &options.krate;
  let private = quote!(#krate::__private);
  let extends = options.extends.as_ref();
  let vis = &item.vis;
  let name = &item.ident;
  let data = format_ident!("{}Data", name);
  let storage = format_ident!("__{}Fields", name);
  let base = format_ident!("{}", BASE_NAME);
  // The base slot, if any, comes first in `XData`, the storage struct and `Debug`.
  let base_slot: Vec<&Ident> = extends.map(|_| &base).into_iter().collect();

  let (doc_attrs, data_attrs): (Vec<_>, Vec<_>) = item.attrs.iter().partition(|a| a.path().is_ident("doc"));
  let data_doc = format!("The initial field values of a [`{name}`].");

  let data_base = extends.map(|ty| {
    quote! {
      /// The base class part of the object, usually created by the base class's constructor.
      #vis #base: #ty,
    }
  });
  let data_fields = fields.iter().map(|f| {
    let (attrs, vis, ident, ty) = (&f.attrs, &f.vis, &f.ident, &f.ty);
    quote! { #(#attrs)* #vis #ident: #ty, }
  });

  let storage_base = extends.map(|ty| quote! { #base: #ty, });
  let storage_fields = fields.iter().map(|f| {
    let (ident, ty, strategy) = (&f.ident, &f.ty, &f.strategy);
    quote_spanned! {ty.span()=> #ident: <#strategy as #krate::Strategy>::Ptr<#ty>, }
  });

  let field_idents: Vec<_> = fields.iter().map(|f| &f.ident).collect();
  let accessors = fields.iter().map(|f| generate_accessors(krate, f));

  let debug_base = extends.map(|_| quote! { #private::ClassDebug::fmt_fields(&self.0.#base, d); });
  let debug_fields = fields.iter().map(|f| {
    let ident = &f.ident;
    let label = ident.unraw().to_string();
    quote! {
      d.field(#label, &#krate::MutablePointer::read_with(&self.0.#ident, ::core::clone::Clone::clone));
    }
  });
  let name_label = name.to_string();

  let full = &generics.full;
  let (impl_generics, ty_generics, where_clause) = full.split_for_impl();
  let data_generics = &generics.data;
  let data_where = &generics.data.where_clause;
  let data_ty_generics = generics.data.split_for_impl().1;
  let mut debug_where = generics.debug_where.clone();
  if let Some(ty) = extends {
    debug_where.predicates.push(parse_quote!(#ty: #private::ClassDebug));
  }

  let base_getter = extends.map(|ty| {
    quote! {
      /// Returns the handle of the base class part of the object.
      ///
      /// It's the same base object that `self` dereferences to, so changes made through either
      /// are visible through both.
      #vis fn #base(&self) -> #ty {
        ::core::clone::Clone::clone(&self.0.#base)
      }
    }
  });

  let subclass_impls = extends.map(|ty| {
    quote! {
      impl #impl_generics #private::SubClass for #name #ty_generics #where_clause {
        type Base = #ty;

        fn base_ref(&self) -> &#ty {
          &self.0.#base
        }
      }

      /// Gives access to the fields and methods of the base class.
      impl #impl_generics ::core::ops::Deref for #name #ty_generics #where_clause {
        type Target = #ty;

        fn deref(&self) -> &#ty {
          &self.0.#base
        }
      }

      /// Returns the handle of the base class part of the object.
      impl #impl_generics ::core::convert::From<#name #ty_generics> for #ty #where_clause {
        fn from(value: #name #ty_generics) -> Self {
          ::core::clone::Clone::clone(&value.0.#base)
        }
      }

      /// Returns the handle of the base class part of the object.
      impl #impl_generics ::core::convert::From<&#name #ty_generics> for #ty #where_clause {
        fn from(value: &#name #ty_generics) -> Self {
          ::core::clone::Clone::clone(&value.0.#base)
        }
      }
    }
  });

  quote! {
    #(#data_attrs)*
    #[doc = #data_doc]
    #[allow(dead_code)]
    #vis struct #data #data_generics #data_where {
      #data_base
      #(#data_fields)*
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    struct #storage #full #where_clause {
      #storage_base
      #(#storage_fields)*
    }

    #(#doc_attrs)*
    #vis struct #name #full (::std::sync::Arc<#storage #ty_generics>) #where_clause;

    #[allow(dead_code)]
    impl #impl_generics #name #ty_generics #where_clause {
      /// Creates a new object from its initial field values.
      fn from_data(data: #data #data_ty_generics) -> Self {
        let #data { #(#base_slot,)* #(#field_idents),* } = data;
        Self(::std::sync::Arc::new(#storage {
          #(#base_slot,)*
          #(#field_idents: #krate::MutablePointer::new(#field_idents),)*
        }))
      }

      /// Returns `true` if both handles refer to the same object.
      #vis fn ptr_eq(&self, other: &Self) -> bool {
        ::std::sync::Arc::ptr_eq(&self.0, &other.0)
      }

      /// Returns the handle of the object as a `B`, which is this class or any class it
      /// (transitively) extends. See `Extends`.
      #vis fn upcast<__B, __I>(&self) -> __B
      where
        Self: #krate::Extends<__B, __I>,
      {
        #krate::Extends::upcast(self)
      }

      #base_getter

      #(#accessors)*
    }

    impl #impl_generics #private::Class for #name #ty_generics #where_clause {}

    #subclass_impls

    /// Clones the handle, so both refer to the same object.
    impl #impl_generics ::core::clone::Clone for #name #ty_generics #where_clause {
      fn clone(&self) -> Self {
        Self(::std::sync::Arc::clone(&self.0))
      }
    }

    /// Compares by identity, see `ptr_eq`.
    impl #impl_generics ::core::cmp::PartialEq for #name #ty_generics #where_clause {
      fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
      }
    }

    impl #impl_generics ::core::cmp::Eq for #name #ty_generics #where_clause {}

    /// Hashes the identity of the object, consistent with `PartialEq`.
    impl #impl_generics ::core::hash::Hash for #name #ty_generics #where_clause {
      fn hash<__H: ::core::hash::Hasher>(&self, state: &mut __H) {
        ::core::ptr::hash(::std::sync::Arc::as_ptr(&self.0), state)
      }
    }

    impl #impl_generics #private::ClassDebug for #name #ty_generics #debug_where {
      #[allow(unused_variables)]
      fn fmt_fields(&self, d: &mut ::core::fmt::DebugStruct<'_, '_>) {
        #debug_base
        #(#debug_fields)*
      }
    }

    /// Prints the field values, including those of the base classes.
    impl #impl_generics ::core::fmt::Debug for #name #ty_generics #debug_where {
      fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        let mut d = f.debug_struct(#name_label);
        #private::ClassDebug::fmt_fields(self, &mut d);
        d.finish()
      }
    }

    /// Clones the handle, so both refer to the same object.
    impl #impl_generics ::core::convert::From<&#name #ty_generics> for #name #ty_generics #where_clause {
      fn from(value: &#name #ty_generics) -> Self {
        ::core::clone::Clone::clone(value)
      }
    }
  }
}

fn generate_accessors(krate: &Path, field: &ClassField) -> TokenStream {
  let ClassField {
    ident,
    vis,
    ty,
    attrs,
    setter_vis,
    ..
  } = field;
  let bare = ident.unraw();
  let setter = format_ident!("set_{}", bare);
  let updater = format_ident!("update_{}", bare);

  let docs: Vec<_> = attrs.iter().filter(|a| a.path().is_ident("doc")).collect();
  let getter_doc = if docs.is_empty() {
    let doc = format!("Returns the current value of `{bare}`.");
    quote!(#[doc = #doc])
  } else {
    quote!(#(#docs)*)
  };
  let setter_doc = format!("Replaces the value of `{bare}`.");
  let updater_doc = format!(
    "Modifies `{bare}` in place and returns the result of `f`.\n\n`f` may be called more than once, and must not \
     access `{bare}` itself."
  );

  quote! {
    #getter_doc
    #vis fn #ident(&self) -> #ty {
      #krate::MutablePointer::read_with(&self.0.#ident, ::core::clone::Clone::clone)
    }

    #[doc = #setter_doc]
    #setter_vis fn #setter(&self, value: impl ::core::convert::Into<#ty>) {
      let value: #ty = value.into();
      #krate::MutablePointer::write_with(&self.0.#ident, |v| *v = ::core::clone::Clone::clone(&value))
    }

    #[doc = #updater_doc]
    #setter_vis fn #updater<__R>(&self, f: impl ::core::ops::FnMut(&mut #ty) -> __R) -> __R {
      #krate::MutablePointer::write_with(&self.0.#ident, f)
    }
  }
}
