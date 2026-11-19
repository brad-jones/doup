use super::MutablePointer;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A [`MutablePointer`] backed by `Arc<Mutex<T>>`.
///
/// Readers and writers are serialised by a single lock, so this suits
/// write-heavy or short critical sections. A poisoned lock is recovered
/// rather than propagated as a panic.
///
/// # Examples
///
/// ```
/// use galvanize_rs::{MutablePointer, ArcMutexPointer};
///
/// let a = ArcMutexPointer::new(vec![1, 2]);
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
/// The lock is not reentrant. Calling back into the same pointer from inside
/// a closure deadlocks (or panics):
///
/// ```no_run
/// # use galvanize_rs::{MutablePointer, ArcMutexPointer};
/// let c = ArcMutexPointer::new(vec![1]);
/// // read_with waits on the lock that write_with already holds
/// c.write_with(|v| v.push(c.read_with(|v2| v2.len())));
/// ```
///
/// Read the value first and then write it instead:
///
/// ```
/// # use galvanize_rs::{MutablePointer, ArcMutexPointer};
/// let c = ArcMutexPointer::new(vec![1]);
/// let n = c.read_with(|v| v.len());
/// c.write_with(|v| v.push(n));
/// assert_eq!(c.read_with(|v| v.len()), 2);
/// ```
pub struct ArcMutexPointer<T>(Arc<Mutex<T>>);

impl<T> ArcMutexPointer<T> {
  /// Acquires the lock, recovering the guard if the mutex was poisoned.
  fn lock(&self) -> MutexGuard<'_, T> {
    self.0.lock().unwrap_or_else(PoisonError::into_inner)
  }
}

impl<T: Clone> MutablePointer<T> for ArcMutexPointer<T> {
  fn new(value: T) -> Self {
    Self(Arc::new(Mutex::new(value)))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    f(&self.lock())
  }

  fn write_with<R>(&self, mut f: impl FnMut(&mut T) -> R) -> R {
    f(&mut self.lock())
  }
}

impl<T> Clone for ArcMutexPointer<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  pointer_contract_tests!(ArcMutexPointer);

  #[test]
  fn recovers_from_poisoned_lock() {
    let pointer = ArcMutexPointer::new(1);
    let poisoner = pointer.clone();
    let _ = std::thread::spawn(move || {
      poisoner.write_with(|v| {
        *v = 2;
        panic!("poison the mutex");
      })
    })
    .join();

    assert!(pointer.0.is_poisoned());
    assert_eq!(pointer.read_with(|v| *v), 2);
    pointer.write_with(|v| *v = 3);
    assert_eq!(pointer.read_with(|v| *v), 3);
  }
}
