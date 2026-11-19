use galvanize_rs_main_models::{Animal, Dog};
use galvanize_rs_main_runtime::Class;

fn main() {
  let animal = Animal::new("root");
  let dog = Dog::new("dog");
  (dog.handle().table().base.speak)(&animal);
}
