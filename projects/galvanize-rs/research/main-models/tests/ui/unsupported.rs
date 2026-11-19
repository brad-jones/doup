use galvanize_rs_main_models::{class, class_methods};

#[class(open)]
pub struct Generic {}
#[class_methods]
impl Generic {
  #[class_virtual]
  pub fn get<T: Default>(&self) -> T {
    T::default()
  }
}
#[class(open)]
pub struct Mutable {}
#[class_methods]
impl Mutable {
  #[class_virtual]
  pub fn set(&mut self) {}
}
#[class(open)]
pub struct SelfResult {}
#[class_methods]
impl SelfResult {
  #[class_virtual]
  pub fn copy(&self) -> Self {
    self.clone()
  }
}
#[class(open)]
pub struct Collision {}
#[class_methods]
impl Collision {
  pub fn from_data(&self) {}
}
fn main() {}
