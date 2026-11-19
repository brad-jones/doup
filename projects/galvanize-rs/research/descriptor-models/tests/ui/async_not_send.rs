use galvanize_rs_descriptor_models::{class, class_methods};
use std::rc::Rc;

#[class(open)]
pub struct Bad {}

#[class_methods]
impl Bad {
  #[class_virtual]
  pub async fn get(&self) -> usize {
    let value = Rc::new(1);
    std::future::pending::<()>().await;
    *value
  }
}

fn main() {}
