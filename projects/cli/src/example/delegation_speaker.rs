//! Enum Dispatch Pattern Example
//!
//! Powered by: <https://crates.io/crates/delegation>
//!
//! Both the trait and the enum are marked with `#[delegate]`. It does not
//! generate `From` impls, so those are written by hand.

#![allow(dead_code)]
use delegation::delegate;

#[delegate]
trait Speak {
  fn say(&self);
}

#[delegate(derive(Speak))]
enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

struct Dog;

impl Speak for Dog {
  fn say(&self) {
    println!("Woof");
  }
}

impl From<Dog> for SpeakKind {
  fn from(speaker: Dog) -> Self {
    SpeakKind::Dog(speaker)
  }
}

struct Cat;

impl Speak for Cat {
  fn say(&self) {
    println!("Meow");
  }
}

impl From<Cat> for SpeakKind {
  fn from(speaker: Cat) -> Self {
    SpeakKind::Cat(speaker)
  }
}

fn example() {
  let which_speaker_to_use = "dog";

  let speaker: SpeakKind = match which_speaker_to_use {
    "dog" => Dog {}.into(),
    "cat" => Cat {}.into(),
    _ => panic!("unknown speaker"),
  };

  speaker.say();
}
