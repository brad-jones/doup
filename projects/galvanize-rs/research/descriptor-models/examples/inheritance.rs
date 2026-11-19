//! Runs the generated hierarchy and reports handle sizes; no handwritten runtime expansion.
#![forbid(unsafe_code)]

use galvanize_rs_descriptor_models::{Animal, Dog, Puppy, Tagged};

fn main() {
  let puppy = Puppy::new("Bit");
  let animal: Animal = puppy.upcast();
  assert!(animal.ptr_eq(&puppy));
  println!("{}", animal.describe());
  println!("Dog: {}, Puppy: {}", animal.is::<Dog>(), animal.is::<Puppy>());
  println!("generic super: {}", Tagged::new(7_u64).get());
  println!(
    "handle words: Animal={}, Dog={}, Puppy={}",
    size_of::<Animal>() / size_of::<usize>(),
    size_of::<Dog>() / size_of::<usize>(),
    size_of::<Puppy>() / size_of::<usize>(),
  );
}
