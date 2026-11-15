//! `plugin_wrapper!` generates a store-owning wrapper around a wasmtime `bindgen!` world.
//!
//! The wrapper has one async method per world-level export in the WIT file, named like the
//! export (kebab to snake case) and without bindgen's `call_` prefix, so callers never handle
//! the `Store` and the host stays in sync with the contract automatically.
//!
//! ```ignore
//! plugin_wrapper! {
//!   wit = "../wit",              // path relative to the invoking crate's manifest
//!   world = "platform",
//!   bindings = Platform,         // type produced by `bindgen!` (with async exports)
//!   name = PlatformPlugin,       // wrapper to generate
//!   state = super::host::HostState,
//! }
//! ```

mod types;

use heck::ToSnakeCase;
use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use std::path::PathBuf;
use syn::{
  Ident, LitStr, Path, Token,
  parse::{Parse, ParseStream},
  parse_macro_input,
};
use types::TypeCx;
use wit_parser::{Function, FunctionKind, Resolve, WorldItem};

struct Input {
  wit: LitStr,
  world: LitStr,
  bindings: Ident,
  name: Ident,
  state: Path,
}

impl Parse for Input {
  fn parse(input: ParseStream) -> syn::Result<Self> {
    let mut wit: Option<LitStr> = None;
    let mut world: Option<LitStr> = None;
    let mut bindings: Option<Ident> = None;
    let mut name: Option<Ident> = None;
    let mut state: Option<Path> = None;

    while !input.is_empty() {
      let key: Ident = input.parse()?;
      input.parse::<Token![=]>()?;
      match key.to_string().as_str() {
        "wit" => wit = Some(input.parse()?),
        "world" => world = Some(input.parse()?),
        "bindings" => bindings = Some(input.parse()?),
        "name" => name = Some(input.parse()?),
        "state" => state = Some(input.parse()?),
        other => {
          return Err(syn::Error::new(
            key.span(),
            format!("unknown key `{other}`, expected wit, world, bindings, name or state"),
          ));
        }
      }
      if !input.is_empty() {
        input.parse::<Token![,]>()?;
      }
    }

    let missing = |key: &str| syn::Error::new(Span::call_site(), format!("missing `{key}`"));
    Ok(Self {
      wit: wit.ok_or_else(|| missing("wit"))?,
      world: world.ok_or_else(|| missing("world"))?,
      bindings: bindings.ok_or_else(|| missing("bindings"))?,
      name: name.ok_or_else(|| missing("name"))?,
      state: state.ok_or_else(|| missing("state"))?,
    })
  }
}

#[proc_macro]
pub fn plugin_wrapper(input: TokenStream) -> TokenStream {
  let input = parse_macro_input!(input as Input);
  expand(input)
    .unwrap_or_else(syn::Error::into_compile_error)
    .into()
}

fn expand(input: Input) -> syn::Result<TokenStream2> {
  let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
    .map_err(|err| syn::Error::new(Span::call_site(), format!("CARGO_MANIFEST_DIR: {err}")))?;
  let wit_path = PathBuf::from(manifest_dir).join(input.wit.value());

  let mut resolve = Resolve::new();
  let (package, sources) = resolve
    .push_path(&wit_path)
    .map_err(|err| syn::Error::new(input.wit.span(), format!("{err:#}")))?;
  let world_id = resolve
    .select_world(&[package], Some(&input.world.value()))
    .map_err(|err| syn::Error::new(input.world.span(), format!("{err:#}")))?;
  let world = &resolve.worlds[world_id];

  let cx = TypeCx {
    resolve: &resolve,
    world,
  };
  let methods = world
    .exports
    .values()
    .map(|item| match item {
      WorldItem::Function(func) => method(&cx, func),
      _ => Err(syn::Error::new(
        input.world.span(),
        "plugin_wrapper!: only world-level function exports are supported",
      )),
    })
    .collect::<syn::Result<Vec<_>>>()?;

  // Makes cargo rebuild the invoking crate whenever the WIT contract changes.
  let tracked = sources
    .paths()
    .map(|path| path.to_string_lossy().into_owned())
    .collect::<Vec<_>>();

  let Input {
    bindings,
    name,
    state,
    ..
  } = &input;
  let pre = format_ident!("{}Pre", bindings);
  Ok(quote! {
    #(const _: &[u8] = include_bytes!(#tracked);)*

    type Configure = ::std::sync::Arc<
      dyn Fn(&mut ::wasmtime_wasi::WasiCtxBuilder) + Send + Sync,
    >;

    /// A plugin component, instantiated lazily into a fresh `Store` per phase.
    pub struct #name {
      pre: #pre<#state>,
      configure: Configure,
      session: Option<(::wasmtime::Store<#state>, #bindings)>,
    }

    impl #name {
      pub fn new(
        pre: #pre<#state>,
        configure: impl Fn(&mut ::wasmtime_wasi::WasiCtxBuilder) + Send + Sync + 'static,
      ) -> Self {
        Self { pre, configure: ::std::sync::Arc::new(configure), session: None }
      }

      /// Discards the current instance; the next call instantiates a fresh one using `configure`.
      pub fn begin_phase(
        &mut self,
        configure: impl Fn(&mut ::wasmtime_wasi::WasiCtxBuilder) + Send + Sync + 'static,
      ) {
        self.configure = ::std::sync::Arc::new(configure);
        self.session = None;
      }

      async fn take_session(
        &mut self,
      ) -> ::wasmtime::Result<(::wasmtime::Store<#state>, #bindings)> {
        if let Some(session) = self.session.take() {
          return Ok(session);
        }
        let mut store = ::wasmtime::Store::new(self.pre.engine(), #state::new(&*self.configure));
        let bindings = self.pre.instantiate_async(&mut store).await?;
        Ok((store, bindings))
      }

      #(#methods)*
    }
  })
}

fn method(cx: &TypeCx, func: &Function) -> syn::Result<TokenStream2> {
  if !matches!(func.kind, FunctionKind::Freestanding) {
    return Err(syn::Error::new(
      Span::call_site(),
      format!(
        "plugin_wrapper!: `{}` must be a plain synchronous func",
        func.name
      ),
    ));
  }

  let snake = func.name.to_snake_case();
  let method = format_ident!("{snake}");
  let call = format_ident!("call_{snake}");
  let doc = format!("Calls the `{}` export.", func.name);

  let names = func
    .params
    .iter()
    .map(|param| format_ident!("{}", param.name.to_snake_case()))
    .collect::<Vec<_>>();
  let types = func
    .params
    .iter()
    .map(|param| cx.param(&param.ty))
    .collect::<syn::Result<Vec<_>>>()?;
  let ret = match &func.result {
    Some(ty) => cx.owned(ty)?,
    None => quote!(()),
  };

  Ok(quote! {
    #[doc = #doc]
    pub async fn #method(&mut self, #(#names: #types),*) -> ::wasmtime::Result<#ret> {
      let (mut store, bindings) = self.take_session().await?;
      let result = bindings.#call(&mut store, #(#names),*).await;
      // An Err here is a trap or host failure, so the instance is dropped rather than reused.
      if result.is_ok() {
        self.session = Some((store, bindings));
      }
      result
    }
  })
}
