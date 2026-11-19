//! Bench-owned fork of `docs/research/galvanize-rs-true-inheritance-safe-spike.rs`, the design in
//! `galvanize-rs-true-inheritance-safe.md`.
//!
//! It follows the design document where the spike took shortcuts:
//!
//! - Field accessors are direct paths (`&self.base.base`) generated from the ancestor list (section 3), instead
//!   of forwarding through `__parent()`.
//! - Override bodies run on a handle of the overriding class (section 2.2). The trait method only has `this:
//!   &Animal`, and without `unsafe` the only way to get a `&Dog` from it is to build a `Dog`, i.e. a checked
//!   downcast. The spike's override bodies never use `self`, so it skipped that.
//!
//! It changes the design in one place: `downcast` checks the `TypeId` chain before cloning the `Arc`, and
//! `__cast` writes the handle into a caller-provided `Option<C>` instead of returning a `Box<dyn Any>`. That
//! makes a failed downcast free of refcount traffic and a successful one free of allocations.
//!
//! It adds, for measurement:
//!
//! - allocation-free members: the `age` field and the `legs` (never overridden), `weight` (overridden) and
//!   `bark_volume` (declared by `Dog`) virtual methods;
//! - `Beagle`, a final subclass of `Dog` that overrides nothing;
//! - `Boxed<T>` and its final subclass `Tagged<T>`, a generic hierarchy.
#![forbid(unsafe_code)]
use std::any::{Any, TypeId};
use std::ops::Deref;
use std::sync::{Arc, RwLock};

// ---------- #[class(open)] struct Animal { name: Str, age: u64 } ----------

pub struct __AnimalFields {
  name: RwLock<String>,
  age: RwLock<u64>,
}

pub trait AnimalObject: Any + Send + Sync {
  fn __parent(&self) -> Option<&dyn AnimalObject>;
  fn __animal_fields(&self) -> &__AnimalFields;
  fn __is(&self, target: TypeId) -> bool;
  /// Writes a handle into `out` if it's an `&mut Option<C>` for this class or an ancestor.
  fn __cast(self: Arc<Self>, out: &mut dyn Any);
  fn speak(&self, this: &Animal) -> String {
    match self.__parent() {
      Some(p) => p.speak(this),
      None => this.__speak(),
    }
  }
  fn legs(&self, this: &Animal) -> u64 {
    match self.__parent() {
      Some(p) => p.legs(this),
      None => this.__legs(),
    }
  }
  fn weight(&self, this: &Animal) -> u64 {
    match self.__parent() {
      Some(p) => p.weight(this),
      None => this.__weight(),
    }
  }
}

impl AnimalObject for __AnimalFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    None
  }
  fn __animal_fields(&self) -> &__AnimalFields {
    self
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Animal>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Animal>>() {
      *slot = Some(Animal(self));
    }
  }
}

#[derive(Clone)]
pub struct Animal(Arc<dyn AnimalObject>);

impl Animal {
  pub fn new(name: &str) -> Self {
    Self(Arc::new(Self::init(name)))
  }
  pub fn init(name: &str) -> __AnimalFields {
    __AnimalFields {
      name: RwLock::new(name.into()),
      age: RwLock::new(3),
    }
  }
  pub fn name(&self) -> String {
    self.0.__animal_fields().name.read().unwrap().clone()
  }
  pub fn age(&self) -> u64 {
    *self.0.__animal_fields().age.read().unwrap()
  }
  pub fn speak(&self) -> String {
    self.0.speak(self)
  }
  fn __speak(&self) -> String {
    "...".into()
  }
  pub fn legs(&self) -> u64 {
    self.0.legs(self)
  }
  fn __legs(&self) -> u64 {
    4
  }
  pub fn weight(&self) -> u64 {
    self.0.weight(self)
  }
  fn __weight(&self) -> u64 {
    10
  }
  /// Non-virtual, but late-binds to `speak`.
  pub fn describe(&self) -> String {
    format!("{} says {}", self.name(), self.speak())
  }
  pub fn is<C: 'static>(&self) -> bool {
    self.0.__is(TypeId::of::<C>())
  }
  pub fn downcast<C: 'static>(&self) -> Option<C> {
    if !self.is::<C>() {
      return None;
    }
    let mut slot = None::<C>;
    self.0.clone().__cast(&mut slot);
    slot
  }
  /// The handle an override body runs on: a checked downcast, see the module docs.
  fn __view<C: 'static>(&self) -> C {
    self.downcast().expect("an override only runs on objects of its class")
  }
}

// ---------- #[class(open, extends = Animal)] struct Dog { tricks: usize } ----------

pub struct __DogFields {
  base: __AnimalFields,
  #[allow(dead_code, reason = "payload, sizes the allocation like the spike's")]
  tricks: RwLock<usize>,
}

pub trait DogObject: AnimalObject {
  fn __dog_parent(&self) -> Option<&dyn DogObject>;
  fn __dog_fields(&self) -> &__DogFields;
  fn bark_volume(&self, this: &Dog) -> u64 {
    match self.__dog_parent() {
      Some(p) => p.bark_volume(this),
      None => this.__bark_volume(),
    }
  }
}

impl AnimalObject for __DogFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __animal_fields(&self) -> &__AnimalFields {
    &self.base
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Dog>() || t == TypeId::of::<Animal>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Dog>>() {
      *slot = Some(Dog::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Animal>>() {
      *slot = Some(Animal(self));
    }
  }
  // #[override(Animal)]
  fn speak(&self, this: &Animal) -> String {
    this.__view::<Dog>().__override_speak()
  }
  // #[override(Animal)]
  fn weight(&self, this: &Animal) -> u64 {
    this.__view::<Dog>().__override_weight()
  }
}

impl DogObject for __DogFields {
  fn __dog_parent(&self) -> Option<&dyn DogObject> {
    None
  }
  fn __dog_fields(&self) -> &__DogFields {
    self
  }
}

/// One view per ancestor level, so `Deref` can hand out `&Animal`.
#[derive(Clone)]
pub struct Dog {
  obj: Arc<dyn DogObject>,
  as_animal: Animal,
}

impl Deref for Dog {
  type Target = Animal;
  fn deref(&self) -> &Animal {
    &self.as_animal
  }
}

impl Dog {
  fn from_obj(obj: Arc<dyn DogObject>) -> Self {
    Self {
      as_animal: Animal(obj.clone()),
      obj,
    }
  }
  pub fn new(name: &str) -> Self {
    Self::from_obj(Arc::new(Self::init(name)))
  }
  pub fn init(name: &str) -> __DogFields {
    __DogFields {
      base: Animal::init(name),
      tricks: RwLock::new(0),
    }
  }
  fn __override_speak(&self) -> String {
    "Woof".into()
  }
  fn __override_weight(&self) -> u64 {
    30
  }
  pub fn bark_volume(&self) -> u64 {
    self.obj.bark_volume(self)
  }
  fn __bark_volume(&self) -> u64 {
    8
  }
}

// ---------- #[class(extends = Dog, Animal)] struct Puppy {} ----------

pub struct __PuppyFields {
  base: __DogFields,
}

impl AnimalObject for __PuppyFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __animal_fields(&self) -> &__AnimalFields {
    &self.base.base
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Puppy>() || t == TypeId::of::<Dog>() || t == TypeId::of::<Animal>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Puppy>>() {
      *slot = Some(Puppy::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Dog>>() {
      *slot = Some(Dog::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Animal>>() {
      *slot = Some(Animal(self));
    }
  }
  // #[override(Animal)]
  fn speak(&self, this: &Animal) -> String {
    this.__view::<Puppy>().__override_speak()
  }
  // #[override(Animal)]
  fn weight(&self, this: &Animal) -> u64 {
    this.__view::<Puppy>().__override_weight()
  }
}

impl DogObject for __PuppyFields {
  fn __dog_parent(&self) -> Option<&dyn DogObject> {
    Some(&self.base)
  }
  fn __dog_fields(&self) -> &__DogFields {
    &self.base
  }
  // #[override(Dog)]
  fn bark_volume(&self, this: &Dog) -> u64 {
    this.__view::<Puppy>().__override_bark_volume()
  }
}

/// A final class can hold its concrete storage.
#[derive(Clone)]
pub struct Puppy {
  obj: Arc<__PuppyFields>,
  as_dog: Dog,
}

impl Deref for Puppy {
  type Target = Dog;
  fn deref(&self) -> &Dog {
    &self.as_dog
  }
}

impl Puppy {
  fn from_obj(obj: Arc<__PuppyFields>) -> Self {
    Self {
      as_dog: Dog::from_obj(obj.clone()),
      obj,
    }
  }
  pub fn new(name: &str) -> Self {
    Self::from_obj(Arc::new(__PuppyFields { base: Dog::init(name) }))
  }
  // `super` calls: the parent storage's implementation, with the same object.
  fn super_speak(&self) -> String {
    AnimalObject::speak(&self.obj.base, self)
  }
  fn super_weight(&self) -> u64 {
    AnimalObject::weight(&self.obj.base, self)
  }
  fn super_bark_volume(&self) -> u64 {
    DogObject::bark_volume(&self.obj.base, self)
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

// ---------- #[class(extends = Dog, Animal)] struct Beagle {}, overriding nothing ----------

pub struct __BeagleFields {
  base: __DogFields,
}

impl AnimalObject for __BeagleFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __animal_fields(&self) -> &__AnimalFields {
    &self.base.base
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Beagle>() || t == TypeId::of::<Dog>() || t == TypeId::of::<Animal>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Beagle>>() {
      *slot = Some(Beagle::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Dog>>() {
      *slot = Some(Dog::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Animal>>() {
      *slot = Some(Animal(self));
    }
  }
}

impl DogObject for __BeagleFields {
  fn __dog_parent(&self) -> Option<&dyn DogObject> {
    Some(&self.base)
  }
  fn __dog_fields(&self) -> &__DogFields {
    &self.base
  }
}

#[derive(Clone)]
pub struct Beagle {
  #[allow(dead_code, reason = "generated for every final class, only `super` calls read it")]
  obj: Arc<__BeagleFields>,
  as_dog: Dog,
}

impl Deref for Beagle {
  type Target = Dog;
  fn deref(&self) -> &Dog {
    &self.as_dog
  }
}

impl Beagle {
  fn from_obj(obj: Arc<__BeagleFields>) -> Self {
    Self {
      as_dog: Dog::from_obj(obj.clone()),
      obj,
    }
  }
  pub fn new(name: &str) -> Self {
    Self::from_obj(Arc::new(__BeagleFields { base: Dog::init(name) }))
  }
}

// ---------- #[class(open)] struct Boxed<T> { value: T } ----------

pub trait Value: Clone + Send + Sync + 'static {}
impl<T: Clone + Send + Sync + 'static> Value for T {}

pub struct __BoxedFields<T> {
  value: RwLock<T>,
}

pub trait BoxedObject<T: Value>: Any + Send + Sync {
  fn __parent(&self) -> Option<&dyn BoxedObject<T>>;
  fn __boxed_fields(&self) -> &__BoxedFields<T>;
  fn __is(&self, target: TypeId) -> bool;
  fn __cast(self: Arc<Self>, out: &mut dyn Any);
  fn get(&self, this: &Boxed<T>) -> T {
    match self.__parent() {
      Some(p) => p.get(this),
      None => this.__get(),
    }
  }
}

impl<T: Value> BoxedObject<T> for __BoxedFields<T> {
  fn __parent(&self) -> Option<&dyn BoxedObject<T>> {
    None
  }
  fn __boxed_fields(&self) -> &__BoxedFields<T> {
    self
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Boxed<T>>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Boxed<T>>>() {
      *slot = Some(Boxed(self));
    }
  }
}

#[derive(Clone)]
pub struct Boxed<T: Value>(Arc<dyn BoxedObject<T>>);

impl<T: Value> Boxed<T> {
  pub fn new(value: T) -> Self {
    Self(Arc::new(Self::init(value)))
  }
  pub fn init(value: T) -> __BoxedFields<T> {
    __BoxedFields {
      value: RwLock::new(value),
    }
  }
  pub fn get(&self) -> T {
    self.0.get(self)
  }
  fn __get(&self) -> T {
    self.0.__boxed_fields().value.read().unwrap().clone()
  }
  pub fn downcast<C: 'static>(&self) -> Option<C> {
    if !self.0.__is(TypeId::of::<C>()) {
      return None;
    }
    let mut slot = None::<C>;
    self.0.clone().__cast(&mut slot);
    slot
  }
  fn __view<C: 'static>(&self) -> C {
    self.downcast().expect("an override only runs on objects of its class")
  }
}

// ---------- #[class(extends = Boxed<T>)] struct Tagged<T> { tag: u64 } ----------

pub struct __TaggedFields<T> {
  base: __BoxedFields<T>,
  #[allow(dead_code, reason = "payload, sizes the allocation like a real subclass's")]
  tag: RwLock<u64>,
}

impl<T: Value> BoxedObject<T> for __TaggedFields<T> {
  fn __parent(&self) -> Option<&dyn BoxedObject<T>> {
    Some(&self.base)
  }
  fn __boxed_fields(&self) -> &__BoxedFields<T> {
    &self.base
  }
  fn __is(&self, t: TypeId) -> bool {
    t == TypeId::of::<Tagged<T>>() || t == TypeId::of::<Boxed<T>>()
  }
  fn __cast(self: Arc<Self>, out: &mut dyn Any) {
    if let Some(slot) = out.downcast_mut::<Option<Tagged<T>>>() {
      *slot = Some(Tagged::from_obj(self));
    } else if let Some(slot) = out.downcast_mut::<Option<Boxed<T>>>() {
      *slot = Some(Boxed(self));
    }
  }
  // #[override(Boxed)]
  fn get(&self, this: &Boxed<T>) -> T {
    this.__view::<Tagged<T>>().__override_get()
  }
}

#[derive(Clone)]
pub struct Tagged<T: Value> {
  obj: Arc<__TaggedFields<T>>,
  as_boxed: Boxed<T>,
}

impl<T: Value> Deref for Tagged<T> {
  type Target = Boxed<T>;
  fn deref(&self) -> &Boxed<T> {
    &self.as_boxed
  }
}

impl<T: Value> Tagged<T> {
  fn from_obj(obj: Arc<__TaggedFields<T>>) -> Self {
    Self {
      as_boxed: Boxed(obj.clone()),
      obj,
    }
  }
  pub fn new(value: T) -> Self {
    Self::from_obj(Arc::new(__TaggedFields {
      base: Boxed::init(value),
      tag: RwLock::new(0),
    }))
  }
  fn super_get(&self) -> T {
    BoxedObject::get(&self.obj.base, self)
  }
  fn __override_get(&self) -> T {
    self.super_get()
  }
}
