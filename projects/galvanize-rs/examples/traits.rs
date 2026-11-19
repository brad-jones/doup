//! Hanging traits off classes, to get the dynamic dispatch (overriding) that
//! `extends` alone doesn't give you.

use galvanize_rs::{List, Str, class, sf};
use std::fmt::Debug;
use std::sync::Arc;

// ---------- Creature ----------

/// The behaviour every animal shares, like an abstract base class's methods.
///
/// Methods without a body are "abstract", methods with a default body are
/// "virtual" with a base implementation. The supertraits let a `dyn Creature`
/// be printed, and stored in a class that's shared across threads.
pub trait Creature: Debug + Send + Sync {
  /// Gives the default methods access to the shared `Animal` state.
  fn animal(&self) -> &Animal;

  /// Abstract: every creature must say what sound it makes.
  fn sound(&self) -> Str;

  /// Virtual: subclasses may override it.
  fn describe(&self) -> Str {
    sf!("{} has {} legs", self.animal().name(), self.animal().legs())
  }

  /// A template method: it isn't meant to be overridden, but it calls the
  /// overridable methods, so it does the right thing for every creature.
  fn introduce(&self) -> Str {
    sf!("{} says {}. {}.", self.animal().name(), self.sound(), self.describe())
  }
}

// ---------- Animal ----------

#[class]
pub struct Animal {
  pub name: Str,
  pub legs: i64,
}

impl Animal {
  pub fn new(name: impl Into<Str>, legs: i64) -> Self {
    Self::from_data(AnimalData {
      name: name.into(),
      legs,
    })
  }
}

impl Creature for Animal {
  fn animal(&self) -> &Animal {
    self
  }

  fn sound(&self) -> Str {
    "nothing".into()
  }
}

// ---------- Dog ----------

#[class(extends = Animal)]
pub struct Dog {
  pub tricks: List<Str>,
}

impl Dog {
  pub fn new(name: impl Into<Str>) -> Self {
    Self::from_data(DogData {
      base: Animal::new(name, 4),
      tricks: List::new(),
    })
  }
}

impl Creature for Dog {
  fn animal(&self) -> &Animal {
    // `&Dog` coerces to `&Animal` via `Deref`.
    self
  }

  fn sound(&self) -> Str {
    "Woof".into()
  }

  /// Overrides `describe`, calling the base class's version like `super.describe()`.
  fn describe(&self) -> Str {
    sf!(
      "{} and knows {} tricks",
      Creature::describe(self.animal()),
      self.tricks().len()
    )
  }
}

// ---------- Puppy ----------

#[class(extends = Dog)]
pub struct Puppy {
  pub age_weeks: i64,
}

impl Puppy {
  pub fn new(name: impl Into<Str>, age_weeks: i64) -> Self {
    Self::from_data(PuppyData {
      base: Dog::new(name),
      age_weeks,
    })
  }
}

/// Trait impls aren't inherited, so `Puppy` needs its own. Methods it doesn't
/// write get the trait's defaults, not `Dog`'s overrides, so forward to `Dog`
/// explicitly where that's wanted.
impl Creature for Puppy {
  fn animal(&self) -> &Animal {
    // Coerces through two levels: `&Puppy` -> `&Dog` -> `&Animal`.
    self
  }

  fn sound(&self) -> Str {
    "Yip".into()
  }

  fn describe(&self) -> Str {
    sf!(
      "{}, and is {} weeks old",
      Creature::describe(&self.base()),
      self.age_weeks()
    )
  }
}

// ---------- Zoo ----------

/// Holds creatures of any class, as trait objects.
///
/// `Arc<dyn Creature>` is `Clone` and `Debug`, so it can live in a `List`.
#[class]
pub struct Zoo {
  pub name: Str,
  creatures: List<Arc<dyn Creature>>,
}

impl Zoo {
  pub fn new(name: impl Into<Str>) -> Self {
    Self::from_data(ZooData {
      name: name.into(),
      creatures: List::new(),
    })
  }

  /// Admits a handle to the creature, so the zoo shares it with the caller.
  pub fn admit(&self, creature: impl Creature + 'static) {
    self.creatures().push(Arc::new(creature) as Arc<dyn Creature>);
  }

  /// Dynamic dispatch: each creature's own `introduce` runs.
  pub fn introductions(&self) -> Vec<Str> {
    self.creatures().iter().map(|c| c.introduce()).collect()
  }
}

/// Static dispatch works too, for any class that implements the trait.
fn shout<C: Creature>(creature: &C) -> Str {
  sf!("{}!", creature.sound().to_uppercase())
}

fn main() {
  let rex = Dog::new("Rex");
  rex.tricks().push("sit");
  let bit = Puppy::new("Bit", 9);
  let zoo = Zoo::new("City Zoo");

  zoo.admit(Animal::new("Generic", 2));
  zoo.admit(rex.clone());
  zoo.admit(bit.clone());

  // The zoo holds handles to the same objects, so changes show up there too.
  rex.tricks().push("roll over");
  for line in zoo.introductions() {
    println!("{line}");
  }

  println!("{} {}", shout(&rex), shout(&bit));

  // The handle's type decides which impl runs. As an `Animal` it's `Animal`'s
  // impl, even though the object is a `Dog`. As a `dyn Creature` it's `Dog`'s.
  let as_animal: Animal = rex.upcast();
  let as_creature: &dyn Creature = &rex;
  println!("as Animal:   {}", as_animal.introduce());
  println!("as Creature: {}", as_creature.introduce());

  println!("{zoo:?}");
}
