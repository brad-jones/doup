use galvanize_rs_main_models::{Boxed, Tagged};

fn main() {
  let boxed = Boxed::<fn(&str)>::new(|_| {});
  let _widened: Boxed<fn(&'static str)> = boxed;
  let tagged = Tagged::<fn(&str)>::new(|_| {});
  let _widened: Tagged<fn(&'static str)> = tagged;
}
