//! `#[class]` and `#[class_methods]`.
//!
//! The two attributes cooperate through a local `macro_rules!` helper: `#[class]` records the struct in a
//! helper named after it, and `#[class_methods]` invokes that helper with the methods block. The helper
//! then calls `__complete`, which has both items. A root class is generated straight away. A subclass
//! invokes its parent's descriptor (see [`descriptor`]), which calls `__inherit` with the parent's
//! ancestry. Either way, the class's runtime strategy picks the generator.

mod common;
mod descriptor;
mod fields;
mod generics;
mod methods;
mod options;
mod safe_runtime;
mod unsafe_runtime;

use descriptor::{Ancestor, Inheritance, Input, Substitute, VirtualMethod};
use fields::{ClassField, RESERVED_NAMES, SelfType, names_in};
use generics::ClassGenerics;
use methods::{Inherited, Kind, Method};
use options::{ClassOptions, Runtime, parse_class_options};
use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use std::collections::HashSet;
use syn::{
  Attribute, Error, Fields, GenericArgument, Generics, Ident, ItemImpl, ItemStruct, Path, PathArguments, Result, Type,
  Visibility, parse_quote,
};

/// The helper `#[class]` defines and `#[class_methods]` invokes.
fn helper_name(class: &Ident) -> Ident {
  format_ident!("__class_methods_for_{}", class)
}

/// A marker type that only implements `HasClassMethods` once `#[class_methods]` has run, so a missing
/// methods block is a clear error rather than a silently missing class.
fn marker_name(class: &Ident) -> Ident {
  format_ident!("__ClassMethodsOf{}", class, span = class.span())
}

pub fn class(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
  let item: ItemStruct = syn::parse2(item)?;
  let helper = helper_name(&item.ident);
  let options = match parse_class_options(attr.clone()).and_then(|options| {
    validate_declaration(&options, &item)?;
    Ok(options)
  }) {
    Ok(options) => options,
    Err(error) => {
      let error = error.into_compile_error();
      // Swallow the methods block, so the error isn't followed by others caused by the missing class.
      return Ok(quote! {
        #error
        macro_rules! #helper {
          ($methods:item) => {};
        }
      });
    }
  };
  let krate = &options.krate;
  let marker = marker_name(&item.ident);
  Ok(quote! {
    macro_rules! #helper {
      ($methods:item) => {
        #krate::__private::__complete! { [#attr] #item $methods }
      };
    }

    #[doc(hidden)]
    struct #marker;
    const _: () = #krate::__private::require_class_methods::<#marker>();
  })
}

pub fn class_methods(attr: TokenStream, item: TokenStream) -> Result<TokenStream> {
  if !attr.is_empty() {
    return Err(Error::new_spanned(attr, "#[class_methods] doesn't take any options"));
  }
  let item: ItemImpl = syn::parse2(item)?;
  let Type::Path(path) = &*item.self_ty else {
    return Err(Error::new_spanned(
      &item.self_ty,
      "#[class_methods] must be on an inherent impl of a #[class]",
    ));
  };
  let name = &path
    .path
    .segments
    .last()
    .ok_or_else(|| Error::new_spanned(path, "expected a #[class]"))?
    .ident;
  let helper = helper_name(name);
  Ok(quote! { #helper! { #item } })
}

/// Generates a root class, or asks a subclass's parent for its descriptor.
pub fn complete(input: TokenStream) -> Result<TokenStream> {
  let input: Input = syn::parse2(input)?;
  let options = parse_class_options(input.attr.clone())?;
  let krate = &options.krate;
  let marker = marker_name(&input.item.ident);
  let class = match &options.extends {
    None => Model::new(input, Vec::new(), None).and_then(generate),
    Some(Type::Path(parent)) if parent.qself.is_none() => {
      let mut descriptor = parent.path.clone();
      for segment in &mut descriptor.segments {
        segment.arguments = PathArguments::None;
      }
      // Only open classes export a descriptor, so extending a final class fails to find it. This explains
      // why, alongside that error.
      let open_check = require_open(&options, &input.item, parent);
      Ok(quote! {
        #open_check
        #descriptor! { #krate::__private::__inherit, { #input } }
      })
    }
    Some(other) => Err(Error::new_spanned(other, "`extends` must name a #[class]")),
  };
  let class = class.unwrap_or_else(Error::into_compile_error);
  Ok(quote! {
    impl #krate::__private::HasClassMethods for #marker {}
    #class
  })
}

/// Generates a subclass, given its parent's ancestry.
pub fn inherit(input: TokenStream) -> Result<TokenStream> {
  let Inheritance {
    input,
    params,
    is_unsafe,
    mut ancestors,
  } = syn::parse2(input)?;
  let parent = parse_class_options(input.attr.clone())?
    .extends
    .ok_or_else(|| Error::new_spanned(&input.item, "missing parent"))?;
  let Type::Path(path) = &parent else {
    return Err(Error::new_spanned(parent, "`extends` must name a #[class]"));
  };
  let segment = path
    .path
    .segments
    .last()
    .ok_or_else(|| Error::new_spanned(path, "expected a parent class"))?;
  let arguments: Vec<Type> = match &segment.arguments {
    PathArguments::None => Vec::new(),
    PathArguments::AngleBracketed(args) => args
      .args
      .iter()
      .map(|arg| match arg {
        GenericArgument::Type(ty) => Ok(ty.clone()),
        _ => Err(Error::new_spanned(arg, "a parent class only takes type arguments")),
      })
      .collect::<Result<_>>()?,
    other => return Err(Error::new_spanned(other, "expected type arguments")),
  };
  if params.len() != arguments.len() {
    return Err(Error::new_spanned(
      &parent,
      format!(
        "give each of the parent's {} type parameters an argument, including those with defaults",
        params.len()
      ),
    ));
  }
  if ancestors.is_empty() {
    return Err(Error::new_spanned(parent, "the parent's descriptor is empty"));
  }
  let mut substitute = Substitute(params.iter().map(ToString::to_string).zip(arguments).collect());
  for ancestor in &mut ancestors {
    substitute.ancestor(ancestor);
  }
  let runtime = if is_unsafe { Runtime::Unsafe } else { Runtime::Safe };
  generate(Model::new(input, ancestors, Some(runtime))?)
}

fn generate(model: Model) -> Result<TokenStream> {
  match model.runtime {
    Runtime::Safe => safe_runtime::generate(&model),
    Runtime::Unsafe => unsafe_runtime::generate(&model),
  }
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

/// Checks that a subclass's parent is open, in a function with the subclass's generics.
fn require_open(options: &ClassOptions, item: &ItemStruct, parent: &syn::TypePath) -> TokenStream {
  let mut item = item.clone();
  let self_ty = SelfType {
    ident: item.ident.clone(),
    ty_generics: item.generics.split_for_impl().1.to_token_stream(),
  };
  let Ok(fields) = fields::take_fields(&mut item, options, &self_ty) else {
    return quote!();
  };
  let Ok(generics) = generics::class_generics(&item.generics, &fields, options.extends.as_ref()) else {
    return quote!();
  };
  let (ig, _, wc) = generics.full.split_for_impl();
  let krate = &options.krate;
  quote! {
    const _: () = {
      #[allow(dead_code)]
      fn __require_open #ig () #wc {
        #krate::__private::require_open::<#parent>();
      }
    };
  }
}

/// Reports the errors in a class's struct and options straight away, rather than once its methods block
/// supplies the rest.
fn validate_declaration(options: &ClassOptions, item: &ItemStruct) -> Result<()> {
  validate_struct(item)?;
  if let Some(extends) = &options.extends {
    validate_extends(extends, &item.generics)?;
  }
  let mut item = item.clone();
  let self_ty = SelfType {
    ident: item.ident.clone(),
    ty_generics: item.generics.split_for_impl().1.to_token_stream(),
  };
  let fields = fields::take_fields(&mut item, options, &self_ty)?;
  generics::class_generics(&item.generics, &fields, options.extends.as_ref())?;
  Ok(())
}

/// Rejects parents that can never be a class, which would otherwise give confusing errors.
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

/// Everything the generators need to know about a class.
pub struct Model {
  krate: Path,
  vis: Visibility,
  name: Ident,
  /// Doc comments, which document the handle.
  doc_attrs: Vec<Attribute>,
  /// The struct's other attributes, which apply to `XData`.
  data_attrs: Vec<Attribute>,
  open: bool,
  parent: Option<Type>,
  runtime: Runtime,
  /// The `runtime_strategy` as written, if it was.
  runtime_path: Option<Path>,
  generics: ClassGenerics,
  fields: Vec<ClassField>,
  methods: Vec<Method>,
  /// The root first, the parent last.
  ancestors: Vec<Ancestor>,
  inherited: Inherited,
}

impl Model {
  fn new(input: Input, ancestors: Vec<Ancestor>, inherited_runtime: Option<Runtime>) -> Result<Self> {
    let mut options = parse_class_options(input.attr)?;
    let krate = options.krate.clone();
    let open = options.open;
    let extends = options.extends.clone();
    let runtime = options.runtime.take();
    let mut item = input.item;
    validate_struct(&item)?;
    if let Some(extends) = &extends {
      validate_extends(extends, &item.generics)?;
    }
    let (runtime, runtime_path) = match (runtime, inherited_runtime) {
      (Some(explicit), Some(inherited)) if explicit.runtime != inherited => {
        return Err(Error::new_spanned(
          explicit.path,
          format!(
            "a subclass must use its parent's runtime strategy, `{}`; leave `runtime_strategy` out to inherit it",
            inherited.name()
          ),
        ));
      }
      (Some(explicit), _) => (explicit.runtime, Some(explicit.path)),
      (None, inherited) => (inherited.unwrap_or(Runtime::Safe), None),
    };

    let self_ty = SelfType {
      ident: item.ident.clone(),
      ty_generics: item.generics.split_for_impl().1.to_token_stream(),
    };
    let fields = fields::take_fields(&mut item, &options, &self_ty)?;
    let generics = generics::class_generics(&item.generics, &fields, extends.as_ref())?;
    let name = item.ident;
    let tg = generics.full.split_for_impl().1;
    let own: Type = parse_quote!(#name #tg);
    methods::validate_impl(&input.methods, &own, &generics.full)?;

    let mut inherited = Inherited::new();
    for (index, ancestor) in ancestors.iter().enumerate() {
      for method in &ancestor.methods {
        inherited.insert(method.name.to_string(), (index, method.is_async));
      }
    }
    let mut reserved: HashSet<String> = RESERVED_NAMES.iter().map(|n| n.to_string()).collect();
    for field in &fields {
      let bare = field.bare();
      reserved.extend([bare.to_string(), format!("set_{bare}"), format!("update_{bare}")]);
    }
    let methods = methods::collect(input.methods, &mut reserved, &inherited)?;

    let (doc_attrs, data_attrs) = item.attrs.into_iter().partition(|a| a.path().is_ident("doc"));
    Ok(Self {
      krate,
      vis: item.vis,
      name,
      doc_attrs,
      data_attrs,
      open,
      parent: extends,
      runtime,
      runtime_path,
      generics,
      fields,
      methods,
      ancestors,
      inherited,
    })
  }

  /// The handle type, `Name<T..>`.
  fn own(&self) -> Type {
    let name = &self.name;
    let tg = self.generics.full.split_for_impl().1;
    parse_quote!(#name #tg)
  }

  fn data_name(&self) -> Ident {
    format_ident!("{}Data", self.name)
  }

  /// `NameData<T..>`, with only the parameters its fields use.
  fn data_ty(&self) -> Type {
    let name = self.data_name();
    let tg = self.generics.data.split_for_impl().1;
    parse_quote!(#name #tg)
  }

  /// The parent's `XData` type, as named by its descriptor.
  fn parent_data(&self) -> Option<&Type> {
    self.ancestors.last().map(|a| &a.data)
  }

  /// A hidden type named after the class, e.g. `__NameFields<T..>`.
  fn hidden_ty(&self, suffix: &str) -> (Ident, Type) {
    let ident = format_ident!("__{}{}", self.name, suffix);
    let tg = self.generics.full.split_for_impl().1;
    let ty = parse_quote!(#ident #tg);
    (ident, ty)
  }

  fn own_virtuals(&self) -> Vec<VirtualMethod> {
    self
      .methods
      .iter()
      .filter(|m| m.kind == Kind::Virtual)
      .map(|m| VirtualMethod {
        name: m.name().clone(),
        is_async: m.is_async(),
      })
      .collect()
  }

  /// The index of the ancestor that declares the virtual method an override overrides.
  fn declaring_ancestor(&self, method: &Method) -> usize {
    self.inherited[&method.name().to_string()].0
  }
}
