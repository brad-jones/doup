//! Enum Dispatch Pattern Example
//!
//! This avoids allocating on the heap & a runtime lookup,
//! at the expense of a larger binary size.
//!
//! This example has been manually written but the boiler plate
//! can be generated, see `macro_enum_dispatch.rs`.

#![allow(dead_code)]

trait Speak {
  fn say(&self);
}

enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

impl Speak for SpeakKind {
  fn say(&self) {
    match self {
      SpeakKind::Dog(speaker) => speaker.say(),
      SpeakKind::Cat(speaker) => speaker.say(),
    }
  }
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
