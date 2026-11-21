/// A cheaply cloneable, shared handle to a value of type `T`.
///
/// As the name suggests the underlying value is mutable, read the value via
/// [`GPointer::read_with`] and write using [`GPointer::write_with`].
///
/// If you use one of the locking pointers [`super::ArcMutexPointer`] or
/// [`super::ArcRwLockPointer`], beware of deadlocks! As a rule of thumb so long
/// as you don't attempt to read or write back to the same reference from with-in
/// the closure passed to `read/write_with` you should be deadlock free.
///
/// Cloning the pointer yields another handle to the *same* value, whereas
/// [`GPointer::deep_clone`] copies the value into an independent pointer.
pub trait GPointer<T: Clone>: Clone {
  /// Creates a new pointer owning `value`.
  fn new(value: T) -> Self;

  /// Runs `f` with shared access to the current value.
  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R;

  /// Runs `f` with exclusive access to the value.
  ///
  /// `f` is `FnMut` because implementations may retry it (e.g. an RCU loop).
  fn write_with<R>(&self, f: impl FnMut(&mut T) -> R) -> R;

  /// Copies the current value into a new, independent pointer.
  fn deep_clone(&self) -> Self {
    Self::new(self.read_with(T::clone))
  }
}
