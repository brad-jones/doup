use std::borrow::Borrow;

/// A cheaply cloneable, shared handle to a value of type `T`.
///
/// As the name suggests the underlying value is mutable, read the value via
/// [`MutablePointer::read_with`] and write using [`MutablePointer::write_with`].
///
/// If you use one of the locking pointers [`super::ArcMutexPointer`] or
/// [`super::ArcRwLockPointer`], beware of deadlocks! As a rule of thumb so long
/// as you don't attempt to read or write back to the same reference from with-in
/// the closure passed to `read/write_with` you should be deadlock free.
///
/// Cloning the pointer yields another handle to the *same* value, whereas
/// [`MutablePointer::deep_clone`] copies the value into an independent pointer.
pub trait MutablePointer<T: Clone>: Clone {
  /// Creates a new pointer owning `value`.
  fn new(value: T) -> Self;

  /// Runs `f` with shared access to the current value.
  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R;

  /// Runs `f` with exclusive access to the value.
  ///
  /// `f` is `FnMut` because implementations may retry it (e.g. an RCU loop).
  fn write_with<R>(&self, f: impl FnMut(&mut T) -> R) -> R;

  /// Returns a clone of the shared value.
  /// Shorthand for: `pointer.read_with(|inner_value| inner_value.clone())`.
  /// If you want this to be a cheap operation, make sure T is another Rc or Arc.
  fn get(&self) -> T {
    self.read_with(T::clone)
  }

  /// Replaces the shared value with a clone of `value`.
  ///
  /// Accepts anything that borrows as `T` (e.g. `T` or `&T`), so the caller
  /// keeps ownership of its value. Every handle cloned from this pointer
  /// observes the new value.
  fn set(&self, value: impl Borrow<T>) {
    let next = value.borrow();
    self.write_with(|current| *current = next.clone())
  }

  /// Replaces the shared value with `value` converted via [`Into`].
  ///
  /// Convenient when you hold a convertible type, e.g. setting a
  /// `MutablePointer<String>` from a `&str`. Every handle cloned from this
  /// pointer observes the new value.
  fn set_from(&self, value: impl Into<T>) {
    let next = value.into();
    self.write_with(|current| *current = next.clone())
  }

  /// Copies the current value into a new, independent pointer.
  fn deep_clone(&self) -> Self {
    Self::new(self.get())
  }
}
