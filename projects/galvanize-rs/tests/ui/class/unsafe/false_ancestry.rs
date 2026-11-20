// A forged descriptor can't make a subclass of `Animal` claim to be a `Dog`: layouts and casts follow the
// declared parent, and the claimed ancestry has to type check against it.
use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_unsafe::Animal;

type PretendDog = Animal;

macro_rules! PretendDog {
  ($callback:path, $payload:tt) => {
    $callback! { $payload [] true
      { galvanize_rs_fixtures_unsafe::Dog;
        galvanize_rs_fixtures_unsafe::__DogFields;
        galvanize_rs_fixtures_unsafe::DogData;
        galvanize_rs_fixtures_unsafe::__DogVTable; [] [] }
    }
  };
}

#[class(extends = PretendDog)]
pub struct Forged {}

#[class_methods]
impl Forged {}

fn main() {}
