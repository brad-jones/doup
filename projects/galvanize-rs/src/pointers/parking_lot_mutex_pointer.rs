use super::MutablePointer;
use parking_lot::Mutex;
use std::sync::Arc;

/// A [`MutablePointer`] backed by `Arc<parking_lot::Mutex<T>>`.
///
/// Like [`super::ArcMutexPointer`] but using the `parking_lot` mutex, which is
/// smaller and usually faster under contention. It never poisons, so a panic
/// inside a closure simply releases the lock.
///
/// # Examples
///
/// ```
/// use galvanize_rs::{MutablePointer, ParkingLotMutexPointer};
///
/// let a = ParkingLotMutexPointer::new(vec![1, 2]);
/// let b = a.clone(); // same underlying value
///
/// b.write_with(|v| v.push(3));
/// assert_eq!(a.read_with(|v| v.len()), 3);
/// ```
///
/// # Deadlocks
///
/// The lock is not reentrant. Calling back into the same pointer from inside
/// a closure deadlocks; read the value first and then write it instead.
pub struct ParkingLotMutexPointer<T>(Arc<Mutex<T>>);

impl<T: Clone> MutablePointer<T> for ParkingLotMutexPointer<T> {
  fn new(value: T) -> Self {
    Self(Arc::new(Mutex::new(value)))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    f(&self.0.lock())
  }

  fn write_with<R>(&self, mut f: impl FnMut(&mut T) -> R) -> R {
    f(&mut self.0.lock())
  }

  fn ptr_eq(&self, other: &Self) -> bool {
    Arc::ptr_eq(&self.0, &other.0)
  }
}

impl<T> Clone for ParkingLotMutexPointer<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  pointer_contract_tests!(ParkingLotMutexPointer);

  #[test]
  fn survives_panicking_writer() {
    let pointer = ParkingLotMutexPointer::new(1);
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
