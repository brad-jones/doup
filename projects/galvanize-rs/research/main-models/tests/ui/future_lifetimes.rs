use galvanize_rs_main_models::AsyncAnimal;

fn main() {
  let animal = AsyncAnimal::from_data(AsyncAnimal::init("root"));
  let future;
  {
    let prefix = String::from("temporary");
    future = animal.speak(&prefix);
  }
  drop(future);
  let future = animal.speak("");
  drop(animal);
  drop(future);
}
