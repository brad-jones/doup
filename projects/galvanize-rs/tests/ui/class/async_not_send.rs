use galvanize_rs::{SafeRuntimeStrategy, UnsafeRuntimeStrategy, class, class_methods};
use std::rc::Rc;

#[class(open, runtime_strategy = SafeRuntimeStrategy)]
pub struct SafeBad {}

#[class_methods]
impl SafeBad {
  #[class_virtual]
  pub async fn get(&self) -> usize {
    let value = Rc::new(1);
    std::future::pending::<()>().await;
    *value
  }
}

#[class(open, runtime_strategy = UnsafeRuntimeStrategy)]
pub struct UnsafeBad {}

#[class_methods]
impl UnsafeBad {
  #[class_virtual]
  pub async fn get(&self) -> usize {
    let value = Rc::new(1);
    std::future::pending::<()>().await;
    *value
  }
}

fn main() {}
