use galvanize_rs_descriptor_models::{Dog, class, class_methods};

#[class(extends = Dog)]
pub struct Bad {}

#[class_methods]
impl Bad {
  #[class_override]
  pub fn speek(&self) -> String {
    "typo".into()
  }
}

fn main() {}
