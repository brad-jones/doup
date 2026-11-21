use crate::GString;
use std::borrow::Borrow;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;

/// An immutable, reference-counted platform string.
///
/// `GOsStr` is to [`OsStr`] what [`GString`] is to [`str`]: it wraps an
/// [`Arc<OsStr>`], so cloning is a cheap atomic reference-count increment.
/// Use it for values that may not be valid UTF-8, such as file names.
///
/// It dereferences to [`OsStr`], so all `OsStr` methods are available directly,
/// and it can be compared against `str`, `&str`, [`String`], [`OsStr`] and
/// [`OsString`] values.
///
/// # Examples
///
/// ```
/// use galvanize_rs::GOsStr;
///
/// let a = GOsStr::from("hello");
/// let b = a.clone();
/// assert_eq!(a, b);
/// assert_eq!(a, "hello");
/// assert_eq!(a.to_str(), Some("hello"));
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GOsStr(Arc<OsStr>);

impl GOsStr {
  /// Creates a new, empty `GOsStr`.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GOsStr;
  ///
  /// assert!(GOsStr::new().is_empty());
  /// ```
  pub fn new() -> Self {
    GOsStr::from("")
  }

  /// Returns the contents as an [`OsStr`].
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GOsStr;
  /// use std::ffi::OsStr;
  ///
  /// assert_eq!(GOsStr::from("abc").as_os_str(), OsStr::new("abc"));
  /// ```
  pub fn as_os_str(&self) -> &OsStr {
    &self.0
  }

  /// Converts to a [`GString`], replacing invalid UTF-8 sequences with
  /// `U+FFFD REPLACEMENT CHARACTER`.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GOsStr;
  ///
  /// assert_eq!(GOsStr::from("abc").to_gstring_lossy(), "abc");
  /// ```
  pub fn to_gstring_lossy(&self) -> GString {
    GString::from(self.0.to_string_lossy().as_ref())
  }

  /// Converts to a [`GString`] if the contents are valid UTF-8.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GOsStr;
  ///
  /// assert_eq!(GOsStr::from("abc").to_gstring().unwrap(), "abc");
  /// ```
  pub fn to_gstring(&self) -> Option<GString> {
    self.0.to_str().map(GString::from)
  }
}

impl Default for GOsStr {
  fn default() -> Self {
    GOsStr::new()
  }
}

impl Deref for GOsStr {
  type Target = OsStr;
  fn deref(&self) -> &OsStr {
    &self.0
  }
}

impl AsRef<OsStr> for GOsStr {
  fn as_ref(&self) -> &OsStr {
    &self.0
  }
}

impl AsRef<Path> for GOsStr {
  fn as_ref(&self) -> &Path {
    Path::new(&*self.0)
  }
}

impl Borrow<OsStr> for GOsStr {
  fn borrow(&self) -> &OsStr {
    &self.0
  }
}

impl From<&OsStr> for GOsStr {
  fn from(s: &OsStr) -> Self {
    GOsStr(Arc::from(s))
  }
}

impl From<OsString> for GOsStr {
  fn from(s: OsString) -> Self {
    GOsStr(Arc::from(s))
  }
}

impl From<&OsString> for GOsStr {
  fn from(s: &OsString) -> Self {
    GOsStr::from(s.as_os_str())
  }
}

impl From<&str> for GOsStr {
  fn from(s: &str) -> Self {
    GOsStr::from(OsStr::new(s))
  }
}

impl From<String> for GOsStr {
  fn from(s: String) -> Self {
    GOsStr::from(OsString::from(s))
  }
}

impl From<&String> for GOsStr {
  fn from(s: &String) -> Self {
    GOsStr::from(s.as_str())
  }
}

impl From<&GString> for GOsStr {
  fn from(s: &GString) -> Self {
    GOsStr::from(s.as_str())
  }
}

impl From<GString> for GOsStr {
  fn from(s: GString) -> Self {
    GOsStr::from(s.as_str())
  }
}

impl From<&GOsStr> for GOsStr {
  fn from(s: &GOsStr) -> Self {
    s.clone()
  }
}

impl From<GOsStr> for OsString {
  fn from(s: GOsStr) -> Self {
    s.0.to_os_string()
  }
}

impl From<GOsStr> for Arc<OsStr> {
  fn from(s: GOsStr) -> Self {
    s.0
  }
}

impl fmt::Debug for GOsStr {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Debug::fmt(&*self.0, f)
  }
}

impl PartialEq<OsStr> for GOsStr {
  fn eq(&self, other: &OsStr) -> bool {
    &*self.0 == other
  }
}

impl PartialEq<&OsStr> for GOsStr {
  fn eq(&self, other: &&OsStr) -> bool {
    &*self.0 == *other
  }
}

impl PartialEq<OsString> for GOsStr {
  fn eq(&self, other: &OsString) -> bool {
    &*self.0 == other.as_os_str()
  }
}

impl PartialEq<str> for GOsStr {
  fn eq(&self, other: &str) -> bool {
    &*self.0 == other
  }
}

impl PartialEq<&str> for GOsStr {
  fn eq(&self, other: &&str) -> bool {
    &*self.0 == *other
  }
}

impl PartialEq<String> for GOsStr {
  fn eq(&self, other: &String) -> bool {
    &*self.0 == other.as_str()
  }
}

impl PartialEq<GString> for GOsStr {
  fn eq(&self, other: &GString) -> bool {
    &*self.0 == other.as_str()
  }
}

impl PartialEq<GOsStr> for &str {
  fn eq(&self, other: &GOsStr) -> bool {
    other == self
  }
}

impl PartialEq<GOsStr> for str {
  fn eq(&self, other: &GOsStr) -> bool {
    other == self
  }
}

impl PartialEq<GOsStr> for OsStr {
  fn eq(&self, other: &GOsStr) -> bool {
    other == self
  }
}
