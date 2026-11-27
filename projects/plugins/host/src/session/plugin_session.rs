use crate::host::HostState;
use crate::world::PluginWorld;
use wasmtime::{Store, component::Accessor};

/// A session is the combination of a set of bindings and their store.
/// Exclusive access to the `Store` is what makes concurrent calls need a pool.
///
/// The `Store` is what holds the guests memory space, so anytime a new store
/// is created, the world's `configure` export needs to be called.
///
/// [`PluginSessionPool`] ensures this happen anytime a session is checked out of the pool.
pub struct PluginSession<W: PluginWorld> {
  pub store: Store<HostState>,
  pub bindings: W::Bindings,
}

impl<W: PluginWorld> PluginSession<W> {
  /// Calls an `async func` export. Their generated `call_*` methods take an [`Accessor`] instead of the
  /// `Store`, so they can only run inside [`Store::run_concurrent`], this takes care of that.
  ///
  /// Usually called via [`PluginSessionPool::call_export`](super::PluginSessionPool::call_export):
  ///
  /// ```ignore
  /// let mut session = self.session_pool.checkout_session().await?;
  /// let result = session.call_async(async |bindings, accessor| bindings.call_my_export(accessor).await).await?;
  /// self.session_pool.checkin_session(session);
  /// ```
  ///
  /// # Errors
  ///
  /// Returns an error if the guest traps or the call otherwise fails at the wasmtime level,
  /// the guest's own `result<_, guest-error>` is left for the caller to handle.
  pub async fn call_async<R>(
    &mut self,
    call: impl AsyncFnOnce(&W::Bindings, &Accessor<HostState>) -> wasmtime::Result<R>,
  ) -> wasmtime::Result<R> {
    let bindings = &self.bindings;
    self.store.run_concurrent(async |accessor| call(bindings, accessor).await).await?
  }
}
