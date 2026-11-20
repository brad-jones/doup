// Unchecked thunks and views are unsafe to call, even from the class's own module.
use galvanize_rs::__private::unsafe_runtime::view_unchecked;
use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_unsafe::{Animal, Dog};

#[class(extends = Animal)]
pub struct LocalDog {}

#[class_methods]
impl LocalDog {
  #[class_override]
  pub fn speak(&self) -> String {
    "local".into()
  }
}

fn main() {
  let animal = Animal::new("root");
  LocalDog::__thunk_speak(&animal);
  let _: &Dog = view_unchecked(&animal);
}
