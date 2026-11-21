use crate::pointers::{DefaultPointerStrategy, MutablePointer, PointerStrategy};

/// A shared, growable list. Cloning yields another handle to the same list.
///
/// The `S` [`PointerStrategy`] picks the [`MutablePointer`] the data lives in and
/// defaults to [`DefaultPointerStrategy`]. Use [`GList::with_strategy`] to pick another.
///
/// Elements are stored by value and must be [`Clone`]: accessors such as
/// [`GList::get`] and [`GList::iter`] return clones rather than references.
///
/// ```
/// use galvanize_rs::{ArcSwapStrategy, GList};
///
/// let a: GList<i32> = GList::new();
/// let b: GList<i32, ArcSwapStrategy> = GList::with_strategy();
/// ```
pub struct GList<T: Clone, S: PointerStrategy = DefaultPointerStrategy>(S::Ptr<Vec<T>>);

impl<T: Clone> GList<T> {
  /// Creates an empty list using the [`DefaultPointerStrategy`].
  pub fn new() -> Self {
    Self::with_strategy()
  }
}

impl<T: Clone, S: PointerStrategy> GList<T, S> {
  /// Creates an empty list using the strategy `S`.
  pub fn with_strategy() -> Self {
    Self(S::Ptr::new(Vec::new()))
  }

  /// Returns the number of elements.
  pub fn len(&self) -> usize {
    self.0.read_with(|v| v.len())
  }

  /// Returns `true` if the list has no elements.
  pub fn is_empty(&self) -> bool {
    self.0.read_with(|v| v.is_empty())
  }

  /// Returns a clone of the element at `index`, or `None` if out of bounds.
  pub fn get(&self, index: usize) -> Option<T> {
    self.0.read_with(|v| v.get(index).cloned())
  }

  /// Returns a clone of the first element, or `None` if the list is empty.
  pub fn first(&self) -> Option<T> {
    self.0.read_with(|v| v.first().cloned())
  }

  /// Returns a clone of the last element, or `None` if the list is empty.
  pub fn last(&self) -> Option<T> {
    self.0.read_with(|v| v.last().cloned())
  }

  /// Appends `value` to the end of the list.
  pub fn push(&self, value: impl Into<T>) {
    let value = value.into();
    self.0.write_with(|v| v.push(value.clone()));
  }

  /// Removes and returns the last element, or `None` if the list is empty.
  pub fn pop(&self) -> Option<T> {
    self.0.write_with(|v| v.pop())
  }

  /// Inserts `value` at `index`, shifting later elements to the right.
  ///
  /// # Panics
  ///
  /// Panics if `index > len`.
  pub fn insert(&self, index: usize, value: impl Into<T>) {
    let value = value.into();
    self.0.write_with(|v| v.insert(index, value.clone()));
  }

  /// Removes and returns the element at `index`, shifting later elements left.
  ///
  /// # Panics
  ///
  /// Panics if `index >= len`.
  pub fn remove(&self, index: usize) -> T {
    self.0.write_with(|v| v.remove(index))
  }

  /// Replaces the element at `index` with `value` and returns the old element.
  ///
  /// # Panics
  ///
  /// Panics if `index >= len`.
  pub fn set(&self, index: usize, value: impl Into<T>) -> T {
    let value = value.into();
    self.0.write_with(|v| std::mem::replace(&mut v[index], value.clone()))
  }

  /// Removes all elements.
  pub fn clear(&self) {
    self.0.write_with(|v| v.clear());
  }

  /// Keeps only the elements for which `f` returns `true`.
  ///
  /// `f` may be called more than once per element if the underlying pointer
  /// retries the write.
  pub fn retain(&self, mut f: impl FnMut(&T) -> bool) {
    self.0.write_with(|v| v.retain(|x| f(x)));
  }

  /// Returns an iterator over a snapshot of the list taken at call time.
  ///
  /// The whole list is cloned up front, and later changes to the list are not
  /// reflected by the iterator.
  pub fn iter(&self) -> std::vec::IntoIter<T> {
    self.0.read_with(|v| v.clone()).into_iter()
  }

  /// Returns an independent list holding clones of the elements.
  ///
  /// Unlike [`Clone::clone`], changes to the copy do not affect the original.
  pub fn deep_clone(&self) -> Self {
    Self(self.0.deep_clone())
  }
}

/// Equivalent to [`GList::with_strategy`].
impl<T: Clone, S: PointerStrategy> Default for GList<T, S> {
  fn default() -> Self {
    Self::with_strategy()
  }
}

/// Clones the handle, not the data; both handles refer to the same list.
/// Use [`GList::deep_clone`] for an independent copy.
impl<T: Clone, S: PointerStrategy> Clone for GList<T, S> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

/// Formats a snapshot of the elements like a [`Vec`].
impl<T: Clone + std::fmt::Debug, S: PointerStrategy> std::fmt::Debug for GList<T, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    // Snapshot first so no lock is held while formatting elements, which may be
    // handles that refer back to this list.
    std::fmt::Debug::fmt(&self.0.read_with(Vec::clone), f)
  }
}

/// Builds a list from the items yielded by an iterator.
impl<T: Clone, S: PointerStrategy> FromIterator<T> for GList<T, S> {
  fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
    Self(S::Ptr::new(iter.into_iter().collect()))
  }
}

/// Creates a [`GList`] containing the given values.
///
/// ```
/// use galvanize_rs::{GList, list};
///
/// let l: GList<i32> = list![1, 2, 3];
/// assert_eq!(l.len(), 3);
/// ```
#[macro_export]
macro_rules! list {
  ($($value:expr),* $(,)?) => {{
    let list = $crate::GList::new();
    $(list.push($value);)*
    list
  }};
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::pointers::{ArcMutexStrategy, ArcRwLockStrategy, ArcSwapStrategy};

  fn exercise<S: PointerStrategy>() {
    let list: GList<i32, S> = GList::with_strategy();
    list.push(1);
    list.push(3);
    list.insert(1, 2);
    assert_eq!(list.set(0, 10), 1);
    assert_eq!(list.remove(2), 3);
    assert_eq!(list.len(), 2);

    let shared = list.clone();
    shared.push(4);
    assert_eq!(list.len(), 3);

    let copy = list.deep_clone();
    copy.clear();
    assert_eq!(list.len(), 3);
  }

  #[test]
  fn works_with_every_strategy() {
    exercise::<ArcRwLockStrategy>();
    exercise::<ArcMutexStrategy>();
    exercise::<ArcSwapStrategy>();
  }
}
