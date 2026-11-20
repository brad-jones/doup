use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_safe::{Polygon, Rectangle};

// Concrete classes must implement every abstract method they inherit.
#[class(extends = Polygon)]
pub struct ImplementsNothing {}

#[class_methods]
impl ImplementsNothing {}

#[class(extends = Rectangle)]
pub struct ImplementsTooLittle {}

#[class_methods]
impl ImplementsTooLittle {}

// Overrides repeat the visibility of the method they override.
#[class(abstract, extends = Polygon)]
pub struct WrongVisibility {}

#[class_methods]
impl WrongVisibility {
  pub fn area(&self) -> f64 {
    1.0
  }

  fn scaled(&self, factor: f64) -> f64 {
    factor
  }
}

// An abstract method has no implementation for `super` to call.
#[class(extends = Polygon)]
pub struct SuperAbstract {}

#[class_methods]
impl SuperAbstract {
  fn area(&self) -> f64 {
    self.super_area()
  }

  pub fn scaled(&self, factor: f64) -> f64 {
    factor
  }
}

// Abstract classes can't be instantiated.
#[class(abstract)]
pub struct Uninstantiable {}

#[class_methods]
impl Uninstantiable {
  pub fn new() -> Self {
    Self::from_data(UninstantiableData {})
  }
}

#[class(open)]
pub struct NotAbstract {}

#[class_methods]
impl NotAbstract {
  #[class_abstract]
  pub fn missing(&self);
}

#[class(abstract)]
pub struct Bodies {}

#[class_methods]
impl Bodies {
  #[class_abstract]
  pub fn with_body(&self) {}

  #[class_virtual]
  pub fn without_body(&self);

  pub fn ordinary_without_body(&self);
}

// Once an ancestor implements an abstract method, replacing that implementation must be marked.
#[class(extends = Rectangle)]
pub struct ReplacesImplementation {}

#[class_methods]
impl ReplacesImplementation {
  fn area(&self) -> f64 {
    1.0
  }

  pub fn scaled(&self, factor: f64) -> f64 {
    factor
  }
}

#[class(abstract, extends = Polygon)]
pub struct Redeclared {}

#[class_methods]
impl Redeclared {
  #[class_abstract]
  fn area(&self) -> f64;
}

fn main() {}
