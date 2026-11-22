use super::{DefaultPointerStrategy, MutablePointer, PointerStrategy};
use std::borrow::Borrow;

/// A cheaply cloneable, shared handle to a value of type `T`.
///
/// This is the everyday pointer type: all of its methods are inherent, so
/// there is no need to import [`MutablePointer`]. The `S` [`PointerStrategy`]
/// picks the underlying pointer and defaults to [`DefaultPointerStrategy`].
///
/// Cloning yields another handle to the *same* value, whereas
/// [`GPointer::duplicate`] copies the value into an independent pointer.
/// See [`MutablePointer`] for the deadlock rules of `read_with`/`write_with`.
///
/// ```
/// use galvanize_rs::{ArcSwapStrategy, GPointer};
///
/// let a = GPointer::new(1);
/// let b = a.clone();
/// b.write_with(|v| *v += 1);
/// assert_eq!(a.get(), 2);
///
/// let c: GPointer<i32, ArcSwapStrategy> = GPointer::with_strategy(0);
/// c.set(5);
/// assert_eq!(c.get(), 5);
/// ```
pub struct GPointer<T: Clone, S: PointerStrategy = DefaultPointerStrategy>(S::Ptr<T>);

impl<T: Clone> GPointer<T> {
  /// Creates a new pointer owning `value`, using the [`DefaultPointerStrategy`].
  pub fn new(value: T) -> Self {
    Self::with_strategy(value)
  }
}

impl<T: Clone, S: PointerStrategy> GPointer<T, S> {
  /// Creates a new pointer owning `value`, using the strategy `S`.
  pub fn with_strategy(value: T) -> Self {
    Self(S::Ptr::new(value))
  }

  /// Runs `f` with shared access to the current value.
  pub fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    self.0.read_with(f)
  }

  /// Runs `f` with exclusive access to the value.
  ///
  /// `f` is `FnMut` because some strategies may retry it.
  pub fn write_with<R>(&self, f: impl FnMut(&mut T) -> R) -> R {
    self.0.write_with(f)
  }

  /// Returns a clone of the shared value.
  pub fn get(&self) -> T {
    self.0.get()
  }

  /// Replaces the shared value with a clone of `value`.
  pub fn set(&self, value: impl Borrow<T>) {
    self.0.set(value)
  }

  /// Replaces the shared value with `value` converted via [`Into`].
  pub fn set_from(&self, value: impl Into<T>) {
    self.0.set_from(value)
  }

  /// Returns `true` if both pointers share the same underlying value (one is a
  /// clone of the other), unlike `==` which compares the values.
  pub fn ptr_eq(&self, other: &Self) -> bool {
    self.0.ptr_eq(&other.0)
  }

  /// Copies the current value into a new, independent pointer.
  ///
  /// This is one level deep: the value itself is cloned with [`Clone`], so any handles inside it (nested
  /// pointers, lists, classes) are still shared with the original.
  pub fn duplicate(&self) -> Self {
    Self(self.0.duplicate())
  }
}

impl<T: Clone, S: PointerStrategy> Clone for GPointer<T, S> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

/// Compares the pointed-to values, not pointer identity.
impl<T: Clone + PartialEq, S: PointerStrategy> PartialEq for GPointer<T, S> {
  fn eq(&self, other: &Self) -> bool {
    // Snapshot one side first so we never hold two locks at once (a pointer compared with
    // its own clone would otherwise risk a deadlock).
    let lhs = self.get();
    other.read_with(|rhs| lhs == *rhs)
  }
}

impl<T: Clone + Eq, S: PointerStrategy> Eq for GPointer<T, S> {}

/// Compares the pointed-to value with a plain `T`, so `pointer == 2` works.
impl<T: Clone + PartialEq, S: PointerStrategy> PartialEq<T> for GPointer<T, S> {
  fn eq(&self, other: &T) -> bool {
    self.read_with(|v| v == other)
  }
}

impl<T: Clone + std::fmt::Display, S: PointerStrategy> std::fmt::Display for GPointer<T, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    self.read_with(|v| std::fmt::Display::fmt(v, f))
  }
}

impl<T: Clone> From<T> for GPointer<T> {
  fn from(value: T) -> Self {
    Self::new(value)
  }
}

impl<T: Clone + Default> Default for GPointer<T> {
  fn default() -> Self {
    Self::new(T::default())
  }
}

impl<T: Clone + std::fmt::Debug, S: PointerStrategy> std::fmt::Debug for GPointer<T, S> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    let strategy = crate::short_type_name::<S>();
    self
      .0
      .read_with(|v| f.debug_tuple(&format!("GPointer<{strategy}>")).field(v).finish())
  }
}

#[cfg(test)]
mod debug_tests {
  use super::*;
  use crate::ArcSwapStrategy;

  #[test]
  fn debug_names_the_pointer_strategy() {
    let default = GPointer::new(1);
    let swap: GPointer<i32, ArcSwapStrategy> = GPointer::with_strategy(2);
    assert_eq!(format!("{default:?}"), "GPointer<ArcRwLockStrategy>(1)");
    assert_eq!(format!("{swap:?}"), "GPointer<ArcSwapStrategy>(2)");
  }
}
