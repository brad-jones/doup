use galvanize_rs_descriptor_models::{Dog, class, class_methods};

#[class(extends = Dog)]
pub struct Bad {}

#[class_methods]
impl Bad {
  #[class_override]
  pub fn speak(&self) -> u64 {
    1
  }
}

fn main() {}
