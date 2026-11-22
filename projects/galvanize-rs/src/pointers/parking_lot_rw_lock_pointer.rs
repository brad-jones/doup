use super::MutablePointer;
use parking_lot::RwLock;
use std::sync::Arc;

/// A [`MutablePointer`] backed by `Arc<parking_lot::RwLock<T>>`.
///
/// Like [`super::ArcRwLockPointer`] but using the `parking_lot` lock, which is
/// smaller and fair to writers (a waiting writer blocks new readers). It never
/// poisons, so a panic inside a closure simply releases the lock.
///
/// # Examples
///
/// ```
/// use galvanize_rs::{MutablePointer, ParkingLotRwLockPointer};
///
/// let a = ParkingLotRwLockPointer::new(vec![1, 2]);
/// let b = a.clone(); // same underlying value
///
/// b.write_with(|v| v.push(3));
/// assert_eq!(a.read_with(|v| v.len()), 3);
/// ```
///
/// # Deadlocks
///
/// The lock is not reentrant. Calling back into the same pointer from inside
/// a closure can deadlock, including nested reads when a writer is waiting.
/// Read the value first and then write it instead.
pub struct ParkingLotRwLockPointer<T>(Arc<RwLock<T>>);

impl<T: Clone> MutablePointer<T> for ParkingLotRwLockPointer<T> {
  fn new(value: T) -> Self {
    Self(Arc::new(RwLock::new(value)))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    f(&self.0.read())
  }

  fn write_with<R>(&self, mut f: impl FnMut(&mut T) -> R) -> R {
    f(&mut self.0.write())
  }

  fn ptr_eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.0, &other.0)
  }
}

impl<T> Clone for ParkingLotRwLockPointer<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  pointer_contract_tests!(ParkingLotRwLockPointer);

  #[test]
  fn survives_panicking_writer() {
    let pointer = ParkingLotRwLockPointer::new(1);
    let poisoner = pointer.clone();
    let _ = std::thread::spawn(move || {
      poisoner.write_with(|v| {
        *v = 2;
        panic!("panic while locked");
      })
    })
    .join();

    assert_eq!(pointer.read_with(|v| *v), 2);
    pointer.write_with(|v| *v = 3);
    assert_eq!(pointer.read_with(|v| *v), 3);
  }
}
