//! Compares the [`GPointer`] implementations under different workloads.
//!
//! Run with `cargo bench -p galvanize-rs`, or filter with e.g.
//! `cargo bench -p galvanize-rs -- mixed`.

use divan::{Bencher, black_box};
use galvanize_rs::{ArcMutexPointer, ArcRwLockPointer, ArcSwapPointer, GPointer};
use std::cell::Cell;

fn main() {
  divan::main();
}

/// Sizes of the pointed-to `Vec<u64>`. `ArcSwapPointer` clones the whole value
/// on every write, so the size is what separates it from the lock-based ones.
const SIZES: [usize; 2] = [16, 4096];

/// Thread counts. `1` measures uncontended cost, higher values measure contention.
const THREADS: [usize; 3] = [1, 2, 8];

fn pointer<P: GPointer<Vec<u64>>>(len: usize) -> P {
  P::new(vec![1; len])
}

/// Read-only workload.
#[divan::bench(types = [ArcMutexPointer<Vec<u64>>, ArcRwLockPointer<Vec<u64>>, ArcSwapPointer<Vec<u64>>], args = SIZES, threads = THREADS)]
fn read<P: GPointer<Vec<u64>> + Sync>(bencher: Bencher, len: usize) {
  let p = pointer::<P>(len);
  bencher.bench(|| p.read_with(|v| black_box(v[len / 2])));
}

/// Write-only workload.
#[divan::bench(types = [ArcMutexPointer<Vec<u64>>, ArcRwLockPointer<Vec<u64>>, ArcSwapPointer<Vec<u64>>], args = SIZES, threads = THREADS)]
fn write<P: GPointer<Vec<u64>> + Sync>(bencher: Bencher, len: usize) {
  let p = pointer::<P>(len);
  bencher.bench(|| p.write_with(|v| v[len / 2] = black_box(v[len / 2]).wrapping_add(1)));
}

/// One write for every nine reads, a common shape for shared state.
#[divan::bench(types = [ArcMutexPointer<Vec<u64>>, ArcRwLockPointer<Vec<u64>>, ArcSwapPointer<Vec<u64>>], args = SIZES, threads = THREADS)]
fn mixed<P: GPointer<Vec<u64>> + Sync>(bencher: Bencher, len: usize) {
  thread_local! {
    static OPS: Cell<usize> = const { Cell::new(0) };
  }

  let p = pointer::<P>(len);
  bencher.bench(|| {
    let op = OPS.with(|ops| {
      ops.set(ops.get().wrapping_add(1));
      ops.get()
    });

    if op.is_multiple_of(10) {
      p.write_with(|v| v[len / 2] = black_box(v[len / 2]).wrapping_add(1));
    } else {
      black_box(p.read_with(|v| v[len / 2]));
    }
  });
}

/// Cost of `clone()` (an `Arc` bump) versus `deep_clone()` (copies the value).
#[divan::bench(types = [ArcMutexPointer<Vec<u64>>, ArcRwLockPointer<Vec<u64>>, ArcSwapPointer<Vec<u64>>])]
fn clone<P: GPointer<Vec<u64>> + Sync>(bencher: Bencher) {
  let p = pointer::<P>(16);
  bencher.bench(|| black_box(&p).clone());
}

#[divan::bench(types = [ArcMutexPointer<Vec<u64>>, ArcRwLockPointer<Vec<u64>>, ArcSwapPointer<Vec<u64>>], args = SIZES)]
fn deep_clone<P: GPointer<Vec<u64>> + Sync>(bencher: Bencher, len: usize) {
  let p = pointer::<P>(len);
  bencher.bench(|| black_box(&p).deep_clone());
}
