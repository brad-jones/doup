use galvanize_rs_main_models::{Puppy, class, class_methods};

type PretendOpen = Puppy;
macro_rules! PretendOpen {
  ($callback:path, $payload:tt) => {
    $callback! { $payload [] true
      { galvanize_rs_main_models::Puppy;
        galvanize_rs_main_models::__PuppyFields;
        galvanize_rs_main_models::PuppyData;
        galvanize_rs_main_models::__PuppyVTable; }
    }
  };
}
#[class(extends = PretendOpen)]
pub struct Forged {}
#[class_methods]
impl Forged {}
fn main() {}
