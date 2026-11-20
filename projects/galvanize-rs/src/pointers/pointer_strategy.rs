use super::{ArcMutexPointer, ArcRwLockPointer, ArcSwapPointer, MutablePointer};

/// Selects which [`MutablePointer`] a collection such as [`crate::List`] or
/// [`crate::Dict`], or the fields of a [`crate::class`], store their data in.
///
/// A [`MutablePointer`] is generic over the value it holds, so it can't be
/// passed around as a plain type parameter. A `PointerStrategy` is a zero sized marker
/// that maps to a whole family of pointers via the [`PointerStrategy::Ptr`] GAT.
pub trait PointerStrategy {
  /// The pointer type used to hold a value of type `T`.
  type Ptr<T: Clone>: MutablePointer<T>;

  /// The pointer type used to hold a class's field of type `T`. Classes are
  /// shared between threads, so this must be `Send + Sync`. It's usually the
  /// same type as [`PointerStrategy::Ptr`].
  type SyncPtr<T: Clone + Send + Sync>: MutablePointer<T> + Send + Sync;
}

/// Backs a collection with [`ArcRwLockPointer`]. This is the default.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArcRwLockStrategy;

impl PointerStrategy for ArcRwLockStrategy {
  type Ptr<T: Clone> = ArcRwLockPointer<T>;
  type SyncPtr<T: Clone + Send + Sync> = ArcRwLockPointer<T>;
}

/// Backs a collection with [`ArcMutexPointer`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ArcMutexStrategy;

impl PointerStrategy for ArcMutexStrategy {
  type Ptr<T: Clone> = ArcMutexPointer<T>;
  type SyncPtr<T: Clone + Send + Sync> = ArcMutexPointer<T>;
}

/// Backs a collection with [`ArcSwapPointer`].
///
/// Reads are lock-free but every write copies the whole collection, and the
/// write closure may be retried.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArcSwapStrategy;

impl PointerStrategy for ArcSwapStrategy {
  type Ptr<T: Clone> = ArcSwapPointer<T>;
  type SyncPtr<T: Clone + Send + Sync> = ArcSwapPointer<T>;
}

/// The [`PointerStrategy`] used when none is specified.
pub type DefaultPointerStrategy = ArcRwLockStrategy;
