use std::borrow::Borrow;
use std::fmt;
use std::iter::FromIterator;
use std::ops::{Add, Deref};
use std::path::Path;
use std::sync::Arc;

/// An immutable, reference-counted string.
///
/// The `G` stands for Galvanize, and definitely not for what it looks like
/// when you wear one: a `GString` is a sleek, minimal, surprisingly cheap-to-share
/// thong of text. Sexy strings, hahaha.
///
/// `GString` wraps an [`Arc<str>`], so cloning is a cheap atomic reference-count
/// increment rather than a heap allocation and copy. It is safe to share
/// across threads.
///
/// It dereferences to [`str`], so all `str` methods are available directly,
/// and it can be compared against `str`, `&str` and [`String`] values.
///
/// # Examples
///
/// ```
/// use galvanize_rs::GString;
///
/// let a = GString::from("hello");
/// let b = a.clone(); // shares the same allocation
/// assert_eq!(a, b);
/// assert_eq!(a, "hello");
/// assert_eq!(a.len(), 5);
///
/// let c = a + " world";
/// assert_eq!(c, "hello world");
/// ```
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GString(Arc<str>);

impl GString {
  /// Creates a new, empty `GString`.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GString;
  ///
  /// assert!(GString::new().is_empty());
  /// ```
  pub fn new() -> Self {
    GString::from("")
  }

  /// Returns the contents as a string slice.
  ///
  /// # Examples
  ///
  /// ```
  /// use galvanize_rs::GString;
  ///
  /// assert_eq!(GString::from("abc").as_str(), "abc");
  /// ```
  pub fn as_str(&self) -> &str {
    &self.0
  }
}

impl Default for GString {
  fn default() -> Self {
    GString::new()
  }
}

impl Deref for GString {
  type Target = str;
  fn deref(&self) -> &str {
    &self.0
  }
}

impl AsRef<str> for GString {
  fn as_ref(&self) -> &str {
    &self.0
  }
}

impl AsRef<Path> for GString {
  fn as_ref(&self) -> &Path {
    Path::new(&*self.0)
  }
}

impl AsRef<[u8]> for GString {
  fn as_ref(&self) -> &[u8] {
    &*self.0.as_bytes()
  }
}

impl Borrow<str> for GString {
  fn borrow(&self) -> &str {
    &self.0
  }
}

impl From<&str> for GString {
  fn from(s: &str) -> Self {
    GString(Arc::from(s))
  }
}

impl From<String> for GString {
  fn from(s: String) -> Self {
    GString(Arc::from(s))
  }
}

impl From<&String> for GString {
  fn from(s: &String) -> Self {
    GString::from(s.as_str())
  }
}

impl From<&GString> for GString {
  fn from(s: &GString) -> Self {
    s.clone()
  }
}

impl From<char> for GString {
  fn from(c: char) -> Self {
    GString::from(c.to_string())
  }
}

impl From<GString> for String {
  fn from(s: GString) -> Self {
    s.0.to_string()
  }
}

impl From<GString> for Arc<str> {
  fn from(s: GString) -> Self {
    s.0
  }
}

impl fmt::Display for GString {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Display::fmt(&*self.0, f)
  }
}

impl fmt::Debug for GString {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    fmt::Debug::fmt(&*self.0, f)
  }
}

impl PartialEq<str> for GString {
  fn eq(&self, other: &str) -> bool {
    &*self.0 == other
  }
}

impl PartialEq<&str> for GString {
  fn eq(&self, other: &&str) -> bool {
    &*self.0 == *other
  }
}

impl PartialEq<String> for GString {
  fn eq(&self, other: &String) -> bool {
    &*self.0 == other.as_str()
  }
}

impl PartialEq<GString> for &str {
  fn eq(&self, other: &GString) -> bool {
    *self == &*other.0
  }
}

impl PartialEq<GString> for str {
  fn eq(&self, other: &GString) -> bool {
    self == &*other.0
  }
}

impl<S: AsRef<str>> Add<S> for GString {
  type Output = GString;
  fn add(self, rhs: S) -> GString {
    let mut s = String::with_capacity(self.len() + rhs.as_ref().len());
    s.push_str(&self);
    s.push_str(rhs.as_ref());
    GString::from(s)
  }
}

impl<S: AsRef<str>> Add<S> for &GString {
  type Output = GString;
  fn add(self, rhs: S) -> GString {
    self.clone() + rhs
  }
}

impl FromIterator<char> for GString {
  fn from_iter<I: IntoIterator<Item = char>>(iter: I) -> Self {
    GString::from(iter.into_iter().collect::<String>())
  }
}

/// Converts a value into a [`GString`] using [`GString::from`].
///
/// # Examples
///
/// ```
/// use galvanize_rs::s;
///
/// assert_eq!(s!("hello"), "hello");
/// ```
#[macro_export]
macro_rules! s {
  ($value:expr) => {
    $crate::GString::from($value)
  };
}

/// Formats arguments like [`format!`] and returns the result as a [`GString`].
///
/// # Examples
///
/// ```
/// use galvanize_rs::sf;
///
/// assert_eq!(sf!("{}-{}", 1, 2), "1-2");
/// ```
#[macro_export]
macro_rules! sf {
    ($($arg:tt)*) => {
        $crate::GString::from(::std::format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::collections::HashMap;

  #[test]
  fn new_and_default_are_empty() {
    assert!(GString::new().is_empty());
    assert_eq!(GString::default(), GString::new());
  }

  #[test]
  fn from_conversions() {
    let expected = "abc";
    assert_eq!(GString::from("abc"), expected);
    assert_eq!(GString::from(String::from("abc")), expected);
    assert_eq!(GString::from(&String::from("abc")), expected);
    let original = GString::from("abc");
    assert_eq!(GString::from(&original), expected);
    assert_eq!(GString::from('a'), "a");
    assert_eq!(GString::from('é'), "é");
  }

  #[test]
  fn into_string_and_arc() {
    assert_eq!(String::from(GString::from("abc")), "abc");
    assert_eq!(&*Arc::<str>::from(GString::from("abc")), "abc");
  }

  #[test]
  fn clone_shares_allocation() {
    let a = GString::from("shared");
    let b = a.clone();
    assert!(std::ptr::eq(a.as_str(), b.as_str()));
  }

  #[test]
  fn deref_exposes_str_methods() {
    let s = GString::from("Hello");
    assert_eq!(s.len(), 5);
    assert!(s.starts_with("He"));
    assert_eq!(s.to_uppercase(), "HELLO");
    assert_eq!(s.as_str(), "Hello");
    assert_eq!(AsRef::<str>::as_ref(&s), "Hello");
  }

  #[test]
  fn display_and_debug_match_str() {
    let s = GString::from("a\"b");
    assert_eq!(format!("{s}"), "a\"b");
    assert_eq!(format!("{s:?}"), format!("{:?}", "a\"b"));
    assert_eq!(format!("{:>5}", GString::from("ab")), "   ab");
  }

  #[test]
  fn equality_with_other_string_types() {
    let s = GString::from("abc");
    assert!(s == *"abc");
    assert!(s == "abc");
    let owned = String::from("abc");
    assert!(s == owned);
    assert!("abc" == s);
    assert!(*"abc" == s);
    assert!(s != "abd");
    let other = GString::from("abd");
    assert!(s != other);
  }

  #[test]
  fn ordering_matches_str() {
    let (empty, a, b) = (GString::new(), GString::from("a"), GString::from("b"));
    assert!(a < b);
    assert!(empty < a);
    assert_eq!(b.cmp(&a), std::cmp::Ordering::Greater);
  }

  #[test]
  fn borrow_allows_str_lookup_in_maps() {
    let mut map: HashMap<GString, i32> = HashMap::new();
    map.insert(GString::from("key"), 1);
    assert_eq!(map.get("key"), Some(&1));
    assert_eq!(map.get("missing"), None);
  }

  #[test]
  fn add_concatenates_without_mutating_borrowed_lhs() {
    let a = GString::from("foo");
    assert_eq!(&a + "bar", "foobar");
    assert_eq!(&a + String::from("baz"), "foobaz");
    assert_eq!(&a + GString::from("!"), "foo!");
    assert_eq!(a, "foo");
    assert_eq!(a + "", "foo");
  }

  #[test]
  fn from_iterator_of_chars() {
    let s: GString = "abc".chars().rev().collect();
    assert_eq!(s, "cba");
    assert_eq!(Vec::<char>::new().into_iter().collect::<GString>(), "");
  }

  #[test]
  fn macros() {
    assert_eq!(s!("x"), "x");
    assert_eq!(s!(String::from("y")), "y");
    assert_eq!(sf!("{}-{:02}", "a", 7), "a-07");
  }

  #[test]
  fn is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GString>();
  }
}
