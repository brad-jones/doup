use anyhow::{Context, Result};
use std::{
  path::{Path, PathBuf},
  sync::{Mutex, OnceLock, PoisonError},
};
use wasmtime::{
  AsContextMut, Engine, Store,
  component::{Component, InstancePre, Linker},
};
use wasmtime_wasi::WasiCtxBuilder;

pub trait IPre<State, Bindings>: Clone {
  fn new(instance_pre: InstancePre<State>) -> wasmtime::Result<Self>;
  async fn instantiate_async(&self, store: impl AsContextMut<Data = State>) -> wasmtime::Result<Bindings>;
}

pub trait IState {
  fn new(configure: impl FnOnce(&mut WasiCtxBuilder)) -> Self;
}

pub trait IBindings<State> {
  async fn call_configure(
    &self,
    store: &mut Store<State>,
    arg0: &str,
  ) -> wasmtime::Result<Result<(), wasmtime::component::__internal::String>>;
}

pub struct PluginSession<State: 'static, Bindings: IBindings<State>>(pub Store<State>, pub Bindings);

pub struct Plugin<State: IState + 'static, Pre, Bindings>
where
  Bindings: IBindings<State>,
  Pre: IPre<State, Bindings>,
{
  engine: Engine,
  linker: Linker<State>,
  component_path: PathBuf,
  component_pre: OnceLock<Pre>,
  session_pool: Mutex<Vec<PluginSession<State, Bindings>>>,
  wasi_ctx: fn(&mut WasiCtxBuilder),
}

impl<State: IState, Pre, Bindings> Plugin<State, Pre, Bindings>
where
  Bindings: IBindings<State>,
  Pre: IPre<State, Bindings>,
{
  pub fn new(engine: &Engine, linker: &Linker<State>, path: &Path, wasi_ctx: fn(&mut WasiCtxBuilder)) -> Self {
    Self {
      engine: engine.clone(),
      linker: linker.clone(),
      component_path: path.to_path_buf(),
      component_pre: OnceLock::new(),
      session_pool: Mutex::new(Vec::new()),
      wasi_ctx,
    }
  }

  fn component(&self) -> Result<Pre> {
    if let Some(component_pre) = self.component_pre.get() {
      return Ok(component_pre.clone());
    }

    let component = Component::from_file(&self.engine, &self.component_path)?;
    let component_pre = Pre::new(self.linker.instantiate_pre(&component)?)?;
    let _ = self.component_pre.set(component_pre.clone());
    Ok(component_pre)
  }

  async fn create_session(&self) -> Result<PluginSession<State, Bindings>> {
    let mut store = Store::new(&self.engine, State::new(&self.wasi_ctx));
    let bindings = self.component()?.instantiate_async(&mut store).await?;

    bindings
      .call_configure(&mut store, "")
      .await?
      .map_err(anyhow::Error::msg)
      .with_context(|| format!("plugin {} rejected its config", self.component_path.to_string_lossy()))?;

    Ok(PluginSession(store, bindings))
  }

  pub async fn checkout_session(&self) -> Result<PluginSession<State, Bindings>> {
    let session = self.session_pool.lock().unwrap_or_else(PoisonError::into_inner).pop();
    match session {
      Some(session) => Ok(session),
      None => self.create_session().await,
    }
  }

  pub fn checkin_session<R>(&self, session: PluginSession<State, Bindings>, result: wasmtime::Result<R>) -> Result<R> {
    let value = result?;
    self
      .session_pool
      .lock()
      .unwrap_or_else(PoisonError::into_inner)
      .push(session);
    Ok(value)
  }
}
