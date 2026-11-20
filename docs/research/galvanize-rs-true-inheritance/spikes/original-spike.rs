//! Hand-expanded sketch of what "true" inheritance in galvanize-rs could generate, see
//! ../01-original-design.md.
//!
//! One object per instance, handles are views into it, virtual methods via nested vtables,
//! downcasting via a TypeId chain. It's standalone (no galvanize-rs dependency), run it with:
//!
//! ```sh
//! pixi run -- rustc --edition 2024 docs/research/galvanize-rs-true-inheritance/spikes/original-spike.rs -o /tmp/spike
//! /tmp/spike
//! ```
//!
//! It passed Miri (`task miri` as of commit 79bde36) under Stacked & Tree Borrows with strict provenance.
//! Any real implementation must keep its test suite under Miri too.
#![allow(dead_code)]
use std::any::{Any, TypeId};
use std::ops::{Deref, DerefMut};
use std::ptr::NonNull;
use std::sync::{Arc, LazyLock, RwLock};

// ---------- runtime (galvanize_rs) ----------
pub trait Storage: Any + Send + Sync {
  fn is_or_extends(id: TypeId) -> bool
  where
    Self: Sized;
  fn dyn_is_or_extends(&self, id: TypeId) -> bool;
}
/// The allocation: a header pointing at the concrete class's vtable, then the fields.
#[repr(C)]
struct Object<F, V: 'static> {
  vt: &'static V,
  fields: F,
}
/// What every handle is: the owner plus pointers to its fields and vtable, which by the
/// `#[repr(C)]` prefix rule are valid for every ancestor too.
#[derive(Clone)]
pub struct RawHandle {
  owner: Arc<dyn Storage>,
  fields: NonNull<()>,
  vt: NonNull<()>,
}
unsafe impl Send for RawHandle {}
unsafe impl Sync for RawHandle {}
impl<F: Storage, V: Send + Sync + 'static> Storage for Object<F, V> {
  fn is_or_extends(id: TypeId) -> bool {
    F::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    F::is_or_extends(id)
  }
}
impl RawHandle {
  fn new<F: Storage, V: Send + Sync + 'static>(fields: F, vt: &'static V) -> Self {
    let obj = Arc::new(Object { vt, fields });
    let fields = NonNull::from(&obj.fields).cast();
    let vt = NonNull::from(obj.vt).cast();
    Self { owner: obj, fields, vt }
  }
  /// SAFETY: `F` must be the fields type of this handle's class or an ancestor.
  unsafe fn fields<F>(&self) -> &F {
    unsafe { self.fields.cast::<F>().as_ref() }
  }
  unsafe fn vt<V>(&self) -> &V {
    unsafe { self.vt.cast::<V>().as_ref() }
  }
  fn addr(&self) -> *const () {
    Arc::as_ptr(&self.owner) as *const ()
  }
}
/// Implemented by every class handle.
pub unsafe trait Class: Clone + 'static {
  type Fields: Storage;
  fn raw(&self) -> &RawHandle;
  fn from_raw(raw: RawHandle) -> Self;
  fn is<C: Class>(&self) -> bool {
    self.raw().owner.dyn_is_or_extends(TypeId::of::<C::Fields>())
  }
  fn downcast<C: Class>(&self) -> Option<C> {
    self.is::<C>().then(|| C::from_raw(self.raw().clone()))
  }
}
// vtables for generic classes can't be statics, so they'd go in a TypeId-keyed registry.
static REGISTRY: LazyLock<RwLock<Vec<(TypeId, &'static (dyn Any + Send + Sync))>>> = LazyLock::new(Default::default);
fn vtable_for<K: 'static, V: Send + Sync + 'static>(build: impl FnOnce() -> V) -> &'static V {
  let id = TypeId::of::<K>();
  if let Some((_, v)) = REGISTRY.read().unwrap().iter().find(|(k, _)| *k == id) {
    return v.downcast_ref().unwrap();
  }
  let v: &'static V = Box::leak(Box::new(build()));
  REGISTRY.write().unwrap().push((id, v));
  v
}

// ---------- #[class(open)] struct Animal { name: String } ----------
#[repr(C)]
pub struct __AnimalFields {
  name: RwLock<String>,
}
impl Storage for __AnimalFields {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>()
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}
#[repr(C)]
#[derive(Clone)]
pub struct __AnimalVTable {
  pub speak: fn(&Animal) -> String,
  pub describe: fn(&Animal) -> String,
}
#[repr(transparent)]
#[derive(Clone)]
pub struct Animal(RawHandle);
unsafe impl Class for Animal {
  type Fields = __AnimalFields;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r)
  }
}
impl Animal {
  fn vtable() -> &'static __AnimalVTable {
    vtable_for::<Animal, _>(|| __AnimalVTable {
      speak: |this| this.__speak(),
      describe: |this| this.__describe(),
    })
  }
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(Self::init(name), Self::vtable()))
  }
  pub fn init(name: &str) -> __AnimalFields {
    __AnimalFields {
      name: RwLock::new(name.into()),
    }
  }
  pub fn name(&self) -> String {
    unsafe { self.0.fields::<__AnimalFields>() }
      .name
      .read()
      .unwrap()
      .clone()
  }
  fn vt(&self) -> &__AnimalVTable {
    unsafe { self.0.vt() }
  }
  // #[class_methods] impl Animal { #[virtual] fn speak(&self) -> String { "...".into() } ... }
  pub fn speak(&self) -> String {
    (self.vt().speak)(self)
  }
  fn __speak(&self) -> String {
    "...".into()
  }
  pub fn describe(&self) -> String {
    (self.vt().describe)(self)
  }
  fn __describe(&self) -> String {
    format!("{} says {}", self.name(), self.speak())
  } // late bound!
}
impl<C: Class> PartialEq<C> for Animal {
  fn eq(&self, o: &C) -> bool {
    self.0.addr() == o.raw().addr()
  }
}

// ---------- #[class(open, extends = Animal)] struct Dog { tricks: usize } ----------
#[repr(C)]
pub struct __DogFields {
  base: <Animal as Class>::Fields,
  tricks: RwLock<usize>,
}
impl Storage for __DogFields {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>() || <<Animal as Class>::Fields as Storage>::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}
// The macro doesn't know which vtable type Animal has, so it names it via Animal::vtable()'s type.
#[repr(C)]
#[derive(Clone)]
pub struct __DogVTable<B> {
  base: B,
  pub fetch: fn(&Dog) -> String,
}
impl<B> Deref for __DogVTable<B> {
  type Target = B;
  fn deref(&self) -> &B {
    &self.base
  }
}
impl<B> DerefMut for __DogVTable<B> {
  fn deref_mut(&mut self) -> &mut B {
    &mut self.base
  }
}
#[repr(transparent)]
#[derive(Clone)]
pub struct Dog(RawHandle);
unsafe impl Class for Dog {
  type Fields = __DogFields;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r)
  }
}
impl Deref for Dog {
  type Target = Animal;
  fn deref(&self) -> &Animal {
    unsafe { &*(self as *const Dog as *const Animal) }
  }
}
type DogVT = __DogVTable<__AnimalVTable>; // in the macro: __DogVTable<VTableOf<Animal>>
impl Dog {
  fn vtable() -> &'static DogVT {
    vtable_for::<Dog, _>(|| {
      let mut vt = __DogVTable {
        base: Animal::vtable().clone(),
        fetch: |this| this.__fetch(),
      };
      // #[override] fn speak: the macro doesn't know `speak` is declared by Animal, DerefMut finds it,
      // and the closure's parameter type is inferred from the field's fn pointer type.
      vt.speak = |this| this.downcast::<Dog>().unwrap().__override_speak();
      vt
    })
  }
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(Self::init(name), Self::vtable()))
  }
  pub fn init(name: &str) -> __DogFields {
    __DogFields {
      base: Animal::init(name),
      tricks: RwLock::new(0),
    }
  }
  fn vt(&self) -> &DogVT {
    unsafe { self.0.vt() }
  }
  fn __override_speak(&self) -> String {
    "Woof".into()
  }
  pub fn fetch(&self) -> String {
    (self.vt().fetch)(self)
  }
  fn __fetch(&self) -> String {
    format!("{} fetches", self.name())
  }
}

// ---------- #[class(extends = Dog)] struct Puppy {} ----------
#[repr(C)]
pub struct __PuppyFields {
  base: <Dog as Class>::Fields,
}
impl Storage for __PuppyFields {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>() || <<Dog as Class>::Fields as Storage>::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}
#[repr(transparent)]
#[derive(Clone)]
pub struct Puppy(RawHandle);
unsafe impl Class for Puppy {
  type Fields = __PuppyFields;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r)
  }
}
impl Deref for Puppy {
  type Target = Dog;
  fn deref(&self) -> &Dog {
    unsafe { &*(self as *const Puppy as *const Dog) }
  }
}
impl Puppy {
  fn vtable() -> &'static DogVT {
    vtable_for::<Puppy, _>(|| {
      let mut vt = Dog::vtable().clone();
      // `speak` is declared two levels up, `fetch` one level up. `DerefMut` finds both.
      vt.speak = |this| this.downcast::<Puppy>().unwrap().__override_speak();
      vt.fetch = |this| this.downcast::<Puppy>().unwrap().__override_fetch();
      vt
    })
  }
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(__PuppyFields { base: Dog::init(name) }, Self::vtable()))
  }
  // `super.speak()`: the parent's vtable entry, called with the same object. `&Puppy` coerces to
  // the `&Animal` the slot takes, so the macro needn't know which ancestor declared it.
  fn __override_speak(&self) -> String {
    format!("{} (but squeakier)", (Dog::vtable().speak)(self))
  }
  fn __override_fetch(&self) -> String {
    format!("{} clumsily", (Dog::vtable().fetch)(self))
  }
}

fn main() {
  let p = Puppy::new("Bit");
  let as_animal: Animal = {
    let a: &Animal = &p;
    a.clone()
  };
  println!("{}", as_animal.describe()); // dispatches to Puppy's speak
  println!("{}", as_animal.speak());
  println!("{}", p.fetch());
  assert!(as_animal == p); // one identity across the hierarchy
  assert!(as_animal.is::<Dog>() && as_animal.is::<Puppy>());
  let d = Animal::new("Generic");
  assert!(d.downcast::<Dog>().is_none());
  let back: Dog = as_animal.downcast::<Dog>().unwrap();
  println!("{}", back.speak()); // still Puppy's
  let rex = Dog::new("Rex");
  println!("{} / {}", rex.describe(), rex.fetch());
  std::thread::spawn(move || println!("thread: {}", back.describe()))
    .join()
    .unwrap();
}
