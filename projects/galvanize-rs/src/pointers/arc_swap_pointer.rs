use super::MutablePointer;
use arc_swap::ArcSwap;
use std::sync::Arc;

/// A [`MutablePointer`] backed by `Arc<ArcSwap<T>>`.
///
/// Reads are lock-free. Writes clone the current value, apply the closure to
/// the copy and atomically swap it in (read-copy-update), retrying if another
/// writer got there first. The closure passed to `write_with` may therefore
/// run more than once, so it should be free of side effects.
///
/// # Examples
///
/// ```
/// use galvanize_rs::{MutablePointer, ArcSwapPointer};
///
/// let a = ArcSwapPointer::new(vec![1, 2]);
/// let b = a.clone(); // same underlying value
///
/// b.write_with(|v| v.push(3));
/// assert_eq!(a.read_with(|v| v.len()), 3);
///
/// let c = a.deep_clone(); // independent copy
/// c.write_with(|v| v.clear());
/// assert_eq!(a.read_with(|v| v.len()), 3);
/// ```
pub struct ArcSwapPointer<T>(Arc<ArcSwap<T>>);

impl<T: Clone> MutablePointer<T> for ArcSwapPointer<T> {
  fn new(value: T) -> Self {
    Self(Arc::new(ArcSwap::from_pointee(value)))
  }

  fn read_with<R>(&self, f: impl FnOnce(&T) -> R) -> R {
    f(self.0.load().as_ref())
  }

  fn write_with<R>(&self, mut f: impl FnMut(&mut T) -> R) -> R {
    let mut result = None;

    self.0.rcu(|current| {
      let mut next = T::clone(current);
      result = Some(f(&mut next));
      next
    });

    result.expect("rcu runs the closure at least once")
  }
}

impl<T> Clone for ArcSwapPointer<T> {
  fn clone(&self) -> Self {
    Self(self.0.clone())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  pointer_contract_tests!(ArcSwapPointer);

  #[test]
  fn write_does_not_mutate_snapshots_in_place() {
    let pointer = ArcSwapPointer::new(vec![1]);
    let before = pointer.0.load_full();
    pointer.write_with(|v| v.push(2));

    assert_eq!(*before, vec![1]);
    assert_eq!(pointer.read_with(|v| v.clone()), vec![1, 2]);
  }

  #[test]
  fn retries_closure_on_contention() {
    let pointer = ArcSwapPointer::new(0);
    let rival = pointer.clone();
    let mut calls = 0;

    let result = pointer.write_with(|v| {
      calls += 1;
      if calls == 1 {
        // Interfere once so the compare-and-swap fails and the closure reruns.
        rival.write_with(|r| *r += 10);
      }
      *v += 1;
      *v
    });

    assert_eq!(calls, 2);
    assert_eq!(result, 11);
    assert_eq!(pointer.read_with(|v| *v), 11);
  }
}
