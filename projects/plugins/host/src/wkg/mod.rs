//! This is where we plan on putting all the WKG code.
//! But right now doup-plugin-host provides no support for loading plugins from remote wkg references.

/// A wkg reference (`namespace:package@version`) has a `:` but no path separators.
pub fn is_wkg_reference(plugin: &str) -> bool {
  plugin.contains(':') && !plugin.contains(['/', '\\'])
}
