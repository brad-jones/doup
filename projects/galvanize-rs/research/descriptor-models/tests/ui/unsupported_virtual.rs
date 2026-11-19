use galvanize_rs_descriptor_models::{class, class_methods};

#[class(open)]
pub struct Bad {}

#[class_methods]
impl Bad {
  #[class_virtual]
  pub async fn get(&self) -> u64 {
    1
  }
}

fn main() {}
