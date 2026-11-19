use galvanize_rs_main_models::{Animal, Dog, Puppy, Tagged};

fn main() {
  let puppy = Puppy::new("Bit");
  let animal: Animal = puppy.upcast();
  assert!(animal.ptr_eq(&puppy));
  assert_eq!(animal.describe(), "Bit says Woof (but squeakier)");
  assert_eq!(Tagged::new(7_u64).get(), 7);
  println!("{}", animal.describe());
  println!(
    "handle bytes: Animal={}, Dog={}, Puppy={}, Tagged<u64>={}",
    size_of::<Animal>(),
    size_of::<Dog>(),
    size_of::<Puppy>(),
    size_of::<Tagged<u64>>()
  );
}
