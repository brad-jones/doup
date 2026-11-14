//! Async Box<dyn Trait> Example
//!
//! Powered by: <https://crates.io/crates/async-trait>

#![allow(dead_code)]
use async_trait::async_trait;

#[async_trait]
trait AsyncSpeak {
  async fn say(&self);
}

struct AsyncDog;

#[async_trait]
impl AsyncSpeak for AsyncDog {
  async fn say(&self) {
    println!("Woof");
  }
}

impl From<AsyncDog> for Box<dyn AsyncSpeak> {
  fn from(speaker: AsyncDog) -> Self {
    Box::new(speaker)
  }
}

struct AsyncCat;

#[async_trait]
impl AsyncSpeak for AsyncCat {
  async fn say(&self) {
    println!("Meow");
  }
}

impl From<AsyncCat> for Box<dyn AsyncSpeak> {
  fn from(speaker: AsyncCat) -> Self {
    Box::new(speaker)
  }
}

async fn example() {
  let which_speaker_to_use = "dog";

  let speaker: Box<dyn AsyncSpeak> = match which_speaker_to_use {
    "dog" => AsyncDog {}.into(),
    "cat" => AsyncCat {}.into(),
    _ => panic!("unknown speaker"),
  };

  speaker.say().await;
}
