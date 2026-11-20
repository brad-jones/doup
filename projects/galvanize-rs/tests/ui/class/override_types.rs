// Overrides are type checked against the declaring class's signature, by both runtime strategies.
use galvanize_rs::{class, class_methods};

#[class(extends = galvanize_rs_fixtures_safe::Dog)]
pub struct SafeWrong {}

#[class_methods]
impl SafeWrong {
  #[class_override]
  pub fn speak(&self) -> u64 {
    1
  }
}

#[class(extends = galvanize_rs_fixtures_unsafe::Dog)]
pub struct UnsafeWrong {}

#[class_methods]
impl UnsafeWrong {
  #[class_override]
  pub fn speak(&self) -> u64 {
    1
  }
}

fn main() {}
