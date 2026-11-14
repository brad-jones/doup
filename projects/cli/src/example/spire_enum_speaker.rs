//! Enum Dispatch Pattern Example
//!
//! Powered by: <https://crates.io/crates/spire_enum>
//!
//! The trait is left untouched. `#[delegated_enum]` goes on the enum and
//! `#[delegate_impl]` on a bodiless impl block, which is filled in for us.
//! `impl_conversions` generates the `From` impls.

#![allow(dead_code)]
use spire_enum::prelude::{delegate_impl, delegated_enum};

trait Speak {
  fn say(&self);
}

#[delegated_enum(impl_conversions)]
enum SpeakKind {
  Dog(Dog),
  Cat(Cat),
}

#[delegate_impl]
impl Speak for SpeakKind {
  fn say(&self);
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
