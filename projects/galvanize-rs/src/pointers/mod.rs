#[cfg(test)]
#[macro_use]
mod test_support;

mod mutable_pointer;
pub use mutable_pointer::GPointer;

mod arc_mutex_pointer;
pub use arc_mutex_pointer::ArcMutexPointer;

mod arc_rw_lock_pointer;
pub use arc_rw_lock_pointer::ArcRwLockPointer;

mod arc_swap_pointer;
pub use arc_swap_pointer::ArcSwapPointer;

mod pointer_strategy;
pub use pointer_strategy::{
  ArcMutexStrategy, ArcRwLockStrategy, ArcSwapStrategy, DefaultPointerStrategy, PointerStrategy,
};
