use galvanize_rs_descriptor_models::{class, class_methods};

#[class(open)]
pub struct Bad {}

#[class_methods]
impl Bad {
  #[class_virtual]
  pub fn get<T: Default>(&self) -> T {
    T::default()
  }
}

fn main() {}
