//! Enum Dispatch Pattern Example
//!
//! Powered by: <https://crates.io/crates/enum_dispatch>

#![allow(dead_code)]
use enum_dispatch::*;

#[enum_dispatch(SpeakKind)]
trait Speak {
  fn say(&self);
}

#[enum_dispatch]
enum SpeakKind {
  Dog,
  Cat,
}

struct Dog;

impl Speak for Dog {
  fn say(&self) {
    println!("Woof");
  }
}

struct Cat;

impl Speak for Cat {
  fn say(&self) {
    println!("Meow");
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
