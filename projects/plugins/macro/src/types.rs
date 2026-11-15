use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::Error;
use wit_parser::{Resolve, Type, TypeDef, TypeDefKind, TypeOwner, World, WorldItem};

use heck::{ToSnakeCase, ToUpperCamelCase};

/// Maps WIT types onto the Rust types that wasmtime's `bindgen!` generates.
pub struct TypeCx<'a> {
  pub resolve: &'a Resolve,
  pub world: &'a World,
}

fn unsupported(what: &str) -> Error {
  Error::new(
    proc_macro2::Span::call_site(),
    format!("plugin_wrapper!: unsupported WIT construct: {what}"),
  )
}

impl TypeCx<'_> {
  /// The type of a parameter as taken by bindgen's `call_*` exports (borrowed strings & lists).
  pub fn param(&self, ty: &Type) -> syn::Result<TokenStream> {
    if let Type::String = ty {
      return Ok(quote!(&str));
    }
    if let Type::Id(id) = ty {
      let def = &self.resolve.types[*id];
      if def.name.is_none() {
        match &def.kind {
          TypeDefKind::List(inner) => {
            let inner = self.owned(inner)?;
            return Ok(quote!(&[#inner]));
          }
          _ => return Err(unsupported(&format!("{} parameter", def.kind.as_str()))),
        }
      }
      return Err(unsupported("named parameter type"));
    }
    self.owned(ty)
  }

  /// The owned Rust representation of a WIT type.
  pub fn owned(&self, ty: &Type) -> syn::Result<TokenStream> {
    Ok(match ty {
      Type::Bool => quote!(bool),
      Type::U8 => quote!(u8),
      Type::U16 => quote!(u16),
      Type::U32 => quote!(u32),
      Type::U64 => quote!(u64),
      Type::S8 => quote!(i8),
      Type::S16 => quote!(i16),
      Type::S32 => quote!(i32),
      Type::S64 => quote!(i64),
      Type::F32 => quote!(f32),
      Type::F64 => quote!(f64),
      Type::Char => quote!(char),
      Type::String => quote!(::std::string::String),
      Type::ErrorContext => return Err(unsupported("error-context")),
      Type::Id(id) => {
        let def = &self.resolve.types[*id];
        match &def.name {
          Some(name) => self.named(def, name)?,
          None => self.anonymous(&def.kind)?,
        }
      }
    })
  }

  fn anonymous(&self, kind: &TypeDefKind) -> syn::Result<TokenStream> {
    Ok(match kind {
      TypeDefKind::Tuple(tuple) => {
        let types = tuple
          .types
          .iter()
          .map(|ty| self.owned(ty))
          .collect::<syn::Result<Vec<_>>>()?;
        quote!((#(#types,)*))
      }
      TypeDefKind::Option(inner) => {
        let inner = self.owned(inner)?;
        quote!(::core::option::Option<#inner>)
      }
      TypeDefKind::Result(result) => {
        let ok = self.optional(result.ok.as_ref())?;
        let err = self.optional(result.err.as_ref())?;
        quote!(::core::result::Result<#ok, #err>)
      }
      TypeDefKind::List(inner) => {
        let inner = self.owned(inner)?;
        quote!(::std::vec::Vec<#inner>)
      }
      TypeDefKind::Type(inner) => self.owned(inner)?,
      other => return Err(unsupported(other.as_str())),
    })
  }

  /// Resolves a named type to the path bindgen emits it at, relative to the invoking module.
  fn named(&self, def: &TypeDef, name: &str) -> syn::Result<TokenStream> {
    match def.kind {
      TypeDefKind::Record(_)
      | TypeDefKind::Variant(_)
      | TypeDefKind::Enum(_)
      | TypeDefKind::Flags(_)
      | TypeDefKind::Type(_) => {}
      ref other => return Err(unsupported(&format!("named {}", other.as_str()))),
    }

    let ty = format_ident!("{}", name.to_upper_camel_case());
    match def.owner {
      TypeOwner::World(_) => Ok(quote!(#ty)),
      TypeOwner::Interface(id) => {
        let interface = &self.resolve.interfaces[id];
        let (Some(package), Some(iface_name)) = (interface.package, &interface.name) else {
          return Err(unsupported("anonymous interface"));
        };
        let package = &self.resolve.packages[package].name;
        let namespace = format_ident!("{}", package.namespace.to_snake_case());
        let package = format_ident!("{}", package.name.to_snake_case());
        let iface = format_ident!("{}", iface_name.to_snake_case());
        let imported =
          self.world.imports.values().any(
            |item| matches!(item, WorldItem::Interface { id: imported, .. } if *imported == id),
          );
        Ok(if imported {
          quote!(#namespace::#package::#iface::#ty)
        } else {
          quote!(exports::#namespace::#package::#iface::#ty)
        })
      }
      TypeOwner::None => Err(unsupported("type without an owner")),
    }
  }

  fn optional(&self, ty: Option<&Type>) -> syn::Result<TokenStream> {
    ty.map_or_else(|| Ok(quote!(())), |ty| self.owned(ty))
  }
}
