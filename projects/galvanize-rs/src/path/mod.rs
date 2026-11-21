use crate::GString;
use std::borrow::Borrow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::{Deref, Div};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// An immutable, reference-counted filesystem path.
///
/// `GPath` wraps an [`Arc<Path>`], so cloning is a cheap atomic reference-count
/// increment. Consumers never need to care whether a path was originally
/// owned ([`PathBuf`]) or borrowed ([`Path`]).
///
/// It dereferences to [`Path`], so all `Path` methods are available directly.
///
/// # Examples
///
/// ```
/// use galvanize_rs::GPath;
///
/// let a = GPath::from("/tmp");
/// let b = a.clone();
/// assert_eq!(a, b);
/// assert_eq!(a, "/tmp");
/// assert_eq!(&a / "file.txt", "/tmp/file.txt");
/// assert_eq!(a.file_name().unwrap(), "tmp");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GPath(Arc<Path>);

impl GPath {
  /// Creates a new, empty `GPath`.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GPath;
  ///
  /// assert!(GPath::new().as_os_str().is_empty());
  /// ```
  pub fn new() -> Self {
    GPath::from("")
  }

  /// Returns the contents as a [`Path`].
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GPath;
  /// use std::path::Path;
  ///
  /// assert_eq!(GPath::from("a/b").as_path(), Path::new("a/b"));
  /// ```
  pub fn as_path(&self) -> &Path {
    &self.0
  }
}

impl Default for GPath {
  fn default() -> Self {
    GPath::new()
  }
}

impl Deref for GPath {
  type Target = Path;
  fn deref(&self) -> &Path {
    &self.0
  }
}

impl AsRef<Path> for GPath {
  fn as_ref(&self) -> &Path {
    &self.0
  }
}

impl AsRef<OsStr> for GPath {
  fn as_ref(&self) -> &OsStr {
    self.0.as_os_str()
  }
}

impl Borrow<Path> for GPath {
  fn borrow(&self) -> &Path {
    &self.0
  }
}

impl From<&Path> for GPath {
  fn from(p: &Path) -> Self {
    GPath(Arc::from(p))
  }
}

impl From<PathBuf> for GPath {
  fn from(p: PathBuf) -> Self {
    GPath(Arc::from(p))
  }
}

impl From<&PathBuf> for GPath {
  fn from(p: &PathBuf) -> Self {
    GPath::from(p.as_path())
  }
}

impl From<&str> for GPath {
  fn from(s: &str) -> Self {
    GPath::from(Path::new(s))
  }
}

impl From<String> for GPath {
  fn from(s: String) -> Self {
    GPath::from(PathBuf::from(s))
  }
}

impl From<&String> for GPath {
  fn from(s: &String) -> Self {
    GPath::from(s.as_str())
  }
}

impl From<&OsStr> for GPath {
  fn from(s: &OsStr) -> Self {
    GPath::from(Path::new(s))
  }
}

impl From<OsString> for GPath {
  fn from(s: OsString) -> Self {
    GPath::from(PathBuf::from(s))
  }
}

impl From<&GString> for GPath {
  fn from(s: &GString) -> Self {
    GPath::from(s.as_str())
  }
}

impl From<GString> for GPath {
  fn from(s: GString) -> Self {
    GPath::from(s.as_str())
  }
}

impl From<&GPath> for GPath {
  fn from(p: &GPath) -> Self {
    p.clone()
  }
}

impl From<GPath> for PathBuf {
  fn from(p: GPath) -> Self {
    p.0.to_path_buf()
  }
}

impl From<GPath> for Arc<Path> {
  fn from(p: GPath) -> Self {
    p.0
  }
}

impl fmt::Display for GPath {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&self.0.display(), f)
  }
}

impl fmt::Debug for GPath {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Debug::fmt(&*self.0, f)
  }
}

impl PartialEq<Path> for GPath {
  fn eq(&self, other: &Path) -> bool {
    &*self.0 == other
  }
}

impl PartialEq<&Path> for GPath {
  fn eq(&self, other: &&Path) -> bool {
    &*self.0 == *other
  }
}

impl PartialEq<PathBuf> for GPath {
  fn eq(&self, other: &PathBuf) -> bool {
    &*self.0 == other.as_path()
  }
}

impl PartialEq<str> for GPath {
  fn eq(&self, other: &str) -> bool {
    &*self.0 == Path::new(other)
  }
}

impl PartialEq<&str> for GPath {
  fn eq(&self, other: &&str) -> bool {
    &*self.0 == Path::new(*other)
  }
}

impl PartialEq<String> for GPath {
  fn eq(&self, other: &String) -> bool {
    &*self.0 == Path::new(other)
  }
}

impl PartialEq<GPath> for Path {
  fn eq(&self, other: &GPath) -> bool {
    self == &*other.0
  }
}

impl PartialEq<GPath> for &Path {
  fn eq(&self, other: &GPath) -> bool {
    *self == &*other.0
  }
}

impl PartialEq<GPath> for PathBuf {
  fn eq(&self, other: &GPath) -> bool {
    self.as_path() == &*other.0
  }
}

impl PartialEq<GPath> for &str {
  fn eq(&self, other: &GPath) -> bool {
    Path::new(*self) == &*other.0
  }
}

/// Joins paths, like [`Path::join`], returning a new `GPath`.
impl<P: AsRef<Path>> Div<P> for &GPath {
  type Output = GPath;
  fn div(self, rhs: P) -> GPath {
    GPath::from(self.0.join(rhs))
  }
}

impl<P: AsRef<Path>> Div<P> for GPath {
  type Output = GPath;
  fn div(self, rhs: P) -> GPath {
    &self / rhs
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  #[test]
  fn new_and_default_are_empty() {
    assert!(GPath::new().as_os_str().is_empty());
    assert_eq!(GPath::default(), GPath::new());
  }

  #[test]
  fn from_conversions() {
    let expected = "a/b";
    assert_eq!(GPath::from("a/b"), expected);
    assert_eq!(GPath::from(String::from("a/b")), expected);
    assert_eq!(GPath::from(&String::from("a/b")), expected);
    assert_eq!(GPath::from(Path::new("a/b")), expected);
    assert_eq!(GPath::from(PathBuf::from("a/b")), expected);
    assert_eq!(GPath::from(&PathBuf::from("a/b")), expected);
    assert_eq!(GPath::from(OsStr::new("a/b")), expected);
    assert_eq!(GPath::from(OsString::from("a/b")), expected);
    assert_eq!(GPath::from(GString::from("a/b")), expected);
    assert_eq!(GPath::from(&GString::from("a/b")), expected);
    let original = GPath::from("a/b");
    assert_eq!(GPath::from(&original), expected);
  }

  #[test]
  fn into_pathbuf_and_arc() {
    assert_eq!(PathBuf::from(GPath::from("a")), PathBuf::from("a"));
    assert_eq!(&*Arc::<Path>::from(GPath::from("a")), Path::new("a"));
  }

  #[test]
  fn clone_shares_allocation() {
    let a = GPath::from("shared");
    let b = a.clone();
    assert!(std::ptr::eq(a.as_path(), b.as_path()));
  }

  #[test]
  fn deref_exposes_path_methods() {
    let p = GPath::from("/tmp/file.txt");
    assert!(p.is_absolute());
    assert_eq!(p.extension().unwrap(), "txt");
    assert_eq!(p.parent().unwrap(), Path::new("/tmp"));
    assert_eq!(AsRef::<OsStr>::as_ref(&p), OsStr::new("/tmp/file.txt"));
  }

  #[test]
  fn display_and_debug() {
    let p = GPath::from("a/b");
    assert_eq!(format!("{p}"), "a/b");
    assert_eq!(format!("{p:?}"), format!("{:?}", Path::new("a/b")));
  }

  #[test]
  fn equality_with_other_path_types() {
    let p = GPath::from("a/b");
    assert!(p == *Path::new("a/b"));
    assert!(p == Path::new("a/b"));
    assert!(p == PathBuf::from("a/b"));
    assert!(p == *"a/b");
    assert!(p == "a/b");
    assert!(p == String::from("a/b"));
    assert!(Path::new("a/b") == p);
    assert!(*Path::new("a/b") == p);
    assert!(PathBuf::from("a/b") == p);
    assert!("a/b" == p);
    assert!(p != "a/c");
  }

  #[test]
  fn borrow_allows_path_lookup_in_maps() {
    let mut map: HashMap<GPath, i32> = HashMap::new();
    map.insert(GPath::from("key"), 1);
    assert_eq!(map.get(Path::new("key")), Some(&1));
    assert_eq!(map.get(Path::new("missing")), None);
  }

  #[test]
  fn div_joins_without_mutating_borrowed_lhs() {
    let a = GPath::from("a");
    assert_eq!(&a / "b", "a/b");
    assert_eq!(&a / String::from("c"), "a/c");
    assert_eq!(&a / GPath::from("d"), "a/d");
    assert_eq!(a.clone() / "e" / "f", "a/e/f");
    assert_eq!(a, "a");
  }

  #[test]
  fn is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GPath>();
  }
}
