use anyhow::Result;
use wasmtime_wasi::WasiCtxBuilder;

mod host;
mod session;
mod wkg;

mod ecosystem;
pub use ecosystem::EcosystemPlugin;

mod platform;
pub use platform::PlatformPlugin;

/// Just a simple wrapper to make it easy to put a WasiCtxBuilder closure into a variable.
///
/// Before:
///
/// ```ignore
/// let mut session = self
///   .session_pool
///   .create_session(HttpPolicy::Denied, |b| Ok(b.preopened_dir(workspace_dir, "/workspace", FsPerms::ReadOnly)?))
///   .await?;
/// ```
///
/// After:
///
/// ```ignore
/// let permissions = wasi(|b| Ok(b.preopened_dir(workspace_dir, "/workspace", FsPerms::ReadOnly)?));
/// let mut session = self.session_pool.create_session(HttpPolicy::Denied, permissions).await?;
/// ```
///
/// It's purely a stylistic thing,
fn wasi(
  b: impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder>,
) -> impl FnOnce(&mut WasiCtxBuilder) -> Result<&mut WasiCtxBuilder> {
  b
}
