use galvanize_rs_main_models::{Animal, Dog, class, class_methods};
use galvanize_rs_main_runtime::{Class, ClassData, Handle, vtable};

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
  let animal = Animal::new("not a dog");
  let _ = Dog::from_handle(animal.handle().clone());
  let _ = LocalDog {
    __handle: animal.handle().clone(),
    __invariant: Default::default(),
  };
  let _ = Dog::from_raw(animal.handle().clone());
  let dog = Dog::new("dog");
  let slot = dog.handle().table().base.speak;
  let _ = slot;
  vtable::<Animal>(|| dog.handle().table().clone());
  Handle::<Animal>::new(<Animal as ClassData>::build(Animal::init("root")), dog.handle().table());
}
