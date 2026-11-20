use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_safe::{Animal, AsyncAnimal};

#[class(extends = Animal)]
pub struct Overrides {}

#[class_methods]
impl Overrides {
  #[class_override]
  pub fn speek(&self) -> String {
    "typo".into()
  }
}

#[class(extends = Animal)]
pub struct Redeclared {}

#[class_methods]
impl Redeclared {
  #[class_virtual]
  pub fn speak(&self) -> String {
    "again".into()
  }
}

#[class(extends = Animal)]
pub struct AsyncOverride {}

#[class_methods]
impl AsyncOverride {
  #[class_override]
  pub async fn speak(&self) -> String {
    String::new()
  }
}

#[class(extends = AsyncAnimal)]
pub struct SyncOverride {}

#[class_methods]
impl SyncOverride {
  #[class_override]
  pub fn speak(&self, _prefix: &str) -> Result<String, &'static str> {
    Ok(String::new())
  }
}

#[class(open)]
pub struct Unsupported {}

#[class_methods]
impl Unsupported {
  #[class_virtual]
  pub fn generic<T: Default>(&self) -> T {
    T::default()
  }

  #[class_virtual]
  pub fn mutable(&mut self) {}

  #[class_virtual]
  pub fn copy(&self) -> Self {
    self.clone()
  }

  #[class_virtual]
  pub fn opaque(&self) -> impl Fn() {
    || ()
  }

  #[class_virtual]
  fn private(&self) {}

  #[class_virtual]
  #[class_override]
  pub fn both(&self) {}

  pub fn from_data(&self) {}
}

#[class(extends = Animal)]
pub struct SuperCollision {}

#[class_methods]
impl SuperCollision {
  #[class_override]
  pub fn speak(&self) -> String {
    "hi".into()
  }

  pub fn super_speak(&self) {}
}

#[class]
pub struct NotInherent {}

#[class_methods]
impl Clone for NotInherent {}

fn main() {}
