use crate::pointers::{DefaultStrategy, MutablePointer, Strategy};
use std::{borrow::Borrow, collections::BTreeMap, ops::RangeBounds};

/// A shared, sorted map. Cloning yields another handle to the same map.
///
/// The `S` [`Strategy`] picks the [`MutablePointer`] the data lives in and
/// defaults to [`DefaultStrategy`]. Use [`Dict::with_strategy`] to pick another.
/// Keys and values must be [`Clone`] because the pointers may need to copy the
/// whole map. Values are stored by value, so accessors such as [`Dict::get`]
/// and [`Dict::iter`] return clones rather than references.
///
/// ```
/// use galvanize_rs::{ArcSwapStrategy, Dict};
///
/// let a: Dict<String, i32> = Dict::new();
/// let b: Dict<String, i32, ArcSwapStrategy> = Dict::with_strategy();
/// ```
pub struct Dict<K: Clone, V: Clone, S: Strategy = DefaultStrategy>(S::Ptr<BTreeMap<K, V>>);

impl<K: Ord + Clone, V: Clone> Dict<K, V> {
  /// Creates an empty dict using the [`DefaultStrategy`].
  pub fn new() -> Self {
    Self::with_strategy()
  }
}

impl<K: Ord + Clone, V: Clone, S: Strategy> Dict<K, V, S> {
  /// Creates an empty dict using the strategy `S`.
  pub fn with_strategy() -> Self {
    Self(S::Ptr::new(BTreeMap::new()))
  }

  /// Returns the number of entries.
  pub fn len(&self) -> usize {
    self.0.read_with(|v| v.len())
  }

  /// Returns `true` if the dict has no entries.
  pub fn is_empty(&self) -> bool {
    self.0.read_with(|v| v.is_empty())
  }

  /// Returns `true` if the dict contains `key`.
  pub fn contains_key<Q>(&self, key: &Q) -> bool
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
  {
    self.0.read_with(|v| v.contains_key(key))
  }

  /// Returns a clone of the value for `key`, or `None` if it is absent.
  pub fn get<Q>(&self, key: &Q) -> Option<V>
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
  {
    self.0.read_with(|v| v.get(key).cloned())
  }

  /// Inserts `value` under `key` and returns the previous value, if any.
  pub fn insert(&self, key: impl Into<K>, value: impl Into<V>) -> Option<V> {
    let (key, value) = (key.into(), value.into());
    self.0.write_with(|v| v.insert(key.clone(), value.clone()))
  }

  /// Returns a clone of the value for `key`, first inserting `f()` if absent.
  ///
  /// `f` is called at most once, and only when the key is missing.
  pub fn get_or_insert_with(&self, key: impl Into<K>, f: impl FnOnce() -> V) -> V {
    let key = key.into();
    let mut f = Some(f);
    // Cached so that a retried write reuses the value instead of calling `f` again.
    let mut made: Option<V> = None;
    self.0.write_with(|v| {
      v.entry(key.clone())
        .or_insert_with(|| {
          made
            .get_or_insert_with(|| (f.take().expect("f is only called once"))())
            .clone()
        })
        .clone()
    })
  }

  /// Removes `key` and returns its value, or `None` if it was absent.
  pub fn remove<Q>(&self, key: &Q) -> Option<V>
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
  {
    self.0.write_with(|v| v.remove(key))
  }

  /// Removes `key` and returns the stored key and value, or `None` if absent.
  pub fn remove_entry<Q>(&self, key: &Q) -> Option<(K, V)>
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
  {
    self.0.write_with(|v| v.remove_entry(key))
  }

  /// Removes and returns the entry with the smallest key, if any.
  pub fn pop_first(&self) -> Option<(K, V)> {
    self.0.write_with(|v| v.pop_first())
  }

  /// Removes and returns the entry with the largest key, if any.
  pub fn pop_last(&self) -> Option<(K, V)> {
    self.0.write_with(|v| v.pop_last())
  }

  /// Removes all entries.
  pub fn clear(&self) {
    self.0.write_with(|v| v.clear());
  }

  /// Keeps only the entries for which `f` returns `true`.
  ///
  /// `f` may be called more than once per entry if the underlying pointer
  /// retries the write.
  pub fn retain(&self, mut f: impl FnMut(&K, &V) -> bool) {
    self.0.write_with(|v| v.retain(|k, x| f(k, x)));
  }
}

impl<K: Ord + Clone, V: Clone, S: Strategy> Dict<K, V, S> {
  /// Returns clones of the stored key and value for `key`, or `None` if absent.
  pub fn get_key_value<Q>(&self, key: &Q) -> Option<(K, V)>
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
  {
    self
      .0
      .read_with(|v| v.get_key_value(key).map(|(k, x)| (k.clone(), x.clone())))
  }

  /// Returns a clone of the entry with the smallest key, if any.
  pub fn first_key_value(&self) -> Option<(K, V)> {
    self
      .0
      .read_with(|v| v.first_key_value().map(|(k, x)| (k.clone(), x.clone())))
  }

  /// Returns a clone of the entry with the largest key, if any.
  pub fn last_key_value(&self) -> Option<(K, V)> {
    self
      .0
      .read_with(|v| v.last_key_value().map(|(k, x)| (k.clone(), x.clone())))
  }

  /// Returns an iterator, in key order, over a snapshot of the entries whose
  /// keys fall within `range`.
  ///
  /// # Panics
  ///
  /// Panics if the range start is greater than its end, or if both bounds are
  /// equal and excluded.
  pub fn range<Q, R>(&self, range: R) -> std::vec::IntoIter<(K, V)>
  where
    K: Borrow<Q>,
    Q: Ord + ?Sized,
    R: RangeBounds<Q>,
  {
    self
      .0
      .read_with(|v| v.range(range).map(|(k, x)| (k.clone(), x.clone())).collect::<Vec<_>>())
      .into_iter()
  }

  /// Returns an iterator, in key order, over a snapshot of the keys.
  pub fn keys(&self) -> std::vec::IntoIter<K> {
    self.0.read_with(|v| v.keys().cloned().collect::<Vec<_>>()).into_iter()
  }

  /// Returns an iterator, in key order, over a snapshot of the values.
  pub fn values(&self) -> std::vec::IntoIter<V> {
    self
      .0
      .read_with(|v| v.values().cloned().collect::<Vec<_>>())
      .into_iter()
  }

  /// Returns an iterator, in key order, over a snapshot of the entries.
  ///
  /// The whole map is cloned up front, and later changes to the dict are not
  /// reflected by the iterator.
  pub fn iter(&self) -> std::collections::btree_map::IntoIter<K, V> {
    self.0.read_with(|v| v.clone()).into_iter()
  }

  /// Returns an independent dict holding clones of the entries.
  ///
  /// Unlike [`Clone::clone`], changes to the copy do not affect the original.
  pub fn deep_clone(&self) -> Self {
    Self(self.0.deep_clone())
  }
}

/// Equivalent to [`Dict::with_strategy`].
impl<K: Ord + Clone, V: Clone, S: Strategy> Default for Dict<K, V, S> {
  fn default() -> Self {
    Self::with_strategy()
  }
}

/// Clones the handle, not the data; both handles refer to the same dict.
/// Use [`Dict::deep_clone`] for an independent copy.
impl<K: Clone, V: Clone, S: Strategy> Clone for Dict<K, V, S> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

/// Formats a snapshot of the entries like a [`BTreeMap`].
impl<K: Clone + std::fmt::Debug, V: Clone + std::fmt::Debug, S: Strategy> std::fmt::Debug for Dict<K, V, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // Snapshot first so no lock is held while formatting entries, which may be
    // handles that refer back to this dict.
    std::fmt::Debug::fmt(&self.0.read_with(BTreeMap::clone), f)
  }
}

/// Builds a dict from key-value pairs. Later pairs replace earlier ones with
/// the same key.
impl<K: Ord + Clone, V: Clone, S: Strategy> FromIterator<(K, V)> for Dict<K, V, S> {
  fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
    Self(S::Ptr::new(iter.into_iter().collect()))
  }
}

/// Adds key-value pairs to the dict, replacing the values of existing keys.
impl<K: Ord + Clone, V: Clone, S: Strategy> Extend<(K, V)> for Dict<K, V, S> {
  fn extend<I: IntoIterator<Item = (K, V)>>(&mut self, iter: I) {
    let items: Vec<_> = iter.into_iter().collect();
    self.0.write_with(|m| m.extend(items.iter().cloned()));
  }
}

/// Creates a [`Dict`] from `key => value` pairs.
///
/// ```
/// use galvanize_rs::{Dict, dict};
///
/// let d: Dict<String, i32> = dict! { "a" => 1, "b" => 2 };
/// assert_eq!(d.get("a"), Some(1));
/// ```
#[macro_export]
macro_rules! dict {
  ($($key:expr => $value:expr),* $(,)?) => {{
    let dict = $crate::Dict::new();
    $(dict.insert($key, $value);)*
    dict
  }};
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::pointers::{ArcMutexStrategy, ArcRwLockStrategy, ArcSwapStrategy};

  fn exercise<S: Strategy>() {
    let dict: Dict<String, i32, S> = Dict::with_strategy();
    assert_eq!(dict.insert("a", 1), None);
    assert_eq!(dict.insert("a", 2), Some(1));
    assert_eq!(dict.get_or_insert_with("b", || 5), 5);
    assert_eq!(dict.get_or_insert_with("b", || unreachable!()), 5);

    let shared = dict.clone();
    shared.remove("a");
    assert_eq!(dict.len(), 1);

    let copy = dict.deep_clone();
    copy.clear();
    assert_eq!(dict.len(), 1);
  }

  #[test]
  fn works_with_every_strategy() {
    exercise::<ArcRwLockStrategy>();
    exercise::<ArcMutexStrategy>();
    exercise::<ArcSwapStrategy>();
  }
}
