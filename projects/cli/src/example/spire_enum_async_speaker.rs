//! Enum Dispatch Pattern Example
//!
//! Powered by: <https://crates.io/crates/spire_enum>
//!
//! Async version.

#![allow(dead_code)]
use spire_enum::prelude::{delegate_impl, delegated_enum};
use std::future::Future;

trait AsyncSpeak: Send + Sync {
  fn say(&self) -> impl Future<Output = ()> + Send;
}

#[delegated_enum(impl_conversions)]
enum AsyncSpeakKind {
  AsyncDog(AsyncDog),
  AsyncCat(AsyncCat),
}

impl AsyncSpeakKind {
  fn new(backend: impl Into<Self>) -> Self {
    backend.into()
  }
}

#[delegate_impl]
impl AsyncSpeak for AsyncSpeakKind {
  async fn say(&self);
}

struct AsyncDog;

impl AsyncSpeak for AsyncDog {
  async fn say(&self) {
    println!("Woof");
  }
}

struct AsyncCat;

impl AsyncSpeak for AsyncCat {
  async fn say(&self) {
    println!("Meow");
  }
}

async fn example() {
  let which_speaker_to_use = "dog";

  let speaker = match which_speaker_to_use {
    "dog" => AsyncSpeakKind::new(AsyncDog),
    "cat" => AsyncSpeakKind::new(AsyncCat),
    _ => panic!("unknown speaker"),
  };

  speaker.say().await;
}
