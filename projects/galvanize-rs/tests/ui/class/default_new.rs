use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_safe::ConfigFile;

#[derive(Clone, Debug)]
pub struct NotDefault;

// Every field, including inherited ones, must be `Default` for `new()`, which is checked where it's called.
#[class]
pub struct Own {
  pub value: NotDefault,
}

#[class_methods]
impl Own {}

#[class(open)]
pub struct Parent {
  pub value: NotDefault,
}

#[class_methods]
impl Parent {}

#[class(extends = Parent)]
pub struct Inherits {}

#[class_methods]
impl Inherits {}

// Declaring a constructor of any name stops `new()` being generated.
#[class]
pub struct Created {}

#[class_methods]
impl Created {
  pub fn create() -> Self {
    Self::from_data(CreatedData {})
  }
}

#[class]
pub struct Loaded {}

#[class_methods]
impl Loaded {
  pub fn load() -> Result<Loaded, String> {
    Ok(Self::from_data(LoadedData {}))
  }
}

fn main() {
  let _ = Own::new();
  let _ = Inherits::default();
  // `new()` and `Default` are generated together, so these show neither is. (Calling `new` would make
  // rustc suggest `PartialEq::ne`, whose path in the error depends on the toolchain.)
  let _ = Created::default();
  let _ = Loaded::default();
  // Abstract classes can't be instantiated, so they don't get them either.
  let _ = ConfigFile::default();
}
