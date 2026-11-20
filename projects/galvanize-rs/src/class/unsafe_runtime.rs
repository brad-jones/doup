//! The kernel behind [`UnsafeRuntimeStrategy`](crate::UnsafeRuntimeStrategy).
//!
//! Each object is one `Arc` allocation holding the most-derived class's storage. Generated storage and
//! vtable types are `repr(C)` and embed their actual parent's storage/vtable as their first field, so a
//! pointer to the whole is also a valid pointer to every ancestor's prefix. A [`Handle<C>`] retains the
//! owning `Arc`, a pointer to the storage and a pointer to the object's immutable vtable. Generated class
//! handles are `repr(transparent)` over a `Handle<Self>`, so a reference to a descendant's handle can be
//! reinterpreted as a reference to an ancestor's.
//!
//! # Safety argument
//!
//! - A typed, invariant [`Handle<C>`] is the only construction currency. The erased representation is
//!   private to this module, so not even a generated class's own module can pair a handle with the wrong
//!   storage or table, and variance can't change `C`.
//! - [`Handle::new`] obtains `C`'s table itself and never accepts a caller-supplied table or builder.
//!   Published tables are immutable and live for the rest of the process.
//! - Downcasts and checked [`view`]s create a typed view only after checking the dynamic object's actual
//!   ancestry, via [`Class::is_class`].
//! - Virtual-method slots are `unsafe fn` pointers: a slot from a subclass's table may narrow its receiver,
//!   so calling one detached from its object requires `unsafe`. Generated methods call slots taken from
//!   their own validated object's table, or for `super`, from the declaring class's cached parent table,
//!   always with the original receiver.
//! - Implementing [`Class`] is `unsafe`; only the class macro does so, following the contract below.
//!
//! The templates never wrap user method bodies in `unsafe`. Passing Miri (`task test:miri`) is evidence
//! for this argument, not a proof of it.
#![deny(unsafe_op_in_unsafe_fn)]

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, RwLock};

/// The layout contract implemented by generated class handles.
///
/// # Safety
///
/// `Self` must be `repr(transparent)` over `Handle<Self>` (other fields may only be alignment-one ZSTs).
/// `Fields` and `VTable` must be `repr(C)` and embed the actual parent's corresponding type as their first
/// field. `is_class` must report exactly `Self` and its actual ancestors, with invariant type arguments.
/// `build_vtable` must return only slots that accept `Self` and its descendants, and inherited slots must
/// preserve that. Tables hold immutable data and function pointers, never mutable dispatch state.
/// `handle` and `from_handle` must preserve the supplied representation.
///
/// Implementations must not rely on safe callers respecting any further preconditions of these methods.
#[diagnostic::on_unimplemented(message = "`{Self}` isn't a #[class] using UnsafeRuntimeStrategy")]
pub unsafe trait Class: super::__private::Class {
  /// The complete storage, including the actual parent's as a prefix.
  type Fields: Send + Sync + 'static;
  /// The immutable dispatch table, including the actual parent's as a prefix.
  type VTable: Clone + Send + Sync + 'static;

  /// Borrows this class's invariant carrier.
  fn handle(&self) -> &Handle<Self>;
  /// Wraps an already validated carrier of exactly this static class.
  fn from_handle(handle: Handle<Self>) -> Self;
  /// Tests nominal class identity, including actual ancestors.
  fn is_class(id: TypeId) -> bool;
  /// Builds this class's table without executing any user method bodies.
  fn build_vtable() -> Self::VTable;
}

/// Typed construction data; implementations can only produce the class's actual storage type.
pub trait ClassData: Class {
  /// The initial values, including the parent's.
  type Data;
  /// Builds initialized storage, without installing a dispatch table.
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

// SAFETY: only Handle::new creates these pointers. The fields live in the Send + Sync owner, and tables are
// immutable and retained for the rest of the process. No safe API replaces either pointer or the owner.
unsafe impl Send for RawHandle {}
// SAFETY: shared access only exposes shared references into Send + Sync storage and immutable tables.
unsafe impl Sync for RawHandle {}

/// An invariant, owning view of an object of class `C` or one of its descendants.
///
/// The class parameter can't be changed by safe reconstruction or variance coercions.
#[repr(transparent)]
pub struct Handle<C: Class> {
  raw: RawHandle,
  marker: PhantomData<fn(C) -> C>,
}

impl<C: Class> Clone for Handle<C> {
  #[inline]
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
      .field("identity", &self.identity())
      .finish()
  }
}

impl<C: Class> Handle<C> {
  /// Allocates initialized storage and pairs it with `C`'s table, which it obtains itself.
  ///
  /// # Panics
  ///
  /// If the internal table registry is poisoned.
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

  /// Borrows the storage of this static view.
  #[inline]
  pub fn fields(&self) -> &C::Fields {
    // SAFETY: typed construction and checked casts, plus Class's prefix contract, establish this view.
    unsafe { self.raw.fields.cast::<C::Fields>().as_ref() }
  }

  /// Borrows this static view's prefix of the object's immutable table.
  #[inline]
  pub fn table(&self) -> &'static C::VTable {
    // SAFETY: tables are retained for the rest of the process; Class guarantees the compatible prefix.
    unsafe { self.raw.vtable.cast::<C::VTable>().as_ref() }
  }

  /// Tests whether the object is a `T`, or a descendant of `T`.
  #[inline]
  pub fn is<T: ?Sized + 'static>(&self) -> bool {
    self.raw.owner.is_class(TypeId::of::<T>())
  }

  /// Returns an owning view of the object as a `T`, after checking it is one.
  #[inline]
  pub fn downcast<T: Class>(&self) -> Option<T> {
    self.is::<T>().then(|| {
      T::from_handle(Handle {
        raw: self.raw.clone(),
        marker: PhantomData,
      })
    })
  }

  /// The address of the object's allocation, the same for every view of it.
  #[inline]
  pub fn identity(&self) -> *const () {
    Arc::as_ptr(&self.raw.owner).cast()
  }
}

/// Borrows `this` as a `C` without cloning its owner, after checking the object is a `C`.
pub fn view<B: Class, C: Class>(this: &B) -> Option<&C> {
  this.handle().is::<C>().then(|| {
    // SAFETY: the dynamic check establishes the requested class and Class guarantees the layout.
    unsafe { view_unchecked(this) }
  })
}

/// Reinterprets a reference to a class handle as a reference to another class's handle.
///
/// # Safety
///
/// The object behind `this` must be a `C` or a descendant of `C`. Both types must uphold [`Class`]'s
/// contract.
#[inline]
pub unsafe fn view_unchecked<B: Class, C: Class>(this: &B) -> &C {
  // SAFETY: Class requires the same transparent representation; the caller establishes compatibility.
  unsafe { &*std::ptr::from_ref(this).cast::<C>() }
}

type Registry = RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>;
static TABLES: LazyLock<Registry> = LazyLock::new(Default::default);

/// Returns `C`'s immutable table, building and publishing it on first use.
///
/// There's deliberately no way to supply a key, table or builder. Tables (including parents') are built
/// outside the registry lock, and the loser of a race to publish one drops its copy rather than leaking it.
///
/// # Panics
///
/// If the registry is poisoned.
pub fn vtable<C: Class>() -> &'static C::VTable {
  let id = TypeId::of::<C>();
  if let Some(table) = TABLES.read().expect("class table registry poisoned").get(&id) {
    return table.downcast_ref().expect("class table registry type mismatch");
  }
  let built = Box::new(C::build_vtable());
  let table = *TABLES
    .write()
    .expect("class table registry poisoned")
    .entry(id)
    .or_insert_with(|| Box::leak(built));
  table.downcast_ref().expect("class table registry type mismatch")
}

/// Hand-written implementations of the layout contract, standing in for generated ones. The generated
/// classes are tested by the `class_unsafe*` integration tests.
#[cfg(test)]
mod tests {
  use super::*;
  use std::sync::RwLock;
  use std::sync::atomic::{AtomicUsize, Ordering};

  #[repr(C)]
  struct BaseFields {
    value: RwLock<u64>,
  }

  #[repr(C)]
  #[derive(Clone)]
  struct BaseTable {
    get: unsafe fn(&Base) -> u64,
  }

  #[repr(transparent)]
  #[derive(Clone)]
  struct Base(Handle<Base>);

  impl crate::__private::Class for Base {
    fn __identity(&self) -> *const () {
      self.0.identity()
    }
  }

  // SAFETY: transparent over Handle<Self>, no parent, slots accept any Base.
  unsafe impl Class for Base {
    type Fields = BaseFields;
    type VTable = BaseTable;
    fn handle(&self) -> &Handle<Self> {
      &self.0
    }
    fn from_handle(handle: Handle<Self>) -> Self {
      Self(handle)
    }
    fn is_class(id: TypeId) -> bool {
      id == TypeId::of::<Self>()
    }
    fn build_vtable() -> BaseTable {
      unsafe fn get(this: &Base) -> u64 {
        *this.0.fields().value.read().unwrap()
      }
      BaseTable { get }
    }
  }

  impl Base {
    fn get(&self) -> u64 {
      // SAFETY: the slot comes from this object's own table.
      unsafe { (self.0.table().get)(self) }
    }
  }

  struct Counted(Arc<AtomicUsize>);

  impl Drop for Counted {
    fn drop(&mut self) {
      self.0.fetch_add(1, Ordering::SeqCst);
    }
  }

  #[repr(C)]
  struct DerivedFields {
    base: BaseFields,
    extra: u64,
    _drops: Counted,
  }

  #[repr(C)]
  #[derive(Clone)]
  struct DerivedTable {
    base: BaseTable,
  }

  #[repr(transparent)]
  #[derive(Clone)]
  struct Derived(Handle<Derived>);

  impl crate::__private::Class for Derived {
    fn __identity(&self) -> *const () {
      self.0.identity()
    }
  }

  // SAFETY: transparent over Handle<Self>; storage and table embed Base's as their first repr(C) field; the
  // overridden slot only runs on Derived objects.
  unsafe impl Class for Derived {
    type Fields = DerivedFields;
    type VTable = DerivedTable;
    fn handle(&self) -> &Handle<Self> {
      &self.0
    }
    fn from_handle(handle: Handle<Self>) -> Self {
      Self(handle)
    }
    fn is_class(id: TypeId) -> bool {
      id == TypeId::of::<Self>() || Base::is_class(id)
    }
    fn build_vtable() -> DerivedTable {
      unsafe fn get(this: &Base) -> u64 {
        // SAFETY: only installed in Derived's table, so the object is a Derived.
        let this: &Derived = unsafe { view_unchecked(this) };
        *this.0.fields().base.value.read().unwrap() + this.0.fields().extra
      }
      let mut table = DerivedTable {
        base: vtable::<Base>().clone(),
      };
      table.base.get = get;
      table
    }
  }

  fn derived(drops: &Arc<AtomicUsize>) -> Derived {
    Derived(Handle::new(DerivedFields {
      base: BaseFields { value: RwLock::new(1) },
      extra: 10,
      _drops: Counted(drops.clone()),
    }))
  }

  #[test]
  fn views_dispatch_through_the_dynamic_table() {
    let drops = Arc::new(AtomicUsize::new(0));
    let object = derived(&drops);
    let base: &Base = view(&object).expect("a Derived is a Base");
    assert_eq!(base.get(), 11);
    *base.0.fields().value.write().unwrap() = 2;
    assert_eq!(base.get(), 12);

    let plain = Base(Handle::new(BaseFields { value: RwLock::new(5) }));
    assert_eq!(plain.get(), 5);
    assert!(view::<_, Derived>(&plain).is_none());
    assert!(plain.0.downcast::<Derived>().is_none());
    assert!(plain.0.identity() != base.0.identity());
  }

  #[test]
  fn owning_views_share_one_allocation_and_drop_it_once() {
    let drops = Arc::new(AtomicUsize::new(0));
    let object = derived(&drops);
    let base: Base = view::<_, Base>(&object).expect("a Derived is a Base").clone();
    assert_eq!(base.0.identity(), object.0.identity());
    drop(object);
    let recovered = base.0.downcast::<Derived>().expect("still a Derived");
    assert_eq!(recovered.0.fields().extra, 10);
    drop(base);
    assert_eq!(drops.load(Ordering::SeqCst), 0);
    drop(recovered);
    assert_eq!(drops.load(Ordering::SeqCst), 1);
  }

  #[test]
  fn handles_move_between_threads() {
    let drops = Arc::new(AtomicUsize::new(0));
    let object = derived(&drops);
    std::thread::scope(|scope| {
      for _ in 0..2 {
        let object = object.clone();
        scope.spawn(move || {
          let base: &Base = view(&object).expect("a Derived is a Base");
          assert!(base.get() >= 11);
        });
      }
    });
  }
}
