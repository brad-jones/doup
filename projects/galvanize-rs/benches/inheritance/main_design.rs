//! Bench-owned fork of `docs/research/galvanize-rs-true-inheritance-spike.rs`, the design in
//! `galvanize-rs-true-inheritance.md`.
//!
//! Historical, known-unsound prototype: safe raw reconstruction permits UB and generic handles
//! violate invariance. Retained unchanged as a comparison/negative control, not a usable API.
//! See the defensive review and `galvanize-rs-true-inheritance-hardened.md` for the separate repair.
//!
//! It follows the design document where the spike took shortcuts:
//!
//! - Override thunks view `this` as the overriding class without checking (section 4.4), instead of a
//!   checked `downcast`.
//! - Non-generic classes keep their vtable in a `LazyLock` static (section 4.8). Only generic classes use the
//!   registry, which is a `HashMap` rather than a linear scan.
//! - `describe` is a non-virtual method that late-binds to `speak` (section 4.4), as in the safe variant.
//!
//! It adds, for measurement:
//!
//! - allocation-free members: the `age` field and the `legs` (never overridden), `weight` (overridden) and
//!   `bark_volume` (declared by `Dog`) virtual methods;
//! - `Beagle`, a final subclass of `Dog` that overrides nothing;
//! - `Boxed<T>` and its final subclass `Tagged<T>`, a generic hierarchy whose vtables live in the registry.
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
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

impl<F: Storage, V: Send + Sync + 'static> Storage for Object<F, V> {
  fn is_or_extends(id: TypeId) -> bool {
    F::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    F::is_or_extends(id)
  }
}

/// What every handle is: the owner plus pointers to its fields and vtable, which by the `#[repr(C)]` prefix
/// rule are valid for every ancestor too.
#[derive(Clone)]
pub struct RawHandle {
  owner: Arc<dyn Storage>,
  fields: NonNull<()>,
  vt: NonNull<()>,
}

// SAFETY: `owner` is `Send + Sync` (`Storage: Send + Sync`), `fields` points into it, and `vt` points at a
// `'static` vtable of `fn` pointers.
unsafe impl Send for RawHandle {}
unsafe impl Sync for RawHandle {}

impl RawHandle {
  fn new<F: Storage, V: Send + Sync + 'static>(fields: F, vt: &'static V) -> Self {
    let obj = Arc::new(Object { vt, fields });
    let fields = NonNull::from(&obj.fields).cast();
    let vt = NonNull::from(obj.vt).cast();
    Self { owner: obj, fields, vt }
  }

  /// # Safety
  ///
  /// `F` must be the fields type of this handle's class or an ancestor.
  unsafe fn fields<F>(&self) -> &F {
    unsafe { self.fields.cast::<F>().as_ref() }
  }

  /// # Safety
  ///
  /// `V` must be the vtable type of this handle's class or an ancestor.
  unsafe fn vt<V>(&self) -> &V {
    unsafe { self.vt.cast::<V>().as_ref() }
  }
}

/// Implemented by every class handle.
///
/// # Safety
///
/// Only the macro implements it. Implementors must be `#[repr(transparent)]` over a [`RawHandle`] whose
/// fields and vtable belong to `Self` or a subclass.
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

/// Views a handle as a descendant class's handle, without checking.
///
/// # Safety
///
/// `this` must refer to an object of class `C` or one of its subclasses.
unsafe fn view_unchecked<B: Class, C: Class>(this: &B) -> &C {
  // SAFETY: both are `#[repr(transparent)]` over `RawHandle` (`Class`'s contract), and the caller
  // guarantees the object is a `C`.
  unsafe { &*(this as *const B as *const C) }
}

type Registry = RwLock<HashMap<TypeId, &'static (dyn Any + Send + Sync)>>;

/// Vtables of generic classes can't be statics, so they live in a `TypeId`-keyed registry.
static REGISTRY: LazyLock<Registry> = LazyLock::new(Default::default);

fn vtable_for<K: 'static, V: Send + Sync + 'static>(build: impl FnOnce() -> V) -> &'static V {
  let id = TypeId::of::<K>();
  if let Some(v) = REGISTRY.read().unwrap().get(&id) {
    return v.downcast_ref().unwrap();
  }
  // Built outside the lock because building a subclass's vtable looks up its parent's. Racing threads may
  // each build one, the first registered wins and is leaked, the others are dropped.
  let built = Box::new(build());
  let v = *REGISTRY.write().unwrap().entry(id).or_insert_with(|| Box::leak(built));
  v.downcast_ref().unwrap()
}

// ---------- #[class(open)] struct Animal { name: Str, age: u64 } ----------

#[repr(C)]
pub struct __AnimalFields {
  name: RwLock<String>,
  age: RwLock<u64>,
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
  pub legs: fn(&Animal) -> u64,
  pub weight: fn(&Animal) -> u64,
}

static ANIMAL_VT: LazyLock<__AnimalVTable> = LazyLock::new(|| __AnimalVTable {
  speak: |this| this.__speak(),
  legs: |this| this.__legs(),
  weight: |this| this.__weight(),
});

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
    &ANIMAL_VT
  }
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(Self::init(name), Self::vtable()))
  }
  pub fn init(name: &str) -> __AnimalFields {
    __AnimalFields {
      name: RwLock::new(name.into()),
      age: RwLock::new(3),
    }
  }
  fn fields(&self) -> &__AnimalFields {
    // SAFETY: every view of an object carries its own class's fields, which start with an Animal's.
    unsafe { self.0.fields() }
  }
  fn vt(&self) -> &__AnimalVTable {
    // SAFETY: as above, for vtables.
    unsafe { self.0.vt() }
  }
  pub fn name(&self) -> String {
    self.fields().name.read().unwrap().clone()
  }
  pub fn age(&self) -> u64 {
    *self.fields().age.read().unwrap()
  }
  pub fn speak(&self) -> String {
    (self.vt().speak)(self)
  }
  fn __speak(&self) -> String {
    "...".into()
  }
  pub fn legs(&self) -> u64 {
    (self.vt().legs)(self)
  }
  fn __legs(&self) -> u64 {
    4
  }
  pub fn weight(&self) -> u64 {
    (self.vt().weight)(self)
  }
  fn __weight(&self) -> u64 {
    10
  }
  /// Non-virtual, but late-binds to `speak`.
  pub fn describe(&self) -> String {
    format!("{} says {}", self.name(), self.speak())
  }
}

// ---------- #[class(open, extends = Animal)] struct Dog { tricks: usize } ----------

#[repr(C)]
pub struct __DogFields {
  base: <Animal as Class>::Fields,
  #[allow(dead_code, reason = "payload, sizes the allocation like the spike's")]
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

#[repr(C)]
#[derive(Clone)]
pub struct __DogVTable<B> {
  base: B,
  pub bark_volume: fn(&Dog) -> u64,
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

type DogVT = __DogVTable<__AnimalVTable>;

static DOG_VT: LazyLock<DogVT> = LazyLock::new(|| {
  let mut vt = __DogVTable {
    base: Animal::vtable().clone(),
    bark_volume: |this| this.__bark_volume(),
  };
  // SAFETY (all thunks): only Dog objects, and those of its subclasses, carry this vtable or a copy of it.
  vt.speak = |this| unsafe { view_unchecked::<_, Dog>(this) }.__override_speak();
  vt.weight = |this| unsafe { view_unchecked::<_, Dog>(this) }.__override_weight();
  vt
});

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
    // SAFETY: a Dog is an Animal, and both are `#[repr(transparent)]` over `RawHandle`.
    unsafe { &*(self as *const Dog as *const Animal) }
  }
}

impl Dog {
  fn vtable() -> &'static DogVT {
    &DOG_VT
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
    // SAFETY: every view of an object carries its own class's vtable, which starts with a Dog's.
    unsafe { self.0.vt() }
  }
  fn __override_speak(&self) -> String {
    "Woof".into()
  }
  fn __override_weight(&self) -> u64 {
    30
  }
  pub fn bark_volume(&self) -> u64 {
    (self.vt().bark_volume)(self)
  }
  fn __bark_volume(&self) -> u64 {
    8
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

static PUPPY_VT: LazyLock<DogVT> = LazyLock::new(|| {
  let mut vt = Dog::vtable().clone();
  // SAFETY (all thunks): only Puppy objects carry this vtable.
  vt.speak = |this| unsafe { view_unchecked::<_, Puppy>(this) }.__override_speak();
  vt.weight = |this| unsafe { view_unchecked::<_, Puppy>(this) }.__override_weight();
  vt.bark_volume = |this| unsafe { view_unchecked::<_, Puppy>(this) }.__override_bark_volume();
  vt
});

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
    // SAFETY: a Puppy is a Dog, and both are `#[repr(transparent)]` over `RawHandle`.
    unsafe { &*(self as *const Puppy as *const Dog) }
  }
}

impl Puppy {
  fn vtable() -> &'static DogVT {
    &PUPPY_VT
  }
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(__PuppyFields { base: Dog::init(name) }, Self::vtable()))
  }
  // `super` calls: the parent's vtable entry, called with the same object.
  fn super_speak(&self) -> String {
    (Dog::vtable().speak)(self)
  }
  fn super_weight(&self) -> u64 {
    (Dog::vtable().weight)(self)
  }
  fn super_bark_volume(&self) -> u64 {
    (Dog::vtable().bark_volume)(self)
  }
  fn __override_speak(&self) -> String {
    format!("{} (but squeakier)", self.super_speak())
  }
  fn __override_weight(&self) -> u64 {
    self.super_weight() / 4
  }
  fn __override_bark_volume(&self) -> u64 {
    self.super_bark_volume() / 2
  }
}

// ---------- #[class(extends = Dog)] struct Beagle {}, overriding nothing ----------

#[repr(C)]
pub struct __BeagleFields {
  base: <Dog as Class>::Fields,
}

impl Storage for __BeagleFields {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>() || <<Dog as Class>::Fields as Storage>::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}

static BEAGLE_VT: LazyLock<DogVT> = LazyLock::new(|| Dog::vtable().clone());

#[repr(transparent)]
#[derive(Clone)]
pub struct Beagle(RawHandle);

unsafe impl Class for Beagle {
  type Fields = __BeagleFields;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r)
  }
}

impl Deref for Beagle {
  type Target = Dog;
  fn deref(&self) -> &Dog {
    // SAFETY: a Beagle is a Dog, and both are `#[repr(transparent)]` over `RawHandle`.
    unsafe { &*(self as *const Beagle as *const Dog) }
  }
}

impl Beagle {
  pub fn new(name: &str) -> Self {
    Self(RawHandle::new(__BeagleFields { base: Dog::init(name) }, &*BEAGLE_VT))
  }
}

// ---------- #[class(open)] struct Boxed<T> { value: T } ----------

pub trait Value: Clone + Send + Sync + 'static {}
impl<T: Clone + Send + Sync + 'static> Value for T {}

#[repr(C)]
pub struct __BoxedFields<T> {
  value: RwLock<T>,
}

impl<T: Value> Storage for __BoxedFields<T> {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>()
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}

#[repr(C)]
#[derive(Clone)]
pub struct __BoxedVTable<T> {
  pub get: fn(&Boxed<T>) -> T,
}

#[repr(transparent)]
#[derive(Clone)]
pub struct Boxed<T>(RawHandle, PhantomData<fn() -> T>);

unsafe impl<T: Value> Class for Boxed<T> {
  type Fields = __BoxedFields<T>;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r, PhantomData)
  }
}

impl<T: Value> Boxed<T> {
  fn vtable() -> &'static __BoxedVTable<T> {
    vtable_for::<Self, _>(|| __BoxedVTable {
      get: |this| this.__get(),
    })
  }
  pub fn new(value: T) -> Self {
    Self::from_raw(RawHandle::new(Self::init(value), Self::vtable()))
  }
  pub fn init(value: T) -> __BoxedFields<T> {
    __BoxedFields {
      value: RwLock::new(value),
    }
  }
  fn vt(&self) -> &__BoxedVTable<T> {
    // SAFETY: every view of an object carries its own class's vtable, which starts with a Boxed's.
    unsafe { self.0.vt() }
  }
  pub fn get(&self) -> T {
    (self.vt().get)(self)
  }
  fn __get(&self) -> T {
    // SAFETY: every view of an object carries its own class's fields, which start with a Boxed's.
    unsafe { self.0.fields::<__BoxedFields<T>>() }
      .value
      .read()
      .unwrap()
      .clone()
  }
}

// ---------- #[class(extends = Boxed<T>)] struct Tagged<T> { tag: u64 } ----------

#[repr(C)]
pub struct __TaggedFields<T: Value> {
  base: <Boxed<T> as Class>::Fields,
  #[allow(dead_code, reason = "payload, sizes the allocation like a real subclass's")]
  tag: RwLock<u64>,
}

impl<T: Value> Storage for __TaggedFields<T> {
  fn is_or_extends(id: TypeId) -> bool {
    id == TypeId::of::<Self>() || <<Boxed<T> as Class>::Fields as Storage>::is_or_extends(id)
  }
  fn dyn_is_or_extends(&self, id: TypeId) -> bool {
    Self::is_or_extends(id)
  }
}

#[repr(transparent)]
#[derive(Clone)]
pub struct Tagged<T>(RawHandle, PhantomData<fn() -> T>);

unsafe impl<T: Value> Class for Tagged<T> {
  type Fields = __TaggedFields<T>;
  fn raw(&self) -> &RawHandle {
    &self.0
  }
  fn from_raw(r: RawHandle) -> Self {
    Self(r, PhantomData)
  }
}

impl<T: Value> Deref for Tagged<T> {
  type Target = Boxed<T>;
  fn deref(&self) -> &Boxed<T> {
    // SAFETY: a Tagged is a Boxed, and both are `#[repr(transparent)]` over `RawHandle`.
    unsafe { &*(self as *const Tagged<T> as *const Boxed<T>) }
  }
}

impl<T: Value> Tagged<T> {
  fn vtable() -> &'static __BoxedVTable<T> {
    vtable_for::<Self, _>(|| {
      let mut vt = Boxed::<T>::vtable().clone();
      // SAFETY: only Tagged objects carry this vtable.
      vt.get = |this| unsafe { view_unchecked::<_, Tagged<T>>(this) }.__override_get();
      vt
    })
  }
  pub fn new(value: T) -> Self {
    let fields = __TaggedFields {
      base: Boxed::init(value),
      tag: RwLock::new(0),
    };
    Self::from_raw(RawHandle::new(fields, Self::vtable()))
  }
  /// A generic parent's vtable comes from the registry, so `super` calls pay for a lookup.
  fn super_get(&self) -> T {
    (Boxed::<T>::vtable().get)(self)
  }
  fn __override_get(&self) -> T {
    self.super_get()
  }
}
