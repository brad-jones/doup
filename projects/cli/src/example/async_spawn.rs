//! Plain `async fn` in a trait + `tokio::spawn` inside generic code: this is the error case.
//!
//! The failing function is switched off with `#[cfg(any())]` (a condition that is never true)
//! so the crate still compiles. Delete that one line to see the compiler error.
//! See `future_spawn.rs` for the fix.

#![allow(dead_code)]

trait Speak: Send + Sync + 'static {
  async fn say(&self);
}

struct Dog;

impl Speak for Dog {
  async fn say(&self) {
    println!("Woof");
  }
}

// Works: the compiler can see the concrete `Dog::say` future, so it knows it is `Send`.
fn spawn_concrete() {
  tokio::spawn(async move { Dog.say().await });
}

// Works: generic code that only `.await`s. The `Send` check happens at the `spawn` below,
// where the type is the concrete `Dog`.
async fn say_twice<S: Speak>(speaker: &S) {
  speaker.say().await;
  speaker.say().await;
}

fn spawn_generic_helper() {
  tokio::spawn(async move { say_twice(&Dog).await });
}

// Fails: here `S` is unknown, so the compiler only knows "S implements Speak" and has no
// idea whether the future returned by `S::say` is `Send`. `tokio::spawn` demands that it is:
//
//   error: future cannot be sent between threads safely
//          future created by async block is not `Send`
//
// Note `S: Send + Sync + 'static` is already satisfied (via the `Speak` supertraits). The
// problem is the *future*, not the speaker.
#[cfg(any())]
fn spawn_generic<S: Speak>(speaker: S) {
  tokio::spawn(async move { speaker.say().await });
}
