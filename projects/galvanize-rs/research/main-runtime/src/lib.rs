//! Research-only prefix-layout inheritance kernel.
//!
//! A typed, invariant [`Handle`] is the only safe construction currency. The erased representation
//! stays private here, including against code in a generated class's declaration module.
#![deny(unsafe_op_in_unsafe_fn)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, RwLock};

/// Contract implemented by generated transparent class handles.
///
/// # Safety
///
/// `Self` must be transparent over `Handle<Self>` (additional fields may only be alignment-one ZSTs).
/// `Fields` and `VTable` must embed each actual ancestor's corresponding type as their first
/// `repr(C)` field. `is_class` must report exactly Self and those ancestors, with invariant type
/// arguments. `build_vtable` must return only slots accepting Self and its descendants; inherited
/// slots must preserve that contract. Tables contain immutable data and function pointers, never
/// user-mutable dispatch state. `handle` and `from_handle` must preserve the supplied representation.
///
/// Implementors must not rely on safe callers respecting any additional preconditions on these
/// methods, or on descriptor tokens being truthful.
pub unsafe trait Class: Clone + Send + Sync + 'static {
  /// Complete initialized storage, including the actual parent prefix.
  type Fields: Send + Sync + 'static;
  /// Immutable dispatch table, including the actual parent prefix.
  type VTable: Clone + Send + Sync + 'static;

  /// Borrows this class's invariant carrier.
  fn handle(&self) -> &Handle<Self>;
  /// Wraps an already validated carrier of exactly this static class.
  fn from_handle(handle: Handle<Self>) -> Self;
  /// Tests nominal class identity, including actual ancestors.
  fn is_class(id: TypeId) -> bool;
  /// Builds this class's table without executing user method bodies.
  fn build_vtable() -> Self::VTable;
}

/// An extensible class, checked independently of descriptor metadata.
pub trait OpenClass: Class {}

/// Typed construction data; implementations can only return the class's actual field type.
pub trait ClassData: Class {
  /// Input consumed when building initialized storage.
  type Data;
  /// Builds initialized fields, without installing a dispatch table.
  fn build(data: Self::Data) -> Self::Fields;
}

trait Storage: Send + Sync {
  fn is_class(&self, id: TypeId) -> bool;
}

struct Allocation<C: Class> {
  fields: C::Fields,
}

impl<C: Class> Storage for Allocation<C> {
  fn is_class(&self, id: TypeId) -> bool {
    C::is_class(id)
  }
}

#[derive(Clone)]
struct RawHandle {
  owner: Arc<dyn Storage>,
  fields: NonNull<()>,
  vtable: NonNull<()>,
}

// SAFETY: only Handle::new creates these pointers. Fields are in the live Send + Sync owner;
// tables are immutable and permanently retained. No safe API changes either pointer or owner.
unsafe impl Send for RawHandle {}
// SAFETY: shared access exposes only shared references into Send + Sync storage and immutable tables.
unsafe impl Sync for RawHandle {}

/// Invariant owning view of a class or one of its descendants.
///
/// The class parameter cannot be changed through safe reconstruction or variance coercions.
#[repr(transparent)]
pub struct Handle<C: Class> {
  raw: RawHandle,
  marker: PhantomData<fn(C) -> C>,
}

impl<C: Class> Clone for Handle<C> {
  fn clone(&self) -> Self {
    Self {
      raw: self.raw.clone(),
      marker: PhantomData,
    }
  }
}

impl<C: Class> std::fmt::Debug for Handle<C> {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    f.debug_struct("Handle")
      .field("class", &std::any::type_name::<C>())
      .field("identity", &Arc::as_ptr(&self.raw.owner))
      .finish()
  }
}

impl<C: Class> Handle<C> {
  /// Allocates initialized fields and obtains the matching class table internally.
  ///
  /// Panics if the internal table registry is poisoned.
  pub fn new(fields: C::Fields) -> Self {
    let table = vtable::<C>();
    let owner = Arc::new(Allocation::<C> { fields });
    let fields = NonNull::from(&owner.fields).cast();
    Self {
      raw: RawHandle {
        owner,
        fields,
        vtable: NonNull::from(table).cast(),
      },
      marker: PhantomData,
    }
  }

  /// Borrows initialized fields for this static view.
  #[inline]
  pub fn fields(&self) -> &C::Fields {
    // SAFETY: typed construction/casting and Class's prefix contract establish this view's fields.
    unsafe { self.raw.fields.cast::<C::Fields>().as_ref() }
  }

  /// Borrows this static view's prefix of the object's immutable dynamic table.
  #[inline]
  pub fn table(&self) -> &'static C::VTable {
    // SAFETY: every table is permanently retained; Class guarantees the compatible prefix.
    unsafe { self.raw.vtable.cast::<C::VTable>().as_ref() }
  }

  /// Tests whether the dynamic object supports the requested class.
  #[inline]
  pub fn is<T: Class>(&self) -> bool {
    self.raw.owner.is_class(TypeId::of::<T>())
  }

  /// Returns an owning typed view only after checking dynamic compatibility.
  #[inline]
  pub fn downcast<T: Class>(&self) -> Option<T> {
    self.is::<T>().then(|| {
      T::from_handle(Handle {
        raw: self.raw.clone(),
        marker: PhantomData,
      })
    })
  }

  /// Compares the concrete allocation, irrespective of the views' static class.
  pub fn ptr_eq<T: Class>(&self, other: &Handle<T>) -> bool {
    Arc::ptr_eq(&self.raw.owner, &other.raw.owner)
  }
}

/// Borrows a compatible typed view without changing ownership.
pub fn view<B: Class, C: Class>(this: &B) -> Option<&C> {
  this.handle().is::<C>().then(|| {
    // SAFETY: the dynamic check establishes the requested class and Class guarantees layout.
    unsafe { view_unchecked(this) }
  })
}

/// Reinterprets a transparent handle reference without cloning its owner.
///
/// # Safety
///
/// The dynamic object behind `this` must be C or a descendant of C. Both implementations must obey
/// Class's representation contract. The returned reference cannot outlive `this`.
#[inline]
pub unsafe fn view_unchecked<B: Class, C: Class>(this: &B) -> &C {
  // SAFETY: Class requires the same transparent representation; the caller establishes compatibility.
  unsafe { &*std::ptr::from_ref(this).cast::<C>() }
}

type Registry = RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>;
static TABLES: LazyLock<Registry> = LazyLock::new(Default::default);

/// Returns the immutable table belonging to C.
///
/// There is deliberately no safe caller-supplied key, table, or builder. Parent construction occurs
/// outside the registry lock; racing losers are dropped instead of leaked.
pub fn vtable<C: Class>() -> &'static C::VTable {
  let id = TypeId::of::<C>();
  if let Some(table) = TABLES.read().expect("inheritance table registry poisoned").get(&id) {
    return table.downcast_ref().expect("class table registry type mismatch");
  }
  let built = Box::new(C::build_vtable());
  let table = *TABLES
    .write()
    .expect("inheritance table registry poisoned")
    .entry(id)
    .or_insert_with(|| Box::leak(built));
  table.downcast_ref().expect("class table registry type mismatch")
}
