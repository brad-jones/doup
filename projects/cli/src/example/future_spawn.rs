//! The fix for `async_spawn.rs`: promise `Send` in the trait itself.
//!
//! Two separate promises are needed to spawn generic code:
//!   1. `Speak: Send + Sync`        - the speaker can be shared between threads.
//!   2. `-> impl Future<...> + Send` - the future returned by `say` can move between threads.
//!
//! Implementers still write a normal `async fn`; the compiler checks it keeps the promise.

#![allow(dead_code)]

use std::future::Future;

trait Speak: Send + Sync + 'static {
  fn say(&self) -> impl Future<Output = ()> + Send;
}

struct Dog;

impl Speak for Dog {
  async fn say(&self) {
    println!("Woof");
  }
}

struct Cat;

impl Speak for Cat {
  async fn say(&self) {
    println!("Meow");
  }
}

// Now compiles: the trait promises every `say` future is `Send`, even for an unknown `S`.
fn spawn_generic<S: Speak>(speaker: S) -> tokio::task::JoinHandle<()> {
  tokio::spawn(async move { speaker.say().await })
}

// The enum from the recommendation works with it unchanged.
enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

impl Speak for SpeakKind {
  async fn say(&self) {
    match self {
      SpeakKind::Dog(speaker) => speaker.say().await,
      SpeakKind::Cat(speaker) => speaker.say().await,
    }
  }
}

impl From<Dog> for SpeakKind {
  fn from(speaker: Dog) -> Self {
    SpeakKind::Dog(speaker)
  }
}

impl From<Cat> for SpeakKind {
  fn from(speaker: Cat) -> Self {
    SpeakKind::Cat(speaker)
  }
}

async fn example() {
  let which_speaker_to_use = "dog";

  let speaker: SpeakKind = match which_speaker_to_use {
    "dog" => Dog {}.into(),
    "cat" => Cat {}.into(),
    _ => panic!("unknown speaker"),
  };

  spawn_generic(speaker).await.unwrap();
}

// The promise is enforced. This implementation holds an `Rc` (not thread safe) across an
// `.await`, so its future is not `Send`. Delete the `#[cfg(any())]` line to see the error
// point at this impl:
//
//   error: future cannot be sent between threads safely
//          future returned by `say` is not `Send`
#[cfg(any())]
struct Bird;

#[cfg(any())]
impl Speak for Bird {
  async fn say(&self) {
    let not_thread_safe = std::rc::Rc::new(1);
    std::future::ready(()).await;
    drop(not_thread_safe);
  }
}
