use galvanize_rs_main_models::{Animal, AsyncAnimal, Puppy, class, class_methods};

#[class(extends = Animal)]
pub struct Unknown {}
#[class_methods]
impl Unknown {
  #[class_override]
  pub fn typo(&self) {}
}
#[class(extends = Animal)]
pub struct Wrong {}
#[class_methods]
impl Wrong {
  #[class_override]
  pub fn speak(&self) -> u64 {
    1
  }
}
#[class(extends = Animal)]
pub struct AsyncWrong {}
#[class_methods]
impl AsyncWrong {
  #[class_override]
  pub async fn speak(&self) -> String {
    String::new()
  }
}
#[class(extends = AsyncAnimal)]
pub struct SyncWrong {}
#[class_methods]
impl SyncWrong {
  #[class_override]
  pub fn speak(&self, _prefix: &str) -> Result<String, &'static str> {
    Ok(String::new())
  }
}
#[class(extends = Puppy)]
pub struct FinalChild {}
#[class_methods]
impl FinalChild {}
fn main() {}
