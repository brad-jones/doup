//! Hand-expanded sketch of the fully safe variant of "true" inheritance in galvanize-rs, see
//! galvanize-rs-true-inheritance-safe.md next to this file (and read galvanize-rs-true-inheritance.md
//! first).
//!
//! One trait per class (`AnimalObject`, `DogObject: AnimalObject`) implemented by the most-derived
//! storage, `Arc<dyn …>` handles that carry one view per ancestor level, and casts generated from
//! the full ancestor list (`#[class(extends = Dog, Animal)]`). `#![forbid(unsafe_code)]` proves it
//! needs no `unsafe`. It's standalone (no galvanize-rs dependency), run it with:
//!
//! ```sh
//! pixi run -- rustc --edition 2024 docs/research/galvanize-rs-true-inheritance-safe-spike.rs -o /tmp/spike
//! /tmp/spike
//! ```
#![forbid(unsafe_code)]
#![allow(dead_code)]
use std::any::{Any, TypeId};
use std::ops::Deref;
use std::sync::{Arc, RwLock};

// ---- Animal (open) ----
pub struct __AnimalFields {
  name: RwLock<String>,
}
pub trait AnimalObject: Any + Send + Sync {
  fn __parent(&self) -> Option<&dyn AnimalObject>;
  fn __animal_fields(&self) -> &__AnimalFields {
    self.__parent().unwrap().__animal_fields()
  }
  fn __cast(self: Arc<Self>, target: TypeId) -> Option<Box<dyn Any>>;
  fn speak(&self, this: &Animal) -> String {
    match self.__parent() {
      Some(p) => p.speak(this),
      None => this.__speak(),
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
  fn __cast(self: Arc<Self>, t: TypeId) -> Option<Box<dyn Any>> {
    (t == TypeId::of::<Animal>()).then(|| Box::new(Animal(self)) as Box<dyn Any>)
  }
}
#[derive(Clone)]
pub struct Animal(Arc<dyn AnimalObject>);
impl Animal {
  pub fn new(n: &str) -> Self {
    Self(Arc::new(Self::init(n)))
  }
  pub fn init(n: &str) -> __AnimalFields {
    __AnimalFields {
      name: RwLock::new(n.into()),
    }
  }
  pub fn name(&self) -> String {
    self.0.__animal_fields().name.read().unwrap().clone()
  }
  pub fn speak(&self) -> String {
    self.0.speak(self)
  }
  fn __speak(&self) -> String {
    "...".into()
  }
  pub fn describe(&self) -> String {
    format!("{} says {}", self.name(), self.speak())
  }
  /// Identity: every view of an object shares the same allocation.
  pub fn ptr_eq(&self, other: &Animal) -> bool {
    std::ptr::addr_eq(Arc::as_ptr(&self.0), Arc::as_ptr(&other.0))
  }
  pub fn downcast<C: 'static>(&self) -> Option<C> {
    self.0.clone().__cast(TypeId::of::<C>())?.downcast().ok().map(|b| *b)
  }
}

// ---- #[class(open, extends = Animal)] Dog ----
pub struct __DogFields {
  base: __AnimalFields,
  tricks: RwLock<usize>,
}
pub trait DogObject: AnimalObject {
  fn __dog_parent(&self) -> Option<&dyn DogObject>;
  fn __dog_fields(&self) -> &__DogFields {
    self.__dog_parent().unwrap().__dog_fields()
  }
  fn fetch(&self, this: &Dog) -> String {
    match self.__dog_parent() {
      Some(p) => p.fetch(this),
      None => this.__fetch(),
    }
  }
}
impl AnimalObject for __DogFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __cast(self: Arc<Self>, t: TypeId) -> Option<Box<dyn Any>> {
    if t == TypeId::of::<Dog>() {
      return Some(Box::new(Dog::from_obj(self)));
    }
    if t == TypeId::of::<Animal>() {
      return Some(Box::new(Animal(self)));
    }
    None
  }
  fn speak(&self, _this: &Animal) -> String {
    "Woof".into()
  } // #[override]
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
  } // trait upcasting
  pub fn new(n: &str) -> Self {
    Self::from_obj(Arc::new(Self::init(n)))
  }
  pub fn init(n: &str) -> __DogFields {
    __DogFields {
      base: Animal::init(n),
      tricks: RwLock::new(0),
    }
  }
  pub fn fetch(&self) -> String {
    self.obj.fetch(self)
  }
  fn __fetch(&self) -> String {
    format!("{} fetches", self.name())
  }
}

// ---- #[class(extends = Dog, Animal)] Puppy: knows the chain, and which trait declares each override ----
pub struct __PuppyFields {
  base: __DogFields,
}
impl AnimalObject for __PuppyFields {
  fn __parent(&self) -> Option<&dyn AnimalObject> {
    Some(&self.base)
  }
  fn __cast(self: Arc<Self>, t: TypeId) -> Option<Box<dyn Any>> {
    if t == TypeId::of::<Puppy>() {
      return Some(Box::new(Puppy::from_obj(self)));
    }
    if t == TypeId::of::<Dog>() {
      return Some(Box::new(Dog::from_obj(self)));
    }
    if t == TypeId::of::<Animal>() {
      return Some(Box::new(Animal(self)));
    }
    None
  }
  // #[override(Animal)] with a super call: the parent's impl, same `this`.
  fn speak(&self, this: &Animal) -> String {
    format!("{} (but squeakier)", AnimalObject::speak(&self.base, this))
  }
}
impl DogObject for __PuppyFields {
  fn __dog_parent(&self) -> Option<&dyn DogObject> {
    Some(&self.base)
  }
  // #[override(Dog)]
  fn fetch(&self, this: &Dog) -> String {
    format!("{} clumsily", DogObject::fetch(&self.base, this))
  }
}
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
  pub fn new(n: &str) -> Self {
    Self::from_obj(Arc::new(__PuppyFields { base: Dog::init(n) }))
  }
}

fn main() {
  let p = Puppy::new("Bit");
  let a: &Animal = &p;
  println!("{}", a.describe());
  println!("{}", p.fetch());
  let back: Dog = a.downcast::<Dog>().unwrap();
  println!("{} {}", back.speak(), a.downcast::<Puppy>().is_some());
  assert!(Animal::new("x").downcast::<Dog>().is_none());
  assert!(back.ptr_eq(a) && p.ptr_eq(&back));
  println!("{}", Dog::new("Rex").describe()); // Dog's own override, no Puppy involved
  std::thread::spawn(move || println!("thread: {}", back.describe()))
    .join()
    .unwrap();
}
