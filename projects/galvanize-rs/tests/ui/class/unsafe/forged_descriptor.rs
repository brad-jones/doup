// A forged descriptor can't make a final class open.
use galvanize_rs::{class, class_methods};
use galvanize_rs_fixtures_unsafe::Puppy;

type PretendOpen = Puppy;

macro_rules! PretendOpen {
  ($callback:path, $payload:tt) => {
    $callback! { $payload [] true
      { galvanize_rs_fixtures_unsafe::Puppy;
        galvanize_rs_fixtures_unsafe::__PuppyFields;
        galvanize_rs_fixtures_unsafe::PuppyData;
        galvanize_rs_fixtures_unsafe::__PuppyVTable; [] [] [] [] [] }
    }
  };
}

#[class(extends = PretendOpen)]
pub struct Forged {}

#[class_methods]
impl Forged {}

fn main() {}
