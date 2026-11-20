// A slot from a subclass's table may only be called on that subclass's objects, so calling one is unsafe.
use galvanize_rs::__private::unsafe_runtime::Class;
use galvanize_rs_fixtures_unsafe::{Animal, Dog};

fn main() {
  let animal = Animal::new("root");
  let dog = Dog::new("dog");
  (dog.handle().table().base.speak)(&animal);
}
