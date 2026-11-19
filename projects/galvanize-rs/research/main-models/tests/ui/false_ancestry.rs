use galvanize_rs_main_models::{Animal, class, class_methods};

type PretendDog = Animal;
macro_rules! PretendDog {
  ($callback:path, $payload:tt) => {
    $callback! { $payload [] true
      { galvanize_rs_main_models::Dog;
        galvanize_rs_main_models::__DogFields;
        galvanize_rs_main_models::DogData;
        galvanize_rs_main_models::__DogVTable; }
    }
  };
}
#[class(extends = PretendDog)]
pub struct Forged {}
#[class_methods]
impl Forged {}
fn main() {}
