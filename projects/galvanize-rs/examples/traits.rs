//! Multiple inheritance, Rust style: a class extends one parent, and implements any number of traits.
//!
//! Classes give single inheritance of state and behaviour, with virtual methods that subclasses override.
//! Traits are interfaces, and with default methods, mixins. Combined, a `Duck` can be an `Animal` and a
//! `Bird`, and also a `Flyer` and a `Swimmer`, much like a Java or C# class extending a base class and
//! implementing several interfaces.
//!
//! Run with `cargo run -p galvanize-rs --example traits`.

use galvanize_rs::{Str, class, class_methods, sf};

// ---------- the class hierarchy ----------

#[class(open)]
pub struct Animal {
  pub name: Str,
}

#[class_methods]
impl Animal {
  pub fn init(name: &str) -> AnimalData {
    AnimalData { name: name.into() }
  }

  #[class_virtual]
  pub fn sound(&self) -> Str {
    "...".into()
  }

  pub fn speak(&self) -> Str {
    sf!("{} says {}", self.name(), self.sound())
  }
}

#[class(open, extends = Animal)]
pub struct Bird {
  pub span: f64,
}

#[class_methods]
impl Bird {
  pub fn init(name: &str, span: f64) -> BirdData {
    BirdData {
      base: Animal::init(name),
      span,
    }
  }

  pub fn new(name: &str, span: f64) -> Self {
    Self::from_data(Self::init(name, span))
  }

  #[class_override]
  pub fn sound(&self) -> Str {
    "tweet".into()
  }
}

#[class(extends = Bird)]
pub struct Duck {
  pub depth: f64,
}

#[class_methods]
impl Duck {
  pub fn new(name: &str) -> Self {
    Self::from_data(DuckData {
      base: Bird::init(name, 0.9),
      depth: 2.0,
    })
  }

  #[class_override]
  pub fn sound(&self) -> Str {
    "quack".into()
  }
}

#[class(extends = Animal)]
pub struct Fish {}

#[class_methods]
impl Fish {
  pub fn new(name: &str) -> Self {
    Self::from_data(FishData {
      base: Animal::init(name),
    })
  }

  #[class_override]
  pub fn sound(&self) -> Str {
    "blub".into()
  }
}

#[class(extends = Animal)]
pub struct Bat {}

#[class_methods]
impl Bat {
  pub fn new(name: &str) -> Self {
    Self::from_data(BatData {
      base: Animal::init(name),
    })
  }
}

// ---------- the interfaces ----------

/// An interface with a mixin: implementors supply `animal` and `wingspan`, and get `fly` for free.
///
/// `Send + Sync` lets a `dyn Flyer` be shared between threads, like the classes themselves.
pub trait Flyer: Send + Sync {
  /// Gives the default methods access to the shared `Animal` state and methods.
  fn animal(&self) -> &Animal;

  fn wingspan(&self) -> f64;

  /// A default method calling a virtual one: `speak` runs the object's own `sound` override.
  fn fly(&self) -> Str {
    sf!(
      "{} and flies with a {}m wingspan",
      self.animal().speak(),
      self.wingspan()
    )
  }
}

pub trait Swimmer: Send + Sync {
  fn animal(&self) -> &Animal;

  fn dive_depth(&self) -> f64;

  fn swim(&self) -> Str {
    sf!("{} dives to {}m", self.animal().name(), self.dive_depth())
  }
}

/// Interfaces can combine others. With a blanket impl, anything that both flies and swims is waterfowl,
/// without saying so.
pub trait Waterfowl: Flyer + Swimmer {
  fn migrate(&self) -> Str {
    sf!("{}, then {}", self.fly(), self.swim())
  }
}

impl<T: Flyer + Swimmer> Waterfowl for T {}

// ---------- implementing them ----------

impl Flyer for Bird {
  fn animal(&self) -> &Animal {
    // `&Bird` coerces to `&Animal`, via `Deref`.
    self
  }

  fn wingspan(&self) -> f64 {
    self.span()
  }
}

/// Trait impls aren't inherited: a `Duck` is a `Bird`, but isn't a `Flyer` until it says so. Forwarding
/// to `Bird`'s impl keeps a single implementation. Behaviour that subclasses should be able to change
/// belongs in a virtual method, like `sound`, which the default methods then call.
impl Flyer for Duck {
  fn animal(&self) -> &Animal {
    self
  }

  fn wingspan(&self) -> f64 {
    let bird: &Bird = self;
    bird.wingspan()
  }
}

impl Swimmer for Duck {
  fn animal(&self) -> &Animal {
    self
  }

  fn dive_depth(&self) -> f64 {
    self.depth()
  }
}

impl Swimmer for Fish {
  fn animal(&self) -> &Animal {
    self
  }

  fn dive_depth(&self) -> f64 {
    40.0
  }
}

impl Flyer for Bat {
  fn animal(&self) -> &Animal {
    self
  }

  fn wingspan(&self) -> f64 {
    0.3
  }
}

/// Static dispatch over several interfaces at once. Both traits have an `animal` method, so it's called
/// through one explicitly, like an explicit interface implementation in C#.
fn tour<T: Waterfowl>(traveller: &T) -> Str {
  sf!("{} migrates: {}", Flyer::animal(traveller).name(), traveller.migrate())
}

fn main() {
  let robin = Bird::new("Robin", 0.25);
  let duck = Duck::new("Donald");
  let nemo = Fish::new("Nemo");
  let bat = Bat::new("Bruce");

  // Each interface gives its own view of a mixed collection, with dynamic dispatch.
  let flyers: Vec<Box<dyn Flyer>> = vec![Box::new(robin.clone()), Box::new(duck.clone()), Box::new(bat.clone())];
  let swimmers: Vec<Box<dyn Swimmer>> = vec![Box::new(duck.clone()), Box::new(nemo.clone())];
  for flyer in &flyers {
    println!("{}", flyer.fly());
  }
  for swimmer in &swimmers {
    println!("{}", swimmer.swim());
  }
  println!("{}", tour(&duck));

  // And the class hierarchy gives another, with virtual methods. The boxes above hold handles to the same
  // objects, so renaming one shows up everywhere.
  duck.set_name("Daisy");
  let animals: Vec<Animal> = vec![robin.upcast(), duck.upcast(), nemo.upcast(), bat.upcast()];
  for animal in &animals {
    println!("{}", animal.speak());
  }
  println!("{}", flyers[1].fly());
}
