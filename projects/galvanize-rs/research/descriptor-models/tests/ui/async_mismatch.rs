use galvanize_rs_descriptor_models::{Animal, AsyncAnimal, class, class_methods};

#[class(extends = AsyncAnimal)]
pub struct SyncOverride {}

#[class_methods]
impl SyncOverride {
  #[class_override]
  pub fn speak(&self, _prefix: &str) -> Result<String, &'static str> {
    Ok(String::new())
  }
}

#[class(extends = Animal)]
pub struct AsyncOverride {}

#[class_methods]
impl AsyncOverride {
  #[class_override]
  pub async fn speak(&self) -> String {
    String::new()
  }
}

fn main() {}
