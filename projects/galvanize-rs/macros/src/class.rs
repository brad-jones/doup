use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote, quote_spanned};
use syn::{
  Attribute, Error, Fields, Ident, ItemStruct, Path, Result, Type, Visibility, ext::IdentExt, meta::ParseNestedMeta,
  parse::Parser, parse_quote, spanned::Spanned,
};

/// Names of methods the macro always generates, so fields can't use them.
const RESERVED_NAMES: &[&str] = &["from_data", "ptr_eq"];

/// Options given to `#[class(...)]` on the struct.
struct ClassOptions {
  krate: Path,
  strategy: Type,
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
  ty: Type,
  attrs: Vec<Attribute>,
  strategy: Type,
  setter_vis: Visibility,
}

pub fn expand(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
  let mut item: ItemStruct = syn::parse2(item)?;
  let options = parse_class_options(attr)?;
  validate_struct(&item)?;

  let fields = take_fields(&mut item, &options)?;
  Ok(generate(&item, &options, &fields))
}

fn parse_class_options(attr: TokenStream) -> Result<ClassOptions> {
  let mut krate = None;
  let mut strategy = None;

  syn::meta::parser(|meta| {
    if meta.path.is_ident("strategy") {
      set_once(&mut strategy, meta.value()?.parse()?, &meta)
    } else if meta.path.is_ident("crate") {
      set_once(&mut krate, meta.value()?.parse()?, &meta)
    } else {
      Err(meta.error("unknown class option, expected `strategy = ...` or `crate = ...`"))
    }
  })
  .parse2(attr)?;

  let krate: Path = krate.unwrap_or_else(|| parse_quote!(::galvanize_rs));
  let strategy = strategy.unwrap_or_else(|| parse_quote!(#krate::DefaultStrategy));
  Ok(ClassOptions { krate, strategy })
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
  if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
    return Err(Error::new_spanned(&item.generics, "generic classes are not supported"));
  }
  if !matches!(item.fields, Fields::Named(_)) {
    return Err(Error::new(
      item.ident.span(),
      "#[class] requires a struct with named fields",
    ));
  }
  Ok(())
}

/// Collects the fields, removing and applying their `#[class_prop(...)]` attributes.
fn take_fields(item: &mut ItemStruct, options: &ClassOptions) -> Result<Vec<ClassField>> {
  let mut errors: Option<Error> = None;
  let mut fields = Vec::new();

  for field in item.fields.iter_mut() {
    match take_field(field, options) {
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

fn take_field(field: &mut syn::Field, options: &ClassOptions) -> Result<ClassField> {
  let ident = field.ident.clone().expect("validated as a named field");
  if RESERVED_NAMES.contains(&ident.unraw().to_string().as_str()) {
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

  Ok(ClassField {
    ident,
    vis: field.vis.clone(),
    ty: field.ty.clone(),
    attrs,
    strategy: merged.strategy.unwrap_or_else(|| options.strategy.clone()),
    setter_vis,
  })
}

fn generate(item: &ItemStruct, options: &ClassOptions, fields: &[ClassField]) -> TokenStream {
  let krate = &options.krate;
  let vis = &item.vis;
  let name = &item.ident;
  let data = format_ident!("{}Data", name);
  let storage = format_ident!("__{}Fields", name);

  let (doc_attrs, data_attrs): (Vec<_>, Vec<_>) = item.attrs.iter().partition(|a| a.path().is_ident("doc"));
  let data_doc = format!("The initial field values of a [`{name}`].");

  let data_fields = fields.iter().map(|f| {
    let (attrs, vis, ident, ty) = (&f.attrs, &f.vis, &f.ident, &f.ty);
    quote! { #(#attrs)* #vis #ident: #ty, }
  });

  let storage_fields = fields.iter().map(|f| {
    let (ident, ty, strategy) = (&f.ident, &f.ty, &f.strategy);
    quote_spanned! {ty.span()=> #ident: <#strategy as #krate::Strategy>::Ptr<#ty>, }
  });

  let field_idents: Vec<_> = fields.iter().map(|f| &f.ident).collect();
  let accessors = fields.iter().map(|f| generate_accessors(krate, f));

  let debug_fields = fields.iter().map(|f| {
    let ident = &f.ident;
    let label = ident.unraw().to_string();
    quote! {
      .field(#label, &#krate::MutablePointer::read_with(&self.0.#ident, ::core::clone::Clone::clone))
    }
  });
  let name_label = name.to_string();

  quote! {
    #(#data_attrs)*
    #[doc = #data_doc]
    #[allow(dead_code)]
    #vis struct #data {
      #(#data_fields)*
    }

    #[doc(hidden)]
    #[allow(dead_code)]
    struct #storage {
      #(#storage_fields)*
    }

    #(#doc_attrs)*
    #[derive(Clone)]
    #vis struct #name(::std::sync::Arc<#storage>);

    #[allow(dead_code)]
    impl #name {
      /// Creates a new object from its initial field values.
      fn from_data(data: #data) -> Self {
        let #data { #(#field_idents),* } = data;
        Self(::std::sync::Arc::new(#storage {
          #(#field_idents: #krate::MutablePointer::new(#field_idents),)*
        }))
      }

      /// Returns `true` if both handles refer to the same object.
      #vis fn ptr_eq(&self, other: &Self) -> bool {
        ::std::sync::Arc::ptr_eq(&self.0, &other.0)
      }

      #(#accessors)*
    }

    /// Compares by identity, see `ptr_eq`.
    impl ::core::cmp::PartialEq for #name {
      fn eq(&self, other: &Self) -> bool {
        self.ptr_eq(other)
      }
    }

    impl ::core::cmp::Eq for #name {}

    /// Hashes the identity of the object, consistent with `PartialEq`.
    impl ::core::hash::Hash for #name {
      fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
        ::core::ptr::hash(::std::sync::Arc::as_ptr(&self.0), state)
      }
    }

    impl ::core::fmt::Debug for #name {
      fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        f.debug_struct(#name_label)
          #(#debug_fields)*
          .finish()
      }
    }

    /// Clones the handle, so both refer to the same object.
    impl ::core::convert::From<&#name> for #name {
      fn from(value: &#name) -> Self {
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
    #setter_vis fn #updater<R>(&self, f: impl ::core::ops::FnMut(&mut #ty) -> R) -> R {
      #krate::MutablePointer::write_with(&self.0.#ident, f)
    }
  }
}
