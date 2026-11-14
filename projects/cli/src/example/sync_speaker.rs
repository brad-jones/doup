//! Basic Box<dyn Trait> Example
//!
//! This works but falls over if we want to use async methods in our trait.
//! See `async_speaker.rs` for an async version that uses an async_trait macro.

#![allow(dead_code)]

trait Speak {
  fn say(&self);
}

struct Dog;

impl Speak for Dog {
  fn say(&self) {
    println!("Woof");
  }
}

impl From<Dog> for Box<dyn Speak> {
  fn from(speaker: Dog) -> Self {
    Box::new(speaker)
  }
}

struct Cat;

impl Speak for Cat {
  fn say(&self) {
    println!("Meow");
  }
}

impl From<Cat> for Box<dyn Speak> {
  fn from(speaker: Cat) -> Self {
    Box::new(speaker)
  }
}

fn example() {
  let which_speaker_to_use = "dog";

  let speaker: Box<dyn Speak> = match which_speaker_to_use {
    "dog" => Dog {}.into(),
    "cat" => Cat {}.into(),
    _ => panic!("unknown speaker"),
  };

  speaker.say();
}
