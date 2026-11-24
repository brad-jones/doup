/// A wkg reference (`namespace:package@version`) has a `:` but no path separators.
pub fn is_wkg_reference(plugin: &str) -> bool {
  plugin.contains(':') && !plugin.contains(['/', '\\'])
}
