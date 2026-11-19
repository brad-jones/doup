use super::MutablePointer;
use std::sync::{Arc, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};

/// A [`MutablePointer`] backed by `Arc<RwLock<T>>`.
///
/// Multiple readers may hold the value concurrently, while a writer gets
/// exclusive access. A poisoned lock is recovered rather than propagated as
/// a panic.
///
/// # Examples
///
/// ```
/// use galvanize_rs::{MutablePointer, ArcRwLockPointer};
///
/// let a = ArcRwLockPointer::new(vec![1, 2]);
/// let b = a.clone(); // same underlying value
///
/// b.write_with(|v| v.push(3));
/// assert_eq!(a.read_with(|v| v.len()), 3);
///
/// let c = a.deep_clone(); // independent copy
/// c.write_with(|v| v.clear());
/// assert_eq!(a.read_with(|v| v.len()), 3);
/// ```
///
/// # Deadlocks
///
/// The lock is not reentrant. Acquiring it again from inside a closure while
/// a conflicting guard is held deadlocks (or panics):
///
/// ```no_run
/// # use galvanize_rs::{MutablePointer, ArcRwLockPointer};
/// let c = ArcRwLockPointer::new(vec![1]);
/// // read_with waits on the exclusive lock that write_with already holds
/// c.write_with(|v| v.push(c.read_with(|v2| v2.len())));
/// ```
///
/// The reverse also deadlocks: upgrading a read lock to a write lock waits on
/// the read guard that is still held.
///
/// ```no_run
/// # use galvanize_rs::{MutablePointer, ArcRwLockPointer};
/// let c = ArcRwLockPointer::new(vec![1]);
/// // write_with waits for all readers, including this one, to finish
/// c.read_with(|v| c.write_with(|v2| v2.push(v.len())));
/// ```
///
/// Nested reads usually work, but are not guaranteed to: they can deadlock if
/// another thread is waiting to write between the two acquisitions.
///
/// Read the value first and then write it instead:
///
/// ```
/// # use galvanize_rs::{MutablePointer, ArcRwLockPointer};
/// let c = ArcRwLockPointer::new(vec![1]);
/// let n = c.read_with(|v| v.len());
/// c.write_with(|v| v.push(n));
/// assert_eq!(c.read_with(|v| v.len()), 2);
/// ```
pub struct ArcRwLockPointer<T>(Arc<RwLock<T>>);

impl<T> ArcRwLockPointer<T> {
  /// Acquires a shared guard, recovering it if the lock was poisoned.
  fn read(&self) -> RwLockReadGuard<'_, T> {
    self.0.read().unwrap_or_else(PoisonError::into_inner)
  }

  /// Acquires an exclusive guard, recovering it if the lock was poisoned.
  fn write(&self) -> RwLockWriteGuard<'_, T> {
    self.0.write().unwrap_or_else(PoisonError::into_inner)
  }
}

impl<T: Clone> MutablePointer<T> for ArcRwLockPointer<T> {
  fn new(value: T) -> Self {
    Self(Arc::new(RwLock::new(value)))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    f(&self.read())
  }

  fn write_with<R>(&self, mut f: impl FnMut(&mut T) -> R) -> R {
    f(&mut self.write())
  }
}

impl<T> Clone for ArcRwLockPointer<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  pointer_contract_tests!(ArcRwLockPointer);

  #[test]
  fn recovers_from_poisoned_lock() {
    let pointer = ArcRwLockPointer::new(1);
    let poisoner = pointer.clone();
    let _ = std::thread::spawn(move || {
      poisoner.write_with(|v| {
        *v = 2;
        panic!("poison the lock");
      })
    })
    .join();

    assert!(pointer.0.is_poisoned());
    assert_eq!(pointer.read_with(|v| *v), 2);
    pointer.write_with(|v| *v = 3);
    assert_eq!(pointer.read_with(|v| *v), 3);
  }

  #[test]
  fn allows_concurrent_readers() {
    let pointer = ArcRwLockPointer::new(1);
    pointer.read_with(|_| {
      // A second shared guard must not block while the first is held.
      assert_eq!(pointer.read_with(|v| *v), 1);
    });
  }
}
