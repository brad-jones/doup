use galvanize_rs_main_models::{Animal, Dog, class, class_methods};
use galvanize_rs_main_runtime::view_unchecked;

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
