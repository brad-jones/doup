/// Returns `T`'s type name with module paths stripped, e.g. `Vec<String>` rather than
/// `alloc::vec::Vec<alloc::string::String>`. Used by the `Debug` output of pointers and classes.
#[doc(hidden)]
pub fn short_type_name<T: ?Sized>() -> String {
  let full = std::any::type_name::<T>();
  let mut out = String::with_capacity(full.len());
  let mut path = String::new();

  for c in full.chars() {
    if c.is_alphanumeric() || c == '_' || c == ':' {
      path.push(c);
    } else {
      out.push_str(path.rsplit("::").next().unwrap_or(&path));
      path.clear();
      out.push(c);
    }
  }
  out.push_str(path.rsplit("::").next().unwrap_or(&path));
  out
}

#[cfg(test)]
mod tests {
  use super::short_type_name;

  #[test]
  fn strips_module_paths_from_nested_types() {
    assert_eq!(short_type_name::<i32>(), "i32");
    assert_eq!(short_type_name::<Vec<String>>(), "Vec<String>");
    assert_eq!(
      short_type_name::<Option<(u8, Vec<String>)>>(),
      "Option<(u8, Vec<String>)>"
    );
  }
}
